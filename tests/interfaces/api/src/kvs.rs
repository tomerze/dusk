//! Both `kvs` interfaces driven end to end against a live node.
//!
//! The store lives in the launcher, so the args interface reads back across
//! processes: each action below is its own process, and a value one of them
//! sets is there for the next.

use capnp::capability::{FromClientHook as _, Promise};
use dusk_capnp::dusk_capnp::dusk;
use dusk_connection::Connection;
use dusk_program::program_args::ProgramArgs;
use dusk_program::stream::{Stream, StreamMixin};
use dusk_program_kvs::{
    Args as KvsArgs, Record, Value, client::key_display, kvs::key_id, kvs_capnp,
};
use dusk_program_logs::client::LogsArgs;
use dusk_program_logs::common_capnp::any_value;
use dusk_program_logs::{FLAG_REPLAY, logs_args, signal};
use dusk_program_sh::{ShArgs, ShMode, sh_capnp};
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

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

/// `Dusk.process`, then `Dusk.run`, then `Process.portal` - as the shell does.
async fn start(
    client: &dusk::Client,
    program_args: Rc<ProgramArgs>,
) -> (u64, dusk_capnp::dusk_capnp::portal::Client) {
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
        .unwrap();

    (pid, portal)
}

/// Terminate a process and wait for it to leave the namespace.
async fn stop(client: &dusk::Client, pid: u64) {
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
}

/// Drive one args-interface action through `output`, as the interpreter does.
async fn run_action(
    client: &dusk::Client,
    program_args: Rc<ProgramArgs>,
) -> (u64, Vec<Value>, bool) {
    let (pid, portal) = start(client, program_args).await;

    let values = Rc::new(RefCell::new(Vec::new()));
    let mut output_request = portal
        .cast_to::<sh_capnp::output_portal::Client>()
        .output_request();
    output_request
        .get()
        .set_stream(capnp_rpc::new_client(Stream::new(CaptureStream {
            values: values.clone(),
        })));
    let output_reply = output_request.send().promise.await.unwrap();
    let daemonize = output_reply.get().unwrap().get_daemonize();

    let values = values.borrow().clone();
    (pid, values, daemonize)
}

/// The interface the shell drives: one action per invocation, carried in args.
#[tokio::test(flavor = "current_thread")]
async fn test_kvs_args_interface() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let address: std::net::SocketAddr =
                format!("{}:{}", LISTEN_ADDRESS, port).parse().unwrap();
            let connection = Connection::connect(address).await.unwrap();
            let client = connection.client().await;

            let key = key_id("args.interface.key");

            let (pid, values, daemonize) = run_action(
                &client,
                KvsArgs::set(key, &Value::String("1".to_string()), 0, false)
                    .unwrap()
                    .as_program_args()
                    .unwrap(),
            )
            .await;
            assert!(values.is_empty(), "set streamed {values:?}");
            assert!(!daemonize, "set must let the shell reap it");
            stop(&client, pid).await;

            let (pid, values, daemonize) =
                run_action(&client, KvsArgs::get(&[key]).as_program_args().unwrap()).await;
            assert_eq!(
                values,
                vec![Value::Record(Record::with_fields(
                    kvs_capnp::SCAN_TYPE_ID,
                    [
                        (
                            b"Key".to_vec(),
                            Value::List(vec![Value::String(key_display(key))])
                        ),
                        (
                            b"Value".to_vec(),
                            Value::List(vec![Value::String("1".to_string())])
                        ),
                    ],
                ))],
                "get must read back what an earlier process set"
            );
            assert!(!daemonize);
            stop(&client, pid).await;

            let (pid, values, daemonize) =
                run_action(&client, KvsArgs::exists(key).as_program_args().unwrap()).await;
            assert_eq!(values, vec![Value::Bool(true)], "the key is present");
            assert!(!daemonize);
            stop(&client, pid).await;

            let (pid, values, daemonize) = run_action(
                &client,
                KvsArgs::delete(key, false).as_program_args().unwrap(),
            )
            .await;
            assert_eq!(values, vec![Value::Bool(true)], "delete removed the key");
            assert!(!daemonize);
            stop(&client, pid).await;

            // Reading the deleted key is covered by the portal test: here it
            // would exit `main` before the portal `run_action` needs exists.

            let (pid, values, daemonize) =
                run_action(&client, KvsArgs::server().as_program_args().unwrap()).await;
            assert_eq!(
                values,
                vec![Value::Text("running in server mode".to_string())],
                "server streamed {values:?}"
            );
            assert!(daemonize, "server must daemonize");

            let ps_reply = client.ps_request().send().promise.await.unwrap();
            let entries = ps_reply.get().unwrap().get_process_entries().unwrap();
            let pids: Vec<u64> = entries.iter().map(|entry| entry.get_pid()).collect();
            assert!(
                pids.contains(&pid),
                "kvs server {pid} must still be running"
            );

            stop(&client, pid).await;
            connection.disconnect().await.unwrap();
        })
        .await;
}

/// The interface a bound client drives: the four operations as portal RPCs.
#[tokio::test(flavor = "current_thread")]
async fn test_kvs_portal_interface() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let address: std::net::SocketAddr =
                format!("{}:{}", LISTEN_ADDRESS, port).parse().unwrap();
            let connection = Connection::connect(address).await.unwrap();
            let client = connection.client().await;

            let (pid, portal) = start(&client, KvsArgs::server().as_program_args().unwrap()).await;
            let kvs = portal.cast_to::<kvs_capnp::kvs_portal::Client>();

            let key = key_id("portal.interface.key");

            let mut set_request = kvs.set_request();
            set_request.get().set_key(key);
            Value::String("1".to_string())
                .write_to_builder(set_request.get().init_value())
                .unwrap();
            set_request.send().promise.await.unwrap();

            let get_reply = {
                let mut get_request = kvs.get_request();
                get_request.get().set_key(key);
                get_request.send().promise.await.unwrap()
            };
            let value = Value::from_reader(get_reply.get().unwrap().get_value().unwrap()).unwrap();
            assert_eq!(
                value,
                Value::String("1".to_string()),
                "get must read back what set stored"
            );

            let mut exists_request = kvs.exists_request();
            exists_request.get().set_key(key);
            let exists = exists_request
                .send()
                .promise
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_exists();
            assert!(exists, "the key is present");

            let mut delete_request = kvs.delete_request();
            delete_request.get().set_key(key);
            let deleted = delete_request
                .send()
                .promise
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_deleted();
            assert!(deleted, "delete removed the key");

            let error = {
                let mut get_request = kvs.get_request();
                get_request.get().set_key(key);
                match get_request.send().promise.await {
                    Ok(_) => panic!("reading a deleted key must fail"),
                    Err(error) => error,
                }
            };
            assert!(
                error.to_string().contains("not found"),
                "the failure must name the cause: {error}"
            );

            // A stored null is a value: it reads back, where an absent key failed.
            let mut set_request = kvs.set_request();
            set_request.get().set_key(key);
            Value::Null
                .write_to_builder(set_request.get().init_value())
                .unwrap();
            set_request.send().promise.await.unwrap();

            let get_reply = {
                let mut get_request = kvs.get_request();
                get_request.get().set_key(key);
                get_request.send().promise.await.unwrap()
            };
            assert_eq!(
                Value::from_reader(get_reply.get().unwrap().get_value().unwrap()).unwrap(),
                Value::Null,
                "the stored null reads back as null"
            );

            stop(&client, pid).await;
            connection.disconnect().await.unwrap();
        })
        .await;
}

async fn run_to_exit(client: &dusk::Client, program_args: Rc<ProgramArgs>) -> capnp::Error {
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
    let pid = process
        .pid_request()
        .send()
        .promise
        .await
        .unwrap()
        .get()
        .unwrap()
        .get_result();
    let Err(error) = process.run_request().send().promise.await else {
        panic!("process {pid} was expected to fail");
    };
    let mut waitpid_request = client.waitpid_request();
    waitpid_request.get().set_pid(pid);
    assert!(
        waitpid_request.send().promise.await.is_err(),
        "waitpid must answer the failure of process {pid}"
    );
    error
}

async fn scanned_flags(client: &dusk::Client, key: u64) -> Value {
    let (pid, values, daemonize) =
        run_action(client, KvsArgs::scan().as_program_args().unwrap()).await;
    assert!(!daemonize);
    stop(client, pid).await;
    for value in values {
        let Value::Record(page) = value else {
            panic!("kvs scan sent {value:?} where a page of keys belongs");
        };
        let (Some(Value::List(ids)), Some(Value::List(flags))) = (
            page.fields.get(b"ID".as_slice()),
            page.fields.get(b"Flags".as_slice()),
        ) else {
            panic!("kvs scan must send ID and Flags columns: {page:?}");
        };
        if let Some(index) = ids.iter().position(|id| *id == Value::Uint(key)) {
            return flags[index].clone();
        }
    }
    panic!("kvs scan did not list {}", key_display(key));
}

async fn get_value(client: &dusk::Client, key: u64) -> Value {
    let (pid, values, _) =
        run_action(client, KvsArgs::get(&[key]).as_program_args().unwrap()).await;
    stop(client, pid).await;
    let [Value::Record(page)] = values.as_slice() else {
        panic!("kvs get sent {values:?} where one page of keys belongs");
    };
    let Some(Value::List(column)) = page.fields.get(b"Value".as_slice()) else {
        panic!("kvs get must send a Value column: {page:?}");
    };
    column[0].clone()
}

#[tokio::test(flavor = "current_thread")]
async fn test_kvs_sticky_keys() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let address: std::net::SocketAddr =
                format!("{}:{}", LISTEN_ADDRESS, port).parse().unwrap();
            let connection = Connection::connect(address).await.unwrap();
            let client = connection.client().await;

            let key = key_id("dusk.namespace_id");
            let namespace_id = get_value(&client, key).await;
            assert!(
                matches!(namespace_id, Value::Uint(_)),
                "init records the namespace id: {namespace_id:?}"
            );
            assert_eq!(
                scanned_flags(&client, key).await,
                Value::String("sticky".to_string()),
                "a key dusk writes is sticky"
            );

            let forged = Value::String("forged".to_string());
            let error = run_to_exit(
                &client,
                KvsArgs::set(key, &forged, 0, false)
                    .unwrap()
                    .as_program_args()
                    .unwrap(),
            )
            .await;
            assert!(
                error.to_string().contains("is sticky"),
                "kvs set must refuse a sticky key: {error}"
            );
            let error = run_to_exit(
                &client,
                KvsArgs::delete(key, false).as_program_args().unwrap(),
            )
            .await;
            assert!(
                error.to_string().contains("is sticky"),
                "kvs delete must refuse a sticky key: {error}"
            );
            assert_eq!(
                get_value(&client, key).await,
                namespace_id,
                "a refused set and delete leave the key as dusk wrote it"
            );

            let (pid, values, daemonize) = run_action(
                &client,
                KvsArgs::set(key, &forged, 0, true)
                    .unwrap()
                    .as_program_args()
                    .unwrap(),
            )
            .await;
            assert!(values.is_empty(), "set streamed {values:?}");
            assert!(!daemonize);
            stop(&client, pid).await;
            assert_eq!(
                get_value(&client, key).await,
                forged,
                "--forbidden-unstick sets a sticky key"
            );
            assert_eq!(
                scanned_flags(&client, key).await,
                Value::String("sticky".to_string()),
                "a name dusk owns stays sticky after an override"
            );
            let error = run_to_exit(
                &client,
                KvsArgs::set(key, &namespace_id, 0, false)
                    .unwrap()
                    .as_program_args()
                    .unwrap(),
            )
            .await;
            assert!(
                error.to_string().contains("is sticky"),
                "kvs set must still refuse a name dusk owns after an override: {error}"
            );

            let (pid, values, _) = run_action(
                &client,
                KvsArgs::delete(key, true).as_program_args().unwrap(),
            )
            .await;
            assert_eq!(
                values,
                vec![Value::Bool(true)],
                "--forbidden-unstick deletes a key dusk owns"
            );
            stop(&client, pid).await;

            let absent = key_id("dusk.os.windows.edition");
            assert_eq!(
                key_display(absent),
                "dusk.os.windows.edition",
                "init's list of the names it owns names them for a client"
            );
            let (pid, values, _) =
                run_action(&client, KvsArgs::exists(absent).as_program_args().unwrap()).await;
            assert_eq!(
                values,
                vec![Value::Bool(false)],
                "a linux node writes no windows key"
            );
            stop(&client, pid).await;
            let error = run_to_exit(
                &client,
                KvsArgs::set(absent, &forged, 0, false)
                    .unwrap()
                    .as_program_args()
                    .unwrap(),
            )
            .await;
            assert!(
                error.to_string().contains("is sticky"),
                "kvs set must refuse a name dusk owns that holds no value yet: {error}"
            );
            let error = run_to_exit(
                &client,
                KvsArgs::delete(absent, false).as_program_args().unwrap(),
            )
            .await;
            assert!(
                error.to_string().contains("is sticky"),
                "kvs delete must refuse a name dusk owns that holds no value yet: {error}"
            );
            let (pid, values, _) = run_action(
                &client,
                KvsArgs::set(absent, &forged, 0, true)
                    .unwrap()
                    .as_program_args()
                    .unwrap(),
            )
            .await;
            assert!(values.is_empty(), "set streamed {values:?}");
            stop(&client, pid).await;
            assert_eq!(
                get_value(&client, absent).await,
                forged,
                "--forbidden-unstick sets a name dusk owns that held no value"
            );

            let arch = key_id("dusk.target.arch");
            assert_eq!(
                scanned_flags(&client, arch).await,
                Value::String("sticky".to_string()),
                "init writes dusk.target.arch sticky"
            );
            let (pid, values, _) = run_action(
                &client,
                KvsArgs::delete(arch, true).as_program_args().unwrap(),
            )
            .await;
            assert_eq!(
                values,
                vec![Value::Bool(true)],
                "--forbidden-unstick deletes a sticky key"
            );
            stop(&client, pid).await;
            let (pid, values, _) =
                run_action(&client, KvsArgs::exists(arch).as_program_args().unwrap()).await;
            assert_eq!(values, vec![Value::Bool(false)], "the sticky key is gone");
            stop(&client, pid).await;

            let (server, portal) =
                start(&client, KvsArgs::server().as_program_args().unwrap()).await;
            let kvs = portal.cast_to::<kvs_capnp::kvs_portal::Client>();
            let os = key_id("dusk.target.os");
            let mut set_request = kvs.set_request();
            set_request.get().set_key(os);
            forged
                .write_to_builder(set_request.get().init_value())
                .unwrap();
            let Err(error) = set_request.send().promise.await else {
                panic!("the portal set a sticky key without forbiddenUnstick");
            };
            assert!(
                error.to_string().contains("is sticky"),
                "the portal's set must refuse a sticky key: {error}"
            );
            let mut delete_request = kvs.delete_request();
            delete_request.get().set_key(os);
            let Err(error) = delete_request.send().promise.await else {
                panic!("the portal deleted a sticky key without forbiddenUnstick");
            };
            assert!(
                error.to_string().contains("is sticky"),
                "the portal's delete must refuse a sticky key: {error}"
            );
            let mut set_request = kvs.set_request();
            set_request.get().set_key(os);
            set_request.get().set_forbidden_unstick(true);
            forged
                .write_to_builder(set_request.get().init_value())
                .unwrap();
            set_request.send().promise.await.unwrap();
            let mut get_request = kvs.get_request();
            get_request.get().set_key(os);
            let reply = get_request.send().promise.await.unwrap();
            assert_eq!(
                Value::from_reader(reply.get().unwrap().get_value().unwrap()).unwrap(),
                forged,
                "the portal's set with forbiddenUnstick sets a sticky key"
            );
            stop(&client, server).await;

            connection.disconnect().await.unwrap();
        })
        .await;
}

#[test]
fn test_a_launcher_owns_its_names_for_its_own_node_until_it_is_dropped() {
    let written = key_id("logs.written");
    std::thread::spawn(move || {
        let kvs = dusk_program_kvs::kvs::Kvs::new();
        let launcher =
            dusk_program_logs::Launcher::new(dusk_program_logs::LogsConfig::default()).unwrap();
        assert!(
            dusk_program::embassy_futures::block_on(kvs.set_unless_sticky(
                written,
                Value::Uint(1),
                0
            ))
            .is_err(),
            "logs owns logs.written for the node on its thread"
        );
        std::thread::spawn(move || {
            let other = dusk_program_kvs::kvs::Kvs::new();
            assert!(
                dusk_program::embassy_futures::block_on(other.set_unless_sticky(
                    written,
                    Value::Uint(1),
                    0
                ))
                .is_ok(),
                "a node on another thread does not share the ownership"
            );
        })
        .join()
        .unwrap();
        drop(launcher);
        assert!(
            dusk_program::embassy_futures::block_on(kvs.set_unless_sticky(
                written,
                Value::Uint(1),
                0
            ))
            .is_ok(),
            "a dropped launcher's names are free again"
        );
    })
    .join()
    .unwrap();
}

async fn run_line(client: &dusk::Client, line: &str) -> Vec<Value> {
    let script = dusk_program_sh::compile(client.clone(), line)
        .await
        .unwrap();
    let program_args = ShArgs::new(ShMode::Script(script))
        .unwrap()
        .as_program_args()
        .unwrap();
    let (pid, values, _) = run_action(client, program_args).await;
    stop(client, pid).await;
    values
}

struct LogCapture {
    batches: Arc<Mutex<Vec<Vec<u8>>>>,
    set_keys: Arc<Mutex<Vec<u64>>>,
}

impl logs_args::stream::Server for LogCapture {
    fn send(&mut self, params: logs_args::stream::SendParams) -> Promise<(), capnp::Error> {
        let signal_batch =
            dusk_capnp::pry!(params.get().and_then(|params| params.get_signal_batch()));
        let signals = dusk_capnp::pry!(signal_batch.get_signals());
        let acknowledgement = dusk_capnp::pry!(signal_batch.get_ack());
        let mut message = capnp::message::Builder::new_default();
        dusk_capnp::pry!(message.set_root(signals));
        self.batches
            .lock()
            .unwrap()
            .push(capnp::serialize::write_message_to_words(&message));
        for entry in signals.iter() {
            let Ok(signal::Which::LogRecord(Ok(log_record))) = entry.which() else {
                continue;
            };
            let body = log_record
                .get_body()
                .ok()
                .and_then(|body| body.which().ok())
                .and_then(|which| match which {
                    any_value::Which::StringValue(Ok(text)) => {
                        text.to_str().ok().map(str::to_string)
                    }
                    _ => None,
                });
            if body.as_deref() != Some("kvs set") {
                continue;
            }
            for attribute in log_record.get_attributes().into_iter().flatten() {
                if attribute.get_key().ok().and_then(|key| key.to_str().ok()) != Some("key") {
                    continue;
                }
                let key = attribute
                    .get_value()
                    .ok()
                    .and_then(|value| value.which().ok())
                    .and_then(|which| match which {
                        any_value::Which::IntValue(number) => Some(number as u64),
                        any_value::Which::StringValue(Ok(text)) => {
                            text.to_str().ok().and_then(|text| text.parse().ok())
                        }
                        _ => None,
                    });
                self.set_keys.lock().unwrap().extend(key);
            }
        }
        Promise::from_future(async move {
            acknowledgement.ack_request().send().promise.await?;
            Ok(())
        })
    }

    fn stop(
        &mut self,
        _params: logs_args::stream::StopParams,
        _results: logs_args::stream::StopResults,
    ) -> Promise<(), capnp::Error> {
        Promise::from_future(std::future::pending())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_kvs_sensitive_value_stays_out_of_the_logs() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let address: std::net::SocketAddr =
                format!("{}:{}", LISTEN_ADDRESS, port).parse().unwrap();
            let connection = Connection::connect(address).await.unwrap();
            let client = connection.client().await;

            let secret = format!(
                "sensitive-value-{port}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let key = key_id("secret.token");

            let values = run_line(
                &client,
                &format!("kvs set --sensitive secret.token {secret}"),
            )
            .await;
            assert!(values.is_empty(), "set streamed {values:?}");
            assert_eq!(
                get_value(&client, key).await,
                Value::String(secret.clone()),
                "kvs get still answers a sensitive value"
            );
            assert_eq!(
                scanned_flags(&client, key).await,
                Value::String("sensitive".to_string()),
                "kvs scan names the flag"
            );
            let error = run_to_exit(
                &client,
                KvsArgs::set(
                    key_id("client.sticky"),
                    &Value::String("1".to_string()),
                    dusk_program_kvs::kvs::FLAG_STICKY,
                    false,
                )
                .unwrap()
                .as_program_args()
                .unwrap(),
            )
            .await;
            assert!(
                error.to_string().contains("a client cannot set"),
                "a client's set refuses every flag but sensitive: {error}"
            );

            let values = run_line(&client, "kvs get secret.token").await;
            assert!(
                format!("{values:?}").contains(&secret),
                "kvs get through the shell answers the value: {values:?}"
            );

            let batches = Arc::new(Mutex::new(Vec::new()));
            let set_keys = Arc::new(Mutex::new(Vec::new()));
            let capture = LogCapture {
                batches: batches.clone(),
                set_keys: set_keys.clone(),
            };
            let stream: logs_args::stream::Client = capnp_rpc::new_client(capture);
            let program_args = LogsArgs::new(None, FLAG_REPLAY, move || Ok(stream.clone()))
                .as_program_args()
                .unwrap();
            let (pid, _, _) = run_action(&client, program_args).await;
            stop(&client, pid).await;

            assert!(
                set_keys.lock().unwrap().contains(&key),
                "the captured logs must hold the kvs set of {}, or they prove nothing",
                key_display(key)
            );
            let batches = core::mem::take(&mut *batches.lock().unwrap());
            assert!(!batches.is_empty(), "the node replayed no logs");
            let secret = secret.as_bytes();
            for batch in batches.iter() {
                assert!(
                    !batch.windows(secret.len()).any(|window| window == secret),
                    "a sensitive value reached the node's logs"
                );
            }

            connection.disconnect().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_kvs_set_persistent_fails_without_a_file() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let address: std::net::SocketAddr =
                format!("{}:{}", LISTEN_ADDRESS, port).parse().unwrap();
            let connection = Connection::connect(address).await.unwrap();
            let client = connection.client().await;

            let key = key_id("persistent.without.file");
            let error = run_to_exit(
                &client,
                KvsArgs::set(
                    key,
                    &Value::String("1".to_string()),
                    dusk_program_kvs::kvs::FLAG_PERSISTENT,
                    false,
                )
                .unwrap()
                .as_program_args()
                .unwrap(),
            )
            .await;
            assert!(
                error
                    .to_string()
                    .contains("this node keeps no persistent kvs keys"),
                "kvs set --persistent names why it failed: {error}"
            );
            let (pid, values, _) =
                run_action(&client, KvsArgs::exists(key).as_program_args().unwrap()).await;
            assert_eq!(
                values,
                vec![Value::Bool(false)],
                "the failed set stored nothing"
            );
            stop(&client, pid).await;

            connection.disconnect().await.unwrap();
        })
        .await;
}

#[test]
fn test_one_file_holds_one_kvs_launcher_at_a_time() {
    let path = format!(
        "{}/dusk-kvs-launcher-{}",
        std::env::temp_dir().display(),
        std::process::id()
    );
    let config = || dusk_program_kvs::KvsConfig {
        persistent: Some(path.clone()),
    };
    let first = dusk_program_kvs::Launcher::new(config()).unwrap();
    let on_another_thread = std::thread::scope(|scope| {
        scope
            .spawn(|| dusk_program_kvs::Launcher::new(config()).err())
            .join()
            .unwrap()
    });
    let Some(error) = on_another_thread else {
        panic!("a second node in the process took a file another node holds");
    };
    assert!(
        error
            .to_string()
            .contains("another node of this process keeps its persistent kvs keys"),
        "unexpected error: {error:#}"
    );
    let Err(error) = dusk_program_kvs::Launcher::new(dusk_program_kvs::KvsConfig {
        persistent: Some(format!("{path}-other")),
    }) else {
        panic!("one thread registered two persistent files");
    };
    assert!(
        error
            .to_string()
            .contains("already keeps its persistent kvs keys"),
        "unexpected error: {error:#}"
    );
    drop(first);
    std::thread::scope(|scope| {
        scope
            .spawn(|| dusk_program_kvs::Launcher::new(config()).map(drop))
            .join()
            .unwrap()
    })
    .expect("a node takes a file whose last launcher was dropped");

    std::mem::forget(dusk_program_kvs::Launcher::new(config()).unwrap());
    dusk_program_kvs::kvs::release_persistent(dusk_core::driver::tid());
    std::thread::scope(|scope| {
        scope
            .spawn(|| dusk_program_kvs::Launcher::new(config()).map(drop))
            .join()
            .unwrap()
    })
    .expect("a node takes a file whose last node's thread released it, launcher leaked or not");
}
