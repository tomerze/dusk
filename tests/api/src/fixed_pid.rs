const FIXED_PID: u64 = 4242;

struct CreatedCounter {
    calls: std::rc::Rc<std::cell::Cell<u32>>,
}

impl dusk_capnp::dusk_capnp::created::Server for CreatedCounter {
    fn created(
        &mut self,
        _params: dusk_capnp::dusk_capnp::created::CreatedParams,
        _results: dusk_capnp::dusk_capnp::created::CreatedResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        self.calls.set(self.calls.get() + 1);
        capnp::capability::Promise::ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_the_created_callback_fires_for_a_process_that_already_exists() {
    use dusk_connection::Connection;
    use dusk_program_ps::Args as PsArgs;
    use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};

    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let address: std::net::SocketAddr =
                format!("{}:{}", LISTEN_ADDRESS, port).parse().unwrap();
            let connection = Connection::connect(address).await.unwrap();
            let client = connection.client().await;

            let calls = std::rc::Rc::new(std::cell::Cell::new(0));
            for _ in 0..2 {
                let ps_program_args = PsArgs::new(None).as_program_args().unwrap();
                ps_program_args.set_pid(Some(FIXED_PID)).unwrap();
                ps_program_args
                    .set_created(capnp_rpc::new_client(CreatedCounter {
                        calls: calls.clone(),
                    }))
                    .unwrap();

                let mut process_request = client.process_request();
                ps_program_args
                    .with_reader(|reader| process_request.get().set_program_args(reader))
                    .unwrap();
                process_request
                    .send()
                    .promise
                    .await
                    .unwrap()
                    .get()
                    .unwrap()
                    .get_result()
                    .unwrap();
            }

            assert_eq!(
                calls.get(),
                2,
                "the created callback must fire for the second ask as well as the first"
            );

            connection.disconnect().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_args_that_fix_a_pid_create_the_process_at_that_pid() {
    use dusk_connection::Connection;
    use dusk_program_ps::Args as PsArgs;
    use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};

    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let address: std::net::SocketAddr =
                format!("{}:{}", LISTEN_ADDRESS, port).parse().unwrap();
            let connection = Connection::connect(address).await.unwrap();
            let client = connection.client().await;

            let ps_program_args = PsArgs::new(None).as_program_args().unwrap();
            ps_program_args.set_pid(Some(FIXED_PID)).unwrap();

            let mut process_request = client.process_request();
            ps_program_args
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
            assert_eq!(
                pid, FIXED_PID,
                "a process built from args fixing pid {FIXED_PID} must hold that pid"
            );

            let ps_reply = client.ps_request().send().promise.await.unwrap();
            let entries = ps_reply.get().unwrap().get_process_entries().unwrap();
            let pids: Vec<u64> = entries.iter().map(|entry| entry.get_pid()).collect();
            assert!(
                pids.contains(&FIXED_PID),
                "pid {FIXED_PID} should be in ps once it is created, got {pids:?}"
            );

            connection.disconnect().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_the_same_fixed_pid_answers_with_the_same_process() {
    use dusk_connection::Connection;
    use dusk_program_ps::Args as PsArgs;
    use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};

    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let address: std::net::SocketAddr =
                format!("{}:{}", LISTEN_ADDRESS, port).parse().unwrap();
            let connection = Connection::connect(address).await.unwrap();
            let client = connection.client().await;

            let create = || async {
                let ps_program_args = PsArgs::new(None).as_program_args().unwrap();
                ps_program_args.set_pid(Some(FIXED_PID)).unwrap();
                let mut process_request = client.process_request();
                ps_program_args
                    .with_reader(|reader| process_request.get().set_program_args(reader))
                    .unwrap();
                process_request
                    .send()
                    .promise
                    .await
                    .unwrap()
                    .get()
                    .unwrap()
                    .get_result()
                    .unwrap()
            };

            let suspended = || async {
                let ps_reply = client.ps_request().send().promise.await.unwrap();
                let entries = ps_reply.get().unwrap().get_process_entries().unwrap();
                let mut matching = entries.iter().filter(|entry| entry.get_pid() == FIXED_PID);
                let entry = matching.next().expect("the process must be in ps");
                assert!(
                    matching.next().is_none(),
                    "pid {FIXED_PID} must appear in ps once"
                );
                entry.get_suspended()
            };

            let process = create().await;

            let mut run_request = client.run_request();
            run_request.get().set_process(process.clone());
            run_request.send().promise.await.unwrap();
            for _ in 0..100 {
                if !suspended().await {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert!(!suspended().await, "Dusk.run lifts the suspension");

            let second = create().await;
            let second_pid = second
                .pid_request()
                .send()
                .promise
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_result();
            assert_eq!(
                second_pid, FIXED_PID,
                "asking again for pid {FIXED_PID} must answer with that pid"
            );
            assert!(
                !suspended().await,
                "asking again for pid {FIXED_PID} must answer with the process already running, \
                 not a fresh suspended one"
            );

            connection.disconnect().await.unwrap();
        })
        .await;
}
