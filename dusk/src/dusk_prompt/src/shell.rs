use std::process;

use anyhow::{anyhow, Result};
use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::exec_result;
use dusk_capnp::dusk_capnp::portal_result;
use dusk_program_sh::args::ShArgs;
use dusk_program_sh::sh_capnp::sh_portal;
use tracing::info;

pub struct Shell {
    sh_portal: sh_portal::Client,
    pub hostname: String,
    pub available_programs: Vec<String>,
}

impl Shell {
    async fn get_sh_portal(client: dusk::Client) -> Result<sh_portal::Client> {
        let mut exec_request = client.exec_request();
        exec_request
            .get()
            .set_program_args(capnp_rpc::new_client(ShArgs {}));
        let exec_reply = exec_request.send().promise.await?;
        let exec_result = exec_reply.get()?.get_result()?;

        let process = match exec_result.which()? {
            exec_result::Which::Process(process) => Ok(process),
            exec_result::Which::ProgramNotFound(()) => {
                Err(anyhow!("couldn't find the `sh` program"))
            }
            exec_result::Which::ProgramLaunchFailed(()) => {
                Err(anyhow!("failed to launch the `sh` program"))
            }
        }??;

        let mut portal_request = client.portal_request();
        portal_request.get().set_process(process)?;
        let portal_reply = portal_request.send().promise.await?;
        let portal_result = portal_reply.get()?.get_result()?;

        let sh_portal = match portal_result.which()? {
            portal_result::Which::Portal(portal) => Ok(portal),
            portal_result::Which::ProcessNotFound(()) => Err(anyhow!(
                "sh process not found, pid `{0}`",
                process.get_pid()
            )),
        }??
        .cast_to::<sh_portal::Client>();

        Ok(sh_portal)
    }

    pub async fn new(client: dusk::Client) -> Result<Self> {
        let mut available_programs = vec![];
        let mut builtins = vec!["clear".into(), "exit".into()];
        available_programs.append(&mut builtins);

        let hostname_reply = client.hostname_request().send().promise.await?;
        let hostname = hostname_reply.get()?.get_hostname()?.to_str()?;

        Ok(Shell {
            sh_portal: Self::get_sh_portal(client).await?,
            hostname: hostname.into(),
            available_programs,
        })
    }

    pub async fn process_command(&mut self, _command: &str) {
        info!("run");
    }
}
