const FIXED_PID: u64 = 4242;

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
