use anyhow::Result;
use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::stream;
use dusk_capnp::dusk_capnp::{dusk, process};
use dusk_program_sh::ShArgs;
use dusk_program_sh::sh_capnp::{engine, sh_args, sh_portal};
use tokio::sync::oneshot;
use tracing::debug;

pub struct Shell {
    client: dusk::Client,
    sh_process: process::Client,
    pub hostname: String,
}

impl Shell {
    async fn create_sh_process_reconnect_callback(
        client: dusk::Client,
        engine: engine::Client,
    ) -> capnp::Result<process::Client> {
        let program_args = capnp_rpc::new_client::<sh_args::Client, ShArgs>(ShArgs {
            engine: engine.clone(),
        });
        let mut process_request = client.process_request();
        process_request.get().set_program_args(
            program_args.cast_to::<dusk_capnp::dusk_capnp::program_args::Client>(),
        );
        let process_reply = process_request.send().promise.await?;
        let process = process_reply.get()?.get_result()?;

        let mut run_request = client.run_request();
        run_request.get().set_process(process.clone());
        let _run_reply = run_request.send().promise.await?;
        Ok(process)
    }

    async fn create_sh_process(
        client: dusk::Client,
        engine: engine::Client,
    ) -> Result<process::Client> {
        let (process, _) = capnp_rpc::auto_reconnect(move || {
            Ok(capnp_rpc::new_future_client(
                Self::create_sh_process_reconnect_callback(client.clone(), engine.clone()),
            ))
        })?;

        let pid_reply = process.pid_request().send().promise.await?;
        let pid: u64 = pid_reply.get()?.get_result();

        debug!("sh started with pid {}", pid);
        Ok(process)
    }

    pub async fn new(engine: impl engine::Server + 'static) -> Result<Self> {
        let engine: engine::Client = capnp_rpc::new_client(engine);
        let client_reply = engine.client_request().send().promise.await?;
        let client = client_reply.get()?.get_client()?;

        let hostname_reply = client.hostname_request().send().promise.await?;
        let hostname = hostname_reply.get()?.get_result()?.to_str()?;

        let sh_process = Self::create_sh_process(client.clone(), engine).await?;

        Ok(Shell {
            client,
            sh_process,
            hostname: hostname.into(),
        })
    }

    pub async fn sh(
        &mut self,
        command: &str,
        stream: stream::Client,
        done_receiver: oneshot::Receiver<()>,
    ) -> Result<()> {
        let sh_process = self.sh_process.clone();

        let sh_portal = capnp_rpc::new_future_client(async move {
            let portal_request = sh_process.portal_request();
            let portal_reply = portal_request.send().promise.await?;
            Ok(portal_reply
                .get()?
                .get_result()?
                .cast_to::<sh_portal::Client>())
        });

        let mut sh_request = sh_portal.sh_request();
        sh_request.get().set_command(command);
        sh_request.get().set_output(stream);
        let _sh_reply = sh_request.send().promise.await?;
        // sh returns immediately, but the shell command is running until done is called on the output stream.
        done_receiver.await?;
        Ok(())
    }

    /// Kill the shell process, must be called to clean up resources.
    /// Isn't in Drop to allow async cleanup.
    pub async fn kill(self) -> Result<()> {
        let client = self.client.clone();
        let sh_process = self.sh_process.clone();
        let mut kill_request = client.kill_request();
        kill_request.get().set_process(sh_process.clone());
        kill_request.get().set_signal(15); // SIGTERM

        let _ = kill_request.send().promise.await?;

        Ok(())
    }
}
