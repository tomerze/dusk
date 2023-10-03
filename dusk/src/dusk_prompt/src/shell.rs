use anyhow::Result;
use dusk_capnp::dusk_capnp::dusk::Client;
use tracing::info;

pub struct Shell {
    // In the future this will be a client for the shell
    // program and not dusk itself
    shell_client: Client,
    pub hostname: String,
    pub available_programs: Vec<String>,
}

impl Shell {
    pub async fn new(client: Client) -> Result<Self> {
        let mut available_programs = vec![];
        let mut builtins = vec!["clear".into(), "exit".into()];
        available_programs.append(&mut builtins);

        let hostname_reply = client.hostname_request().send().promise.await?;
        let hostname = hostname_reply.get()?.get_hostname()?.to_str()?;

        Ok(Shell {
            shell_client: client,
            hostname: hostname.into(),
            available_programs,
        })
    }

    pub async fn process_command(&mut self, _command: &str) {
        info!("run");
    }
}
