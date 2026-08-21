//! Both `kvs` interfaces driven end to end against a live node. No store yet,
//! so the assertions marked `empty store` are the ones that change when it lands.

use capnp::capability::{FromClientHook as _, Promise};
use dusk_capnp::dusk_capnp::{dusk, stream};
use dusk_connection::Connection;
use dusk_program::program_args::ProgramArgs;
use dusk_program_kvs::{Args as KvsArgs, Value, kvs_capnp};
use dusk_program_sh::sh_capnp;
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Records what a program streams, and whether it called `done`.
struct CaptureStream {
    values: Rc<RefCell<Vec<Value>>>,
    done: Rc<Cell<bool>>,
}

impl stream::Server for CaptureStream {
    fn send(&mut self, params: stream::SendParams) -> Promise<(), capnp::Error> {
        let value = match params
            .get()
            .and_then(|params| params.get_value())
            .and_then(Value::from_reader)
        {
            Ok(value) => value,
            Err(error) => return Promise::err(error),
        };
        self.values.borrow_mut().push(value);
        Promise::ok(())
    }

    fn done(
        &mut self,
        _params: stream::DoneParams,
        _results: stream::DoneResults,
    ) -> Promise<(), capnp::Error> {
        self.done.set(true);
        Promise::ok(())
    }
}

/// `Dusk.process`, then `Dusk.run`, then `Process.portal` — as the shell does.
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
    let done = Rc::new(Cell::new(false));
    let mut output_request = portal
        .cast_to::<sh_capnp::output_portal::Client>()
        .output_request();
    output_request
        .get()
        .set_stream(capnp_rpc::new_client(CaptureStream {
            values: values.clone(),
            done: done.clone(),
        }));
    output_request.send().promise.await.unwrap();

    let values = values.borrow().clone();
    (pid, values, done.get())
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

            let (pid, values, done) = run_action(
                &client,
                KvsArgs::set("a", &Value::String("1".to_string()))
                    .unwrap()
                    .as_program_args()
                    .unwrap(),
            )
            .await;
            assert!(values.is_empty(), "set streamed {values:?}");
            assert!(done, "set must release the shell by calling done");
            stop(&client, pid).await;

            let (pid, values, done) =
                run_action(&client, KvsArgs::get("a").as_program_args().unwrap()).await;
            assert_eq!(values, vec![Value::Null], "empty store: get is null"); // empty store
            assert!(done);
            stop(&client, pid).await;

            let (pid, values, done) =
                run_action(&client, KvsArgs::exists("a").as_program_args().unwrap()).await;
            assert_eq!(
                values,
                vec![Value::Bool(false)],
                "empty store: exists is false"
            ); // empty store
            assert!(done);
            stop(&client, pid).await;

            let (pid, values, done) =
                run_action(&client, KvsArgs::delete("a").as_program_args().unwrap()).await;
            assert_eq!(
                values,
                vec![Value::Bool(false)],
                "empty store: delete removed nothing"
            ); // empty store
            assert!(done);
            stop(&client, pid).await;

            // bind withholds `done`, so the shell must leave it running.
            let (pid, values, done) =
                run_action(&client, KvsArgs::bind().as_program_args().unwrap()).await;
            assert!(values.is_empty(), "bind streamed {values:?}");
            assert!(!done, "bind must daemonize by withholding done");

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

            let mut set_request = kvs.set_request();
            set_request.get().set_key("a");
            Value::String("1".to_string())
                .write_to_builder(set_request.get().init_value())
                .unwrap();
            set_request.send().promise.await.unwrap();

            let get_reply = {
                let mut get_request = kvs.get_request();
                get_request.get().set_key("a");
                get_request.send().promise.await.unwrap()
            };
            let value = Value::from_reader(get_reply.get().unwrap().get_value().unwrap()).unwrap();
            assert_eq!(value, Value::Null, "empty store: get is null"); // empty store

            let mut exists_request = kvs.exists_request();
            exists_request.get().set_key("a");
            let exists = exists_request
                .send()
                .promise
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_exists();
            assert!(!exists, "empty store: exists is false"); // empty store

            let mut delete_request = kvs.delete_request();
            delete_request.get().set_key("a");
            let deleted = delete_request
                .send()
                .promise
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_deleted();
            assert!(!deleted, "empty store: delete removed nothing"); // empty store

            stop(&client, pid).await;
            connection.disconnect().await.unwrap();
        })
        .await;
}
