use crate::kvs::start;
use capnp::capability::{FromClientHook as _, Promise};
use dusk_base::dusk_program_init::Args as InitArgs;
use dusk_capnp::dusk_capnp::dusk;
use dusk_connection::Connection;
use dusk_core::driver::DuskImplExit;
use dusk_program::stream::{Stream, StreamMixin};
use dusk_program::value::Value;
use dusk_program_sh::{ShArgs, ShMode, sh_capnp};
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};
use std::sync::mpsc;
use std::time::Duration;

struct ChannelStream {
    values: tokio::sync::mpsc::UnboundedSender<Value>,
}

impl StreamMixin for ChannelStream {
    fn send(&mut self, value: Value) -> Promise<(), capnp::Error> {
        self.values.send(value).unwrap();
        Promise::ok(())
    }

    fn end(&mut self) {}
}

#[tokio::test(flavor = "current_thread")]
async fn test_one_process_starts_and_stops_twenty_nodes() {
    let disconnected: dusk::Client = capnp_rpc::new_future_client(async {
        Err(capnp::Error::disconnected(
            "the test has no node to talk to".to_string(),
        ))
    });
    let init_script = dusk_program_sh::compile_to_words(disconnected, "kill 1")
        .await
        .unwrap();

    for node in 0..20 {
        let init_script = init_script.clone();
        let (exit_sender, exit_receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let init_program_args = InitArgs::new(&init_script)
                .unwrap()
                .as_program_args()
                .unwrap();
            init_program_args.set_pid(Some(1)).unwrap();
            let launcher_set = dusk_base::default_launcher_set().unwrap();
            exit_sender
                .send(dusk_nix::run(
                    0,
                    move || Ok(launcher_set.clone()),
                    init_program_args,
                ))
                .unwrap();
        });
        assert_eq!(
            exit_receiver.recv_timeout(Duration::from_secs(10)),
            Ok(DuskImplExit::Code(0)),
            "node {node} did not stop"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_one_node_runs_twenty_sessions_and_scripts_at_once() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let address: std::net::SocketAddr = format!("{LISTEN_ADDRESS}:{port}").parse().unwrap();
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let mut connections = Vec::new();
            let mut outputs = Vec::new();
            let mut pids = Vec::new();

            for _ in 0..20 {
                let connection = Connection::connect(address).await.unwrap();
                let client = connection.client().await;

                let script = dusk_program_sh::compile(client.clone(), "echo started; sleep 600000")
                    .await
                    .unwrap();
                let program_args = ShArgs::new(ShMode::Script(script))
                    .unwrap()
                    .as_program_args()
                    .unwrap();
                let (pid, portal) = start(&client, program_args).await;
                pids.push(pid);

                let mut output_request = portal
                    .cast_to::<sh_capnp::output_portal::Client>()
                    .output_request();
                output_request
                    .get()
                    .set_stream(capnp_rpc::new_client(Stream::new(ChannelStream {
                        values: sender.clone(),
                    })));
                outputs.push(output_request.send().promise);

                connections.push((connection, client));
            }

            for _ in &connections {
                tokio::time::timeout(Duration::from_secs(10), receiver.recv())
                    .await
                    .unwrap()
                    .unwrap();
            }

            for (_, client) in &connections {
                let ps_reply = client.ps_request().send().promise.await.unwrap();
                let entries = ps_reply.get().unwrap().get_process_entries().unwrap();
                for pid in &pids {
                    assert!(
                        entries
                            .iter()
                            .any(|entry| entry.get_pid() == *pid && !entry.get_exited()),
                        "sh {pid} is not running"
                    );
                }
            }

            for (connection, _) in connections {
                connection.disconnect().await.unwrap();
            }
        })
        .await;
}
