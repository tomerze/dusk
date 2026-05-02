use anyhow::Result;
use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::stream;
use dusk_capnp::dusk_capnp::{dusk, process};
use dusk_program_sh::entry::ShEntriesBuilder;
use dusk_program_sh::sh_capnp::{sh_args, sh_portal};
use dusk_program_sh::{ShArgs, parser::Parser};
use tokio::sync::oneshot;

pub struct Shell {
    client: dusk::Client,
    parser: Parser,
    sh_process: process::Client,
    pub hostname: String,
    pub sh_pid: u64,
}

impl Shell {
    async fn create_sh_process_reconnect_callback<S: ShEntriesBuilder>(
        client: dusk::Client,
        sh_entries_builder: S,
    ) -> capnp::Result<process::Client> {
        let mut options = capnp::message::Builder::new_default();
        options
            .init_root::<dusk_program_sh::sh_capnp::sh_options::Builder>()
            .set_server(());
        let program_args = capnp_rpc::new_client::<sh_args::Client, ShArgs<S>>(ShArgs {
            client: client.clone(),
            options,
            sh_entries_builder,
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

    async fn create_sh_process<S: ShEntriesBuilder>(
        client: dusk::Client,
        sh_entries_builder: S,
    ) -> Result<process::Client> {
        let (process, _) = capnp_rpc::auto_reconnect(move || {
            Ok(capnp_rpc::new_future_client(
                Self::create_sh_process_reconnect_callback(
                    client.clone(),
                    sh_entries_builder.clone(),
                ),
            ))
        })?;

        Ok(process)
    }

    pub async fn new<S: ShEntriesBuilder>(
        client: dusk::Client,
        sh_entries_builder: S,
        parser: Parser,
    ) -> Result<Self> {
        let hostname_reply = client.hostname_request().send().promise.await?;
        let hostname = hostname_reply.get()?.get_result()?.to_str()?;

        let sh_process = Self::create_sh_process(client.clone(), sh_entries_builder).await?;

        let pid_reply = sh_process.pid_request().send().promise.await?;
        let sh_pid = pid_reply.get()?.get_result();
        Ok(Shell {
            client: client.clone(),
            parser,
            sh_process,
            hostname: hostname.into(),
            sh_pid,
        })
    }

    pub async fn sh(
        &mut self,
        script: &str,
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
        let script_builder = sh_request.get().init_script();
        self.parser.parse(script, script_builder)?;

        sh_request.get().set_output(stream);
        let _sh_reply = sh_request.send().promise.await?;
        // sh returns immediately, but the shell command is running until done is called on the output stream.
        done_receiver.await?;
        Ok(())
    }

    /// Returns the names of functions currently defined in the sh process.
    pub async fn functions(&self) -> Result<Vec<String>> {
        let sh_process = self.sh_process.clone();
        let sh_portal = capnp_rpc::new_future_client(async move {
            let portal_reply = sh_process.portal_request().send().promise.await?;
            Ok(portal_reply
                .get()?
                .get_result()?
                .cast_to::<sh_portal::Client>())
        });
        let reply = sh_portal.functions_request().send().promise.await?;
        let symbols = reply.get()?.get_symbols()?;
        let mut out = Vec::with_capacity(symbols.len() as usize);
        for symbol in symbols.iter() {
            out.push(symbol?.to_str()?.to_string());
        }
        Ok(out)
    }

    /// Kill the shell process, must be called to clean up resources.
    /// Isn't in Drop to allow async cleanup.
    pub async fn kill(self) -> Result<()> {
        let client = self.client.clone();
        let sh_process = self.sh_process.clone();
        let pid = sh_process
            .pid_request()
            .send()
            .promise
            .await?
            .get()?
            .get_result();
        let mut kill_request = client.kill_request();
        kill_request.get().set_pid(pid);
        kill_request.get().set_signal(15); // SIGTERM

        let _ = kill_request.send().promise.await?;

        Ok(())
    }
}
