#[tokio::test(flavor = "current_thread")]
async fn test_process_lifecycle() {
    use dusk_capnp::capnp::capability::FromClientHook;
    use dusk_capnp::{capnp_rpc, dusk_capnp::program_args};
    use dusk_shell::connection::Connection;
    use dusk_tests::{DuskNixImpl, LISTEN_ADDR, gen_port};

    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDR, port);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let addr: std::net::SocketAddr = format!("{}:{}", LISTEN_ADDR, port).parse().unwrap();
            let connection = Connection::connect(addr).await.unwrap();
            let client = connection.client().await;

            let ps_args: dusk_program_ps::ps_capnp::ps_args::Client =
                capnp_rpc::new_client(dusk_program_ps::Args {
                    client: client.clone(),
                });
            let ps_args = ps_args.cast_to::<program_args::Client>();

            // Dusk.process — create the process
            let mut process_req = client.process_request();
            process_req.get().set_program_args(ps_args);
            let process = process_req
                .send()
                .promise
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_result()
                .unwrap();

            // Dusk.run — start executing it
            let mut run_req = client.run_request();
            run_req.get().set_process(process.clone());
            run_req.send().promise.await.unwrap();

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
            let mut kill_req = client.kill_request();
            kill_req.get().set_pid(pid);
            kill_req.get().set_signal(15);
            kill_req.send().promise.await.unwrap();

            // Dusk.waitpid — must block until the process has fully exited
            let mut waitpid_req = client.waitpid_request();
            waitpid_req.get().set_pid(pid);
            waitpid_req.send().promise.await.unwrap();

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
