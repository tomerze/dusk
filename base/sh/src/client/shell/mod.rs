use crate::entry::ShEntriesBuilder;
use crate::sh_capnp::{sh_portal, sh_stop};
use crate::{ShArgs, ShMode, parser::Parser};
use anyhow::Result;
use capnp::capability::{FromClientHook, Promise};
use dusk_capnp::dusk_capnp::stream;
use dusk_capnp::dusk_capnp::{dusk, process};
use std::format;
use std::prelude::rust_2024::*;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

struct Stop {
    notify: Rc<Notify>,
}

impl sh_stop::Server for Stop {
    fn stop(
        &mut self,
        _: sh_stop::StopParams,
        _: sh_stop::StopResults,
    ) -> Promise<(), capnp::Error> {
        let notify = self.notify.clone();
        Promise::from_future(async move {
            notify.notified().await;
            Ok(())
        })
    }
}

pub type RttHandle = Arc<Mutex<Option<Duration>>>;

pub struct Shell {
    client: dusk::Client,
    parser: Parser,
    keepalive_task: JoinHandle<()>,
    sh_process: process::Client,
    pub rtt_handle: RttHandle,
    pub hostname: String,
    pub sh_pid: u64,
}

impl Shell {
    async fn create_sh_process_reconnect_callback<S: ShEntriesBuilder>(
        client: dusk::Client,
        sh_entries_builder: S,
    ) -> capnp::Result<process::Client> {
        let sh_args = ShArgs::new(client.clone(), sh_entries_builder, ShMode::Server)
            .map_err(|err| capnp::Error::failed(format!("{err:?}")))?;
        let program_args = sh_args.as_program_args()?;

        // capnp auto_reconnect returns the first call's Disconnected error while
        // refreshing its current capability in the background; the next call uses
        // the refreshed client. Retry once so the outer sh_process auto_reconnect
        // resolves its placeholder on first poll instead of leaving a hidden
        // strike for the next caller.
        let mut attempts_left = 2;
        loop {
            let mut process_request = client.process_request();
            program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
            match process_request.send().promise.await {
                Ok(process_reply) => {
                    let process = process_reply.get()?.get_result()?;
                    let mut run_request = client.run_request();
                    run_request.get().set_process(process.clone());
                    let _run_reply = run_request.send().promise.await?;
                    return Ok(process);
                }
                Err(err) if err.kind == capnp::ErrorKind::Disconnected && attempts_left > 1 => {
                    attempts_left -= 1;
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
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

    fn spawn_keepalive_task(sh_process: process::Client, rtt_handle: RttHandle) -> JoinHandle<()> {
        const MIN_INTERVAL: Duration = Duration::from_millis(50);
        const MAX_INTERVAL: Duration = Duration::from_secs(10);
        tokio::task::spawn_local(async move {
            loop {
                let start = Instant::now();
                let result = sh_process.pid_request().send().promise.await;
                let elapsed = start.elapsed();
                let sleep_for = match result {
                    Ok(_) => {
                        *rtt_handle.lock().unwrap() = Some(elapsed);
                        (elapsed / 2).clamp(MIN_INTERVAL, MAX_INTERVAL)
                    }
                    Err(_) => {
                        *rtt_handle.lock().unwrap() = None;
                        MIN_INTERVAL
                    }
                };
                tokio::time::sleep(sleep_for).await;
            }
        })
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

        let rtt_handle: RttHandle = Arc::new(Mutex::new(None));
        let keepalive_task = Self::spawn_keepalive_task(sh_process.clone(), rtt_handle.clone());

        Ok(Shell {
            client: client.clone(),
            parser,
            sh_process,
            hostname: hostname.into(),
            sh_pid,
            rtt_handle,
            keepalive_task,
        })
    }

    pub async fn sh(
        &mut self,
        script: &str,
        stream: stream::Client,
        done_receiver: oneshot::Receiver<()>,
        stop_signal: Rc<Notify>,
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

        let stop_cap: sh_stop::Client = capnp_rpc::new_client(Stop {
            notify: stop_signal,
        });

        let mut sh_request = sh_portal.sh_request();
        let script_builder = sh_request.get().init_script();
        self.parser.parse(script, script_builder)?;
        sh_request.get().set_output(stream);
        sh_request.get().set_stop(stop_cap);

        sh_request.send().promise.await?;
        let _ = done_receiver.await;
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
        self.keepalive_task.abort();
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
