use capnp::capability::{FromClientHook as _, Promise};
use dusk_capnp::dusk_capnp::dusk;
use dusk_connection::Connection;
use dusk_program::program_args::ProgramArgs;
use dusk_program::stream::{Stream, StreamMixin};
use dusk_program_kvs::{Value, kvs::key_id};
use dusk_program_sh::{ShArgs, ShMode, sh_capnp};
use dusk_tests::{LISTEN_ADDRESS, gen_port};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

const REPLAY_WARNING: &str = "Dusk wrote this key before the persistent kvs file opened";

const REWRITE_WARNING: &str = "Dusk wrote this key after the persistent kvs file opened";

struct Node {
    child: Child,
    port: u16,
}

impl Node {
    fn start(path: &Path) -> Node {
        let port = gen_port();
        let mut child = Command::new(env!("CARGO_BIN_EXE_kvs_persistent_node"))
            .arg(format!("{LISTEN_ADDRESS}:{port}"))
            .arg(path)
            .stdout(Stdio::null())
            .spawn()
            .expect("start a node");
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while std::net::TcpStream::connect((LISTEN_ADDRESS, port)).is_err() {
            if let Some(status) = child.try_wait().expect("ask whether the node exited") {
                panic!("the node exited before it listened: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "the node did not listen on {port} within {STARTUP_TIMEOUT:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        Node { child, port }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        if let Err(error) = self.child.kill() {
            eprintln!("couldn't kill the node: {error}");
        }
        if let Err(error) = self.child.wait() {
            eprintln!("couldn't reap the node: {error}");
        }
    }
}

struct CaptureStream {
    values: Rc<RefCell<Vec<Value>>>,
}

impl StreamMixin for CaptureStream {
    fn send(&mut self, value: Value) -> Promise<(), capnp::Error> {
        self.values.borrow_mut().push(value);
        Promise::ok(())
    }

    fn end(&mut self) {}
}

async fn run(client: &dusk::Client, program_args: Rc<ProgramArgs>) -> Vec<Value> {
    let mut process_request = client.process_request();
    program_args
        .with_reader(|reader| process_request.get().set_program_args(reader))
        .unwrap();
    let process = process_request
        .send()
        .promise
        .await
        .unwrap()
        .get()
        .unwrap()
        .get_result()
        .unwrap();
    let mut run_request = client.run_request();
    run_request.get().set_process(process.clone());
    run_request.send().promise.await.unwrap();
    let pid = process
        .pid_request()
        .send()
        .promise
        .await
        .unwrap()
        .get()
        .unwrap()
        .get_result();
    let portal = process
        .portal_request()
        .send()
        .promise
        .await
        .unwrap()
        .get()
        .unwrap()
        .get_result()
        .unwrap()
        .cast_to::<sh_capnp::output_portal::Client>();
    let values = Rc::new(RefCell::new(Vec::new()));
    let mut output_request = portal.output_request();
    output_request
        .get()
        .set_stream(capnp_rpc::new_client(Stream::new(CaptureStream {
            values: values.clone(),
        })));
    output_request.send().promise.await.unwrap();

    let mut kill_request = client.kill_request();
    kill_request.get().set_pid(pid);
    kill_request.get().set_signal(15);
    if let Err(error) = kill_request.send().promise.await {
        assert!(
            error.to_string().contains("has exited"),
            "process {pid} could not be stopped: {error}"
        );
    }
    let mut waitpid_request = client.waitpid_request();
    waitpid_request.get().set_pid(pid);
    waitpid_request.send().promise.await.unwrap();
    values.take()
}

async fn run_line(client: &dusk::Client, line: &str) -> Vec<Value> {
    let script = dusk_program_sh::compile(client.clone(), line)
        .await
        .unwrap();
    let program_args = ShArgs::new(ShMode::Script(script))
        .unwrap()
        .as_program_args()
        .unwrap();
    run(client, program_args).await
}

fn column(values: &[Value], name: &[u8], key: u64) -> Option<Value> {
    for value in values {
        let Value::Record(page) = value else {
            panic!("kvs sent {value:?} where a page of keys belongs");
        };
        let (Some(Value::List(ids)), Some(Value::List(column))) =
            (page.fields.get(b"ID".as_slice()), page.fields.get(name))
        else {
            continue;
        };
        if let Some(index) = ids.iter().position(|id| *id == Value::Uint(key)) {
            return Some(column[index].clone());
        }
    }
    None
}

fn only_value(values: &[Value]) -> Option<Value> {
    let [Value::Record(page)] = values else {
        return None;
    };
    let Some(Value::List(column)) = page.fields.get(b"Value".as_slice()) else {
        return None;
    };
    column.first().cloned()
}

fn on_node<T>(node: &Node, work: impl AsyncFnOnce(&dusk::Client) -> T) -> T {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async {
        let address: std::net::SocketAddr =
            format!("{LISTEN_ADDRESS}:{}", node.port).parse().unwrap();
        let connection = Connection::connect(address).await.unwrap();
        let client = connection.client().await;
        let result = work(&client).await;
        connection.disconnect().await.unwrap();
        result
    })
}

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn new() -> ScratchDirectory {
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dusk-kvs-persistent-{}-{nanoseconds}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        ScratchDirectory { path }
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            eprintln!("couldn't remove {}: {error}", self.path.display());
        }
    }
}

#[test]
fn test_a_persistent_key_survives_a_restart() {
    let directory = ScratchDirectory::new();
    let path = directory.path.join("kvs");
    let token = key_id("restart.token");
    let memory = key_id("restart.memory");
    let secret = format!(
        "persistent-value-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let node = Node::start(&path);
    on_node(&node, async |client: &dusk::Client| {
        let values = run_line(
            client,
            &format!("kvs set --persistent --sensitive restart.token {secret}"),
        )
        .await;
        assert!(values.is_empty(), "set streamed {values:?}");
        run_line(client, "kvs set restart.memory forgotten").await;
        let scanned = run_line(client, "kvs scan").await;
        assert_eq!(
            column(&scanned, b"Flags", token),
            Some(Value::String("sensitive, persistent".to_string()))
        );
        run_line(
            client,
            "kvs set --persistent --forbidden-unstick dusk.impl overridden",
        )
        .await;
        assert_eq!(
            only_value(&run_line(client, "kvs get dusk.impl").await),
            Some(Value::String("overridden".to_string())),
            "--persistent --forbidden-unstick overrides a key the impl wrote"
        );
        run_line(
            client,
            "kvs set --persistent --forbidden-unstick dusk.version overridden",
        )
        .await;
    });
    drop(node);

    let bytes = std::fs::read(&path).expect("the node wrote its persistent file");
    assert!(!bytes.is_empty(), "the persistent file is empty");
    assert!(
        !bytes
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()),
        "the persistent file holds the value in the clear"
    );

    let node = Node::start(&path);
    on_node(&node, async |client: &dusk::Client| {
        let got = run_line(client, "kvs get restart.token").await;
        assert_eq!(
            only_value(&got),
            Some(Value::String(secret.clone())),
            "the restarted node reads back the persistent key: {got:?}"
        );
        let scanned = run_line(client, "kvs scan").await;
        assert_eq!(
            column(&scanned, b"Flags", token),
            Some(Value::String("sensitive, persistent".to_string())),
            "the key keeps its flags across the restart"
        );
        assert_eq!(
            column(&scanned, b"Flags", memory),
            None,
            "a key that was not persistent is gone"
        );
        assert_eq!(
            only_value(&run_line(client, "kvs get dusk.impl").await),
            Some(Value::String("nix".to_string())),
            "the impl's value written as the node started outlasts an override kept in the file"
        );
        assert_eq!(
            column(&scanned, b"Flags", key_id("dusk.impl")),
            Some(Value::String("sticky".to_string())),
            "the impl's key is sticky again and out of the file"
        );
        assert_ne!(
            only_value(&run_line(client, "kvs get dusk.version").await),
            Some(Value::String("overridden".to_string())),
            "init's value written after the file opened replaces an override kept in the file"
        );
        let warnings = format!(
            "{:?}",
            run_line(client, "logs dump --replay-only -l warn").await
        );
        assert!(
            warnings.contains(REPLAY_WARNING),
            "the node warns that it dropped the file's copy of a key the impl wrote: {warnings}"
        );
        assert!(
            warnings.contains(REWRITE_WARNING),
            "the node warns that it dropped the file's copy of a key init wrote: {warnings}"
        );

        run_line(client, "kvs delete restart.token").await;
    });
    drop(node);

    let node = Node::start(&path);
    on_node(&node, async |client: &dusk::Client| {
        let exists = run_line(client, "kvs exists restart.token").await;
        assert_eq!(
            exists,
            vec![Value::Bool(false)],
            "a deleted persistent key stays deleted across a restart"
        );
        let warnings = format!(
            "{:?}",
            run_line(client, "logs dump --replay-only -l warn").await
        );
        assert!(
            !warnings.contains(REPLAY_WARNING) && !warnings.contains(REWRITE_WARNING),
            "the overrides left the file at the previous start: {warnings}"
        );
    });
    drop(node);
}
