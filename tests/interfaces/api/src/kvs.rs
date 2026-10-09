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
use dusk_program_sh::sh_capnp;
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};
use std::cell::RefCell;
use std::rc::Rc;

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
    kill_request.send().promise.await.unwrap();

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
                run_action(&client, KvsArgs::bind().as_program_args().unwrap()).await;
            assert!(values.is_empty(), "bind streamed {values:?}");
            assert!(daemonize, "bind must daemonize");

            let ps_reply = client.ps_request().send().promise.await.unwrap();
            let entries = ps_reply.get().unwrap().get_process_entries().unwrap();
            let pids: Vec<u64> = entries.iter().map(|entry| entry.get_pid()).collect();
            assert!(pids.contains(&pid), "bound kvs {pid} must still be running");

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

            let (pid, portal) = start(&client, KvsArgs::bind().as_program_args().unwrap()).await;
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

            let (bound, portal) = start(&client, KvsArgs::bind().as_program_args().unwrap()).await;
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
            stop(&client, bound).await;

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
