use super::resp;
use crate::kvs_capnp::kvs_portal;
use crate::{Args, PROGRAM_ID};
use capnp::capability::{FromClientHook as _, Promise};
use dusk_capnp::pry;
use dusk_program::dusk_capnp::dusk_capnp::{created, dusk, process};
use dusk_program_sh::client::{SWEEP, kill};
use std::cell::RefCell;
use std::format;
use std::net::SocketAddr;
use std::rc::Rc;
use std::vec::Vec;
use tokio::net::TcpListener;

pub(super) struct Bound {
    pub(super) client: dusk::Client,
    pub(super) address: SocketAddr,
    pub(super) server_pid: u64,
    pub(super) flags: u8,
}

impl created::Server for Bound {
    fn created(
        &mut self,
        params: created::CreatedParams,
        _results: created::CreatedResults,
    ) -> Promise<(), capnp::Error> {
        let process = pry!(pry!(params.get()).get_process());
        let client = self.client.clone();
        let address = self.address;
        let server_pid = self.server_pid;
        let flags = self.flags;
        Promise::from_future(async move {
            let listener = match TcpListener::bind(address).await {
                Ok(listener) => listener,
                Err(error) => {
                    tokio::task::spawn_local(async move {
                        if let Err(error) = kill(&client, &process, SWEEP).await {
                            tracing::error!(%error, "couldn't clear the refused bind's process");
                        }
                    });
                    return Err(capnp::Error::failed(format!("bind {address}: {error}")));
                }
            };
            tracing::info!(%address, "kvs listening for RESP connections");
            let connections = Rc::new(RefCell::new(Vec::new()));
            let accepting = tokio::task::spawn_local(serve(
                listener,
                client,
                server_pid,
                flags,
                connections.clone(),
            ));
            tokio::task::spawn_local(async move {
                let ended = async {
                    process.portal_request().send().promise.await?;
                    process.run_request().send().promise.await?;
                    Ok::<(), capnp::Error>(())
                }
                .await;
                accepting.abort();
                for serving in connections.borrow_mut().drain(..) {
                    serving.abort();
                }
                match ended {
                    Ok(()) => {
                        tracing::info!(%address, "kvs stopped listening for RESP connections")
                    }
                    Err(error) => tracing::warn!(
                        %address,
                        %error,
                        "kvs stopped listening for RESP connections"
                    ),
                }
            });
            Ok(())
        })
    }
}

async fn serve(
    listener: TcpListener,
    client: dusk::Client,
    server_pid: u64,
    flags: u8,
    connections: resp::Connections,
) {
    let names = resp::Names::default();
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionAborted => {
                tracing::debug!(%error, "kvs resp connection aborted before it was accepted");
                continue;
            }
            Err(error) => {
                tracing::warn!(%error, "kvs resp couldn't accept a connection; trying again in a second");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };
        tracing::info!(%peer, "kvs resp connection opened");
        let client = client.clone();
        let names = names.clone();
        let serving = tokio::task::spawn_local(async move {
            let portal = server_portal(&client, server_pid).await;
            match resp::connection(stream, portal, flags, &names).await {
                Ok(()) => tracing::info!(%peer, "kvs resp connection closed"),
                Err(error) => tracing::warn!(
                    %peer,
                    error = %format!("{error:#}"),
                    "kvs resp connection failed"
                ),
            }
        });
        let mut connections = connections.borrow_mut();
        connections.retain(|serving| !serving.is_finished());
        connections.push(serving.abort_handle());
    }
}

async fn server_portal(client: &dusk::Client, pid: u64) -> anyhow::Result<kvs_portal::Client> {
    let program_args = Args::server().as_program_args()?;
    program_args.set_pid(Some(pid))?;
    let mut attempts_left = 2;
    loop {
        let attempt = async {
            let mut process_request = client.process_request();
            program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
            let server: process::Client =
                process_request.send().promise.await?.get()?.get_result()?;
            let program_id = server
                .program_id_request()
                .send()
                .promise
                .await?
                .get()?
                .get_result();
            if program_id != PROGRAM_ID {
                return Ok(None);
            }
            let mut run_request = client.run_request();
            run_request.get().set_process(server.clone());
            run_request.send().promise.await?;
            let portal = server.portal_request().send().promise.await?;
            Ok::<_, capnp::Error>(Some(
                portal.get()?.get_result()?.cast_to::<kvs_portal::Client>(),
            ))
        }
        .await;
        match attempt {
            Ok(Some(portal)) => return Ok(portal),
            Ok(None) => anyhow::bail!("pid {pid:#018x} runs another program, not kvs"),
            Err(error) if error.kind == capnp::ErrorKind::Disconnected && attempts_left > 1 => {
                attempts_left -= 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
}
