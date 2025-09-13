use anyhow::Result;
use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::{dusk, process};
use dusk_program_sh::sh_capnp::sh_portal;
use dusk_program_sh::ShArgs;
use tracing::debug;

pub struct Shell {
    pub client: dusk::Client,
    pub sh_process: process::Client,
    pub hostname: String,
    pub available_programs: Vec<String>,
}

impl Shell {
    async fn get_sh_process(client: dusk::Client) -> Result<process::Client> {
        let mut process_request = client.process_request();
        process_request
            .get()
            .set_program_args(capnp_rpc::new_client(ShArgs {}));
        let process_reply = process_request.send().promise.await?;
        let process = process_reply.get()?.get_result()?;

        let pid_reply = process.pid_request().send().promise.await?;
        let pid: u64 = pid_reply.get()?.get_result();

        let mut run_request = client.run_request();
        run_request.get().set_process(process.clone());
        let _run_reply = run_request.send().promise.await?;

        debug!("sh started with pid {}", pid);
        Ok(process)
    }

    pub async fn new(client: dusk::Client) -> Result<Self> {
        let mut available_programs = vec![];
        let mut builtins = vec!["clear".into(), "exit".into()];
        available_programs.append(&mut builtins);

        let hostname_reply = client.hostname_request().send().promise.await?;
        let hostname = hostname_reply.get()?.get_result()?.to_str()?;

        let sh_process = Self::get_sh_process(client.clone()).await?;
        Ok(Shell {
            client,
            sh_process,
            hostname: hostname.into(),
            available_programs,
        })
    }

    pub async fn process_command(&mut self, _command: &str) -> Result<()> {
        let portal_request = self.sh_process.portal_request();
        let portal_reply = portal_request.send().promise.await?;

        let _sh_portal = portal_reply
            .get()?
            .get_result()?
            .cast_to::<sh_portal::Client>();

        Ok(())
    }

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
