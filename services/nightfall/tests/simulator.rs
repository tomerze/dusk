#[allow(dead_code)]
mod support;

use nightfall_simulator::fleet::Fleet;
use nightfall_simulator::identity::{Issuer, device_id, installation_id};
use nightfall_simulator::link::{Ended, Settings, serve};
use nightfall_simulator::node::Node;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
use rustls_pki_types::{CertificateDer, ServerName};
use std::sync::Arc;
use std::time::Duration;
use support::harness::{Environment, FLEET_NAME, run, shutdown, wait_until};

const SIMULATOR: u64 = 3;
const LINKS: u64 = 3;

struct SimulatorCa {
    certificate: CertificateDer<'static>,
    pem: String,
    key_pem: String,
}

fn simulator_ca() -> SimulatorCa {
    let mut parameters = CertificateParams::new(Vec::<String>::new()).unwrap();
    parameters
        .distinguished_name
        .push(DnType::CommonName, "simulator");
    parameters.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    parameters.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let certificate = parameters.self_signed(&key).unwrap();
    SimulatorCa {
        certificate: certificate.der().clone(),
        pem: certificate.pem(),
        key_pem: key.serialize_pem(),
    }
}

fn issuer(simulator: &SimulatorCa, fleet_server: CertificateDer<'static>) -> Issuer {
    Issuer::new(
        &simulator.pem,
        &simulator.key_pem,
        vec![fleet_server],
        Duration::from_secs(3600),
        Some("acme".to_string()),
    )
    .unwrap()
}

#[test]
fn a_simulated_certificate_carries_the_identity_nightfall_reads() {
    let simulator = simulator_ca();
    let device = device_id(SIMULATOR, 9);
    let installation = installation_id(&device);
    let minted = issuer(&simulator, simulator.certificate.clone())
        .certificate(&device, &installation)
        .unwrap();
    assert_eq!(minted.chain[1], simulator.certificate);
    let node = nightfall::peer::node_certificate(&minted.chain[0]).unwrap();
    assert_eq!(node.device_id, device);
    assert_eq!(node.installation_id, installation);
    assert_eq!(node.tenant.as_deref(), Some("acme"));
}

#[test]
fn nightfall_admits_links_signed_by_a_ca_in_its_fleet_client_bundle_and_no_other() {
    let environment = Environment::new();
    let simulator = simulator_ca();
    let stranger = simulator_ca();
    let mut config = environment.config("nightfall-0", 0);
    let bundle = environment
        .directory
        .path()
        .join("fleet-client-and-simulator-ca.crt");
    std::fs::write(
        &bundle,
        std::fs::read_to_string(&config.fleet.client_ca).unwrap() + &simulator.pem,
    )
    .unwrap();
    config.fleet.client_ca = bundle;
    let (instance, _log) = environment.start(config);
    let fleet = Fleet::new(instance.addresses.fleet.to_string());
    let settings = |ca: &SimulatorCa| {
        Arc::new(Settings {
            fleet: fleet.clone(),
            server_name: ServerName::try_from(FLEET_NAME).unwrap(),
            issuer: issuer(ca, environment.pki.fleet_server.certificate.clone()),
            simulator: SIMULATOR,
            heartbeat_timeout: Duration::from_secs(90),
        })
    };
    let trusted = settings(&simulator);
    let untrusted = settings(&stranger);
    run(async {
        fleet.resolve().await;
        let started = tokio::time::Instant::now();
        let links: Vec<_> = (0..LINKS)
            .map(|link| {
                tokio::task::spawn_local(nightfall_simulator::link::run(
                    trusted.clone(),
                    link,
                    started,
                ))
            })
            .collect();
        wait_until("every simulated link", Duration::from_secs(30), || {
            instance.shared.directory.lock().unwrap().local_count() == LINKS as usize
        })
        .await;
        let mut connected: Vec<(String, String, String)> = environment
            .connections()
            .iter()
            .filter(|event| event["event"] == "connected")
            .map(|event| {
                let text = |field: &str| event[field].as_str().map(str::to_string);
                (
                    text("device_id").unwrap(),
                    text("installation_id").unwrap(),
                    text("tenant").unwrap_or_default(),
                )
            })
            .collect();
        connected.sort();
        let mut expected: Vec<(String, String, String)> = (0..LINKS)
            .map(|link| {
                let device = device_id(SIMULATOR, link);
                let installation = installation_id(&device);
                (device, installation, "acme".to_string())
            })
            .collect();
        expected.sort();
        assert_eq!(connected, expected);

        let device = device_id(SIMULATOR + 1, 0);
        let identity = untrusted
            .issuer
            .mint(&device, &installation_id(&device))
            .unwrap();
        let node = Node::new(1, device);
        let served = serve(&untrusted, &node, &identity, 0).await;
        assert!(!matches!(served, Ok((Ended::Silent, _))), "{served:?}");
        assert_eq!(
            instance.shared.directory.lock().unwrap().local_count(),
            LINKS as usize
        );
        assert_eq!(environment.connections().len(), LINKS as usize);
        for link in links {
            link.abort();
        }
    });
    shutdown(instance);
}
