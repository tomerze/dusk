use anyhow::Result;
use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::dusk;
use dusk_program_sh::args::ShArgs;
use dusk_program_sh::sh_capnp::sh_portal;
use tracing::{debug, info};

pub struct Shell {
    _sh_portal: sh_portal::Client,
    pub hostname: String,
    pub available_programs: Vec<String>,
}

impl Shell {
    async fn get_sh_portal(client: dusk::Client) -> Result<sh_portal::Client> {
        let mut process_request = client.process_request();
        process_request
            .get()
            .set_program_args(capnp_rpc::new_client(ShArgs {}));
        let process_reply = process_request.send().promise.await?;
        let process = process_reply.get()?.get_result()?;

        let pid_reply = process.pid_request().send().promise.await?;
        let pid: u64 = pid_reply.get()?.get_result();

        let mut run_request = client.run_request();
        run_request.get().set_process(process);
        let _run_reply = run_request.send().promise.await?;

        debug!("sh started with pid {}", pid);

        let mut portal_request = client.portal_request();
        portal_request.get().set_pid(pid);
        let portal_reply = portal_request.send().promise.await?;

        Ok(portal_reply
            .get()?
            .get_result()?
            .cast_to::<sh_portal::Client>())
    }

    pub async fn new(client: dusk::Client) -> Result<Self> {
        let mut available_programs = vec![];
        let mut builtins = vec!["clear".into(), "exit".into()];
        available_programs.append(&mut builtins);

        let hostname_reply = client.hostname_request().send().promise.await?;
        let hostname = hostname_reply.get()?.get_result()?.to_str()?;

        let _sh_portal = Self::get_sh_portal(client).await?;
        Ok(Shell {
            _sh_portal,
            hostname: hostname.into(),
            available_programs,
        })
    }

    pub async fn process_command(&mut self, _command: &str) {
        info!("run");
    }
}
