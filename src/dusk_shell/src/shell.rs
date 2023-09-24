use anyhow::Result;
use dusk::dusk_capnp;
use tracing::info;

pub struct Shell {
    client: dusk_capnp::dusk::Client,
    pub hostname: String,
    pub available_programs: Vec<String>,
}

impl Shell {
    pub async fn new(client: dusk_capnp::dusk::Client) -> Result<Self> {
        let mut available_programs = vec![];
        let mut builtins = vec!["clear".into(), "exit".into()];
        available_programs.append(&mut builtins);

        let reply = client.hostname_request().send().promise.await?;
        let hostname = reply.get()?.get_hostname()?.to_str()?;

        Ok(Shell {
            client,
            hostname: hostname.into(),
            available_programs,
        })
    }

    pub async fn process_command(&mut self, _command: &str) {
        info!("run");
    }
}
