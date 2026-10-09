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
                KvsArgs::set(key, &Value::String("1".to_string()), false)
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
