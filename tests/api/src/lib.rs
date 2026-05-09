#[tokio::test(flavor = "current_thread")]
async fn test_process_lifecycle() {
    use dusk_program_ps::Args as PsArgs;
    use dusk_shell::connection::Connection;
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

            let ps_program_args = PsArgs::new().as_program_args().unwrap();

            // Dusk.process — create the process
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

            // Dusk.run — start executing it
            let mut run_request = client.run_request();
            run_request.get().set_process(process.clone());
            run_request.send().promise.await.unwrap();

            // Get PID
            let pid = process
                .pid_request()
                .send()
                .promise
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_result();

            // Dusk.kill — send SIGTERM (15)
            let mut kill_request = client.kill_request();
            kill_request.get().set_pid(pid);
            kill_request.get().set_signal(15);
            kill_request.send().promise.await.unwrap();

            // Dusk.waitpid — must block until the process has fully exited
            let mut waitpid_request = client.waitpid_request();
            waitpid_request.get().set_pid(pid);
            waitpid_request.send().promise.await.unwrap();

            // After waitpid returns the process must be gone from ps
            let ps_reply = client.ps_request().send().promise.await.unwrap();
            let entries = ps_reply.get().unwrap().get_process_entries().unwrap();
            let pids: Vec<u64> = entries.iter().map(|e| e.get_pid()).collect();
            assert!(
                !pids.contains(&pid),
                "pid {pid} should not be in ps after waitpid"
            );

            connection.disconnect().await.unwrap();
        })
        .await;
}
