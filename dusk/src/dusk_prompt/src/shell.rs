use anyhow::{anyhow, Result};
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::exec_result;
use dusk_program_sh::args::ShArgs;
use dusk_program_sh::sh_capnp::sh_portal;
use tracing::info;

pub struct Shell {
    // In the future this will be a client for the shell
    // program and not dusk itself
    sh_portal: sh_portal::Client,
    pub hostname: String,
    pub available_programs: Vec<String>,
}

impl Shell {
    pub async fn new(client: dusk::Client) -> Result<Self> {
        let mut available_programs = vec![];
        let mut builtins = vec!["clear".into(), "exit".into()];
        available_programs.append(&mut builtins);

        let hostname_reply = client.hostname_request().send().promise.await?;
        let hostname = hostname_reply.get()?.get_hostname()?.to_str()?;

        let exec_request = client.exec_request();
        exec_request
            .get()
            .set_program_args(capnp_rpc::new_client(ShArgs {}));

        let exec_result = exec_request.send().promise.await?.get()?.get_result()?;
        let pid = match exec_result.which()? {
            exec_result::Which::Pid(pid) => Ok(pid),
            exec_result::Which::ProgramNotFound(()) => {
                Err(anyhow!("couldn't find the `sh` program"))
            }
            exec_result::Which::ProgramLaunchFailed(()) => {
                Err(anyhow!("failed to launch the `sh` program"))
            }
        }?;
        let portal_request = client.portal_request();
        portal_request.get().set_pid(pid);
        let sh_portal = portal_request.send().promise.await?.get()?.get_portal()?;

        Ok(Shell {
            sh_portal: sh_portal,
            hostname: hostname.into(),
            available_programs,
        })
    }

    pub async fn process_command(&mut self, _command: &str) {
        info!("run");
    }
}
