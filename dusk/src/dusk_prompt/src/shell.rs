use anyhow::Result;
use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::dusk;
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
        let process = exec_reply.get()?.get_result()?;

        let mut portal_request = client.portal_request();
        portal_request.get().set_process(process);
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
