#[path = "support/fake_nightfall.rs"]
mod fake_nightfall;

use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use async_io::Async;
use dusk_capnp::capnp_rpc::{RpcSystem, rpc_twoparty_capnp::Side, twoparty};
use dusk_capnp::dusk_capnp::dusk;
use dusk_program_nightfall::connect::ConnectArgs;
use fake_nightfall::*;
use futures::AsyncReadExt as _;

#[test]
fn a_server_the_ca_file_does_not_sign_is_never_trusted() {
    let mut fleet = Fleet::start("foreign-ca", Behaviour::default());
    let (_, foreign_authority) = authority("a CA the fleet does not use");
    std::fs::write(
        fleet.directory.join("fleet-server-ca.pem"),
        foreign_authority.pem(),
    )
    .unwrap();
    let _node = fleet.start_node("");
    fleet.expect(
        FIRST_LINK_TIMEOUT,
        "a refused provisioning handshake",
        |event| {
            matches!(
                event,
                Event::HandshakeFailed {
                    listener: "provision"
                } | Event::AssignAttempted
            )
        },
    );
    fleet.quiet(
        Duration::from_secs(12),
        "a call to a server the node should not trust",
        |event| {
            matches!(
                event,
                Event::AssignAttempted | Event::Enrolled { .. } | Event::Linked { .. }
            )
        },
    );
    assert!(
        fleet
            .seen
            .iter()
            .all(|event| !matches!(event, Event::AssignAttempted)),
        "{:#?}",
        fleet.seen
    );
}

#[test]
fn a_fleet_that_offers_only_tls_1_2_is_never_linked_to() {
    let mut fleet = Fleet::start(
        "tls12",
        Behaviour {
            fleet_offers_only_tls12: true,
            ..Behaviour::default()
        },
    );
    let _node = fleet.start_node("");
    fleet.expect(FIRST_LINK_TIMEOUT, "enrollment", |event| {
        matches!(event, Event::Enrolled { .. })
    });
    fleet.expect(
        Duration::from_secs(15),
        "a refused fleet handshake",
        |event| matches!(event, Event::HandshakeFailed { listener: "fleet" }),
    );
    fleet.quiet(Duration::from_secs(12), "a link over TLS 1.2", |event| {
        matches!(event, Event::Linked { .. })
    });
}

#[test]
fn a_fleet_that_never_completes_the_handshake_is_given_up_on_after_10_seconds() {
    let mut fleet = Fleet::start(
        "handshake-timeout",
        Behaviour {
            links: vec![LinkMode::NoHandshake, LinkMode::NoHandshake],
            ..Behaviour::default()
        },
    );
    let _node = fleet.start_node("");
    fleet.expect(FIRST_LINK_TIMEOUT, "the first connection", |event| {
        matches!(event, Event::Accepted { connection: 1 })
    });
    let first = Instant::now();
    fleet.expect(Duration::from_secs(20), "the second connection", |event| {
        matches!(event, Event::Accepted { connection: 2 })
    });
    let between = first.elapsed();
    assert!(
        between >= Duration::from_millis(9500),
        "tried again after {between:?}"
    );
    assert!(
        between < Duration::from_secs(14),
        "tried again after {between:?}"
    );
}

#[test]
fn a_refused_token_is_not_tried_again_within_15_seconds() {
    let mut fleet = Fleet::start("refused", Behaviour::default());
    let token_file = fleet.directory.join("install-token");
    std::fs::write(&token_file, "not-the-install-token\n").unwrap();
    let _node = fleet.start_node(&format!(" --install-token-file {}", token_file.display()));
    fleet.expect(FIRST_LINK_TIMEOUT, "an assignment attempt", |event| {
        matches!(event, Event::AssignAttempted)
    });
    fleet.quiet(
        Duration::from_secs(15),
        "a second attempt after a refusal",
        |event| matches!(event, Event::AssignAttempted),
    );
}

async fn connect_mode_outcome(port: u16) -> Result<(), capnp::Error> {
    let deadline = Instant::now() + FIRST_LINK_TIMEOUT;
    let stream = loop {
        match Async::<TcpStream>::connect(([127, 0, 0, 1], port)).await {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "the node never listened on {port}: {error}"
                );
                async_io::Timer::after(Duration::from_millis(100)).await;
            }
        }
    };
    let (reader, writer) = stream.split();
    let network = twoparty::VatNetwork::new(reader, writer, Side::Client, Default::default());
    let mut rpc_system = RpcSystem::new(Box::new(network), None);
    let node: dusk::Client = rpc_system.bootstrap(Side::Server);
    let work = async {
        let program_args = dusk_program_nightfall::Args::connect(&ConnectArgs {
            fleet: String::from("127.0.0.1:1"),
            fleet_server_name: None,
            provision: String::from("127.0.0.1:1"),
            provision_server_name: None,
            trust_anchors: String::from("fleet-server-ca.pem"),
            install_token_file: None,
            heartbeat_timeout_seconds: 90,
        })
        .as_program_args()?;
        let mut process_request = node.process_request();
        program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
        let process = process_request.send().promise.await?.get()?.get_result()?;
        process.run_request().send().promise.await.map(|_| ())
    };
    futures::pin_mut!(work);
    match futures::future::select(rpc_system, work).await {
        futures::future::Either::Left((ended, _)) => {
            panic!("the connection to the node ended first: {ended:?}")
        }
        futures::future::Either::Right((outcome, _)) => outcome,
    }
}

#[test]
fn connect_mode_refuses_to_start_on_a_node_without_persistent_kvs_keys() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _node = start_node(format!("nightfall -l 127.0.0.1:{port}"), None);
    let error = futures::executor::block_on(connect_mode_outcome(port))
        .expect_err("connect mode ran on a node without persistent kvs keys");
    assert!(
        error
            .to_string()
            .contains("this node keeps no persistent kvs keys to hold its identity"),
        "{error}"
    );
}
