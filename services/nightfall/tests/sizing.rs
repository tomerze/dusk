#[allow(dead_code)]
mod support;

use capnp::capability::Promise;
use capnp::message::ReaderOptions;
use capnp_rpc::rpc_twoparty_capnp::Side;
use capnp_rpc::{RpcSystem, twoparty};
use dusk_capnp::dusk_capnp::dusk;
use rustls_pki_types::ServerName;
use std::time::{Duration, Instant};
use support::harness::{Environment, FLEET_NAME, run, shutdown, wait_until};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

const SESSIONS: u64 = 2000;

#[derive(Clone)]
struct IdleNode {
    namespace_id: u64,
}

impl dusk::Server for IdleNode {
    fn namespace_id(
        &mut self,
        _params: dusk::NamespaceIdParams,
        mut results: dusk::NamespaceIdResults,
    ) -> Promise<(), capnp::Error> {
        results.get().set_result(self.namespace_id);
        Promise::ok(())
    }

    fn programs(
        &mut self,
        _params: dusk::ProgramsParams,
        mut results: dusk::ProgramsResults,
    ) -> Promise<(), capnp::Error> {
        results.get().init_program_entries(0);
        Promise::ok(())
    }

    fn time(
        &mut self,
        _params: dusk::TimeParams,
        mut results: dusk::TimeResults,
    ) -> Promise<(), capnp::Error> {
        results.get().set_unix_time_ms(1);
        Promise::ok(())
    }

    fn dusk(
        &mut self,
        _params: dusk::DuskParams,
        mut results: dusk::DuskResults,
    ) -> Promise<(), capnp::Error> {
        results
            .get()
            .set_result(capnp_rpc::new_client(self.clone()));
        Promise::ok(())
    }
}

fn resident_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
    let pages: u64 = statm.split_whitespace().nth(1).unwrap().parse().unwrap();
    pages * 4096
}

#[test]
#[ignore = "a measurement: run it alone with --ignored and read its output"]
fn measures_the_memory_of_idle_node_sessions() {
    let environment = Environment::new();
    let mut config = environment.config("nightfall-0", 0);
    config.limits.insert(
        "handshakes_per_second".to_string(),
        toml::Value::Integer(100_000),
    );
    let (instance, _log) = environment.start(config);
    let identities: Vec<_> = (0..SESSIONS)
        .map(|index| {
            environment.node_identity(&format!("{index:032x}"), "ffeeddccbbaa99887766554433221100")
        })
        .collect();
    std::thread::sleep(Duration::from_secs(1));
    let before = resident_bytes();
    let started = Instant::now();
    run(async {
        let mut links = Vec::new();
        for (index, identity) in identities.into_iter().enumerate() {
            let tcp = tokio::net::TcpStream::connect(instance.addresses.fleet)
                .await
                .unwrap();
            let tls = tokio_rustls::TlsConnector::from(identity)
                .connect(ServerName::try_from(FLEET_NAME).unwrap(), tcp)
                .await
                .unwrap();
            let (reader, writer) = tokio::io::split(tls);
            let network = twoparty::VatNetwork::new(
                reader.compat(),
                writer.compat_write(),
                Side::Server,
                ReaderOptions::new(),
            );
            let node: dusk::Client = capnp_rpc::new_client(IdleNode {
                namespace_id: 0x1000_0000 + index as u64,
            });
            links.push(tokio::task::spawn_local(RpcSystem::new(
                Box::new(network),
                Some(node.client),
            )));
        }
        wait_until("every session", Duration::from_secs(120), || {
            instance.shared.directory.lock().unwrap().local_count() == SESSIONS as usize
        })
        .await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let after = resident_bytes();
        println!(
            "{SESSIONS} idle node sessions in {:.1} s: resident memory {} MiB -> {} MiB, {} KiB per session with both ends of each link in this process",
            started.elapsed().as_secs_f64(),
            before / (1024 * 1024),
            after / (1024 * 1024),
            (after.saturating_sub(before)) / SESSIONS / 1024
        );
        for link in links {
            link.abort();
        }
    });
    shutdown(instance);
}
