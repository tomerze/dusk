mod support;

use dusk_base::dusk_program_sh::sh_capnp;
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS};
use nightfall::contracts::Contract;
use nightfall::directory::Route;
use serde_json::Value;
use std::io::Read;
use std::time::Duration;
use support::harness::{
    CAMPAIGN, DEVICE, Environment, INSTALLATION, INTENT_PRINCIPAL, OTHER_DEVICE, connect_client,
    free_port, ledger, link_node, local_session, run, run_returning, shutdown, wait_for_session,
    wait_until, with_action, with_event,
};
use support::node::{kill, pid, ps, run_script, shell_server, shell_server_request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn node() -> (DuskNixImpl, u16) {
    let port = free_port();
    let node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    (node, port)
}

fn verify_ledger(environment: &Environment, log: &nightfall_ledger::memory::MemoryLog) {
    let keys =
        nightfall_ledger::signing::VerifyingKeys::from_jwks(&environment.ledger_key.public_jwks())
            .unwrap();
    let mut lines = Vec::new();
    for record in log.records() {
        lines.extend_from_slice(&record);
        lines.push(b'\n');
    }
    let report =
        nightfall_ledger::verifier::verify_lines(lines.as_slice(), keys, Default::default())
            .unwrap();
    assert!(report.is_clean(), "{report}");
    assert!(report.checkpoints > 0, "{report}");
}

fn check_connections(environment: &Environment) -> Vec<Value> {
    let validator = Contract::Connections.validator().unwrap();
    let connections = environment.connections();
    for message in &connections {
        validator.check(message.to_string().as_bytes()).unwrap();
    }
    connections
}

async fn error_of<T>(outcome: impl Future<Output = Result<T, capnp::Error>>) -> capnp::Error {
    match outcome.await {
        Ok(_) => panic!("the call succeeded"),
        Err(error) => error,
    }
}

#[test]
fn a_client_drives_a_node_through_the_inner_listener_and_every_call_is_ledgered() {
    let environment = Environment::new();
    let (instance, log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    run(async {
        let bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, _, _) = wait_for_session(&instance).await;
        let client = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-0"),
        )
        .await;
        let held = client.dusk.ps_request().send().promise.await.unwrap();
        let processes = held.get().unwrap().get_process_entries().unwrap().len();
        assert!(processes > 0);
        assert_eq!(ps(&client.dusk).await.unwrap(), processes);
        drop(held);

        let shell = shell_server(&client.dusk, None).await;
        let mut run_request = client.dusk.run_request();
        run_request.get().set_process(shell.clone());
        run_request.send().promise.await.unwrap();
        let portal = shell
            .portal_request()
            .send()
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        let program_id = portal
            .program_id_request()
            .send()
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_program_id();
        assert_eq!(program_id, sh_capnp::PROGRAM_ID);
        kill(&client.dusk, pid(&shell).await.unwrap()).await;

        let pipelined = shell_server_request(&client.dusk, None);
        let pipelined_program = pipelined
            .pipeline
            .get_result()
            .portal_request()
            .send()
            .pipeline
            .get_result()
            .program_id_request()
            .send()
            .promise;
        let pipelined_process = pipelined
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        let mut pipelined_run = client.dusk.run_request();
        pipelined_run.get().set_process(pipelined_process.clone());
        pipelined_run.send().promise.await.unwrap();
        let pipelined_program = pipelined_program
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_program_id();
        assert_eq!(pipelined_program, sh_capnp::PROGRAM_ID);
        kill(&client.dusk, pid(&pipelined_process).await.unwrap()).await;

        assert_eq!(
            run_script(&client.dusk, None, "echo hello").await,
            ["hello"]
        );

        let settime = client
            .dusk
            .settime_request()
            .send()
            .promise
            .await
            .err()
            .unwrap();
        assert_eq!(settime.kind, capnp::ErrorKind::Unimplemented);
        wait_until(
            "the ledger to hold the ps results",
            Duration::from_secs(10),
            || with_action(&ledger(&log), "Dusk.ps").len() >= 4,
        )
        .await;
        drop(bridge);
        wait_until("the node session to close", Duration::from_secs(10), || {
            local_session(&instance).is_none()
        })
        .await;
    });
    shutdown(instance);

    let entries = ledger(&log);
    verify_ledger(&environment, &log);
    let setup: Vec<&str> = with_event(&entries, "setup_call")
        .iter()
        .map(|entry| entry["action"].as_str().unwrap())
        .collect();
    assert!(
        setup.starts_with(&["Dusk.namespaceId", "Dusk.programs"]),
        "{setup:?}"
    );
    assert!(setup.contains(&"Dusk.dusk"), "{setup:?}");
    let opened = with_event(&entries, "session_open");
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0]["device_id"], DEVICE);
    let closed = with_event(&entries, "session_close");
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0]["event_detail"]["reason"], "node_closed");
    let ps_entries = with_action(&entries, "Dusk.ps");
    let calls: Vec<&&Value> = ps_entries
        .iter()
        .filter(|entry| entry["kind"] == "call")
        .collect();
    let results: Vec<&&Value> = ps_entries
        .iter()
        .filter(|entry| entry["kind"] == "result")
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(results.len(), 2);
    for call in &calls {
        assert_eq!(call["principal"], "operator-0");
        assert_eq!(call["direction"], "client_to_node");
        assert_eq!(call["cap_id"], 0);
        assert_eq!(call["pid"], "0");
        assert_eq!(call["param_hash"].as_str().unwrap().len(), 64);
        assert!(results.iter().any(|result| result["call_id"] == call["call_id"] && result["result_code"] == "ok"));
    }
    assert_eq!(results[0]["result_cap_ids"], results[1]["result_cap_ids"]);
    let sends = with_action(&entries, "Stream.send");
    assert!(
        sends
            .iter()
            .any(|entry| entry["direction"] == "node_to_client")
    );
    let denied = with_action(&entries, "Dusk.settime");
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0]["result_code"], "denied");
    let portal_results = with_action(&entries, "Process.portal")
        .into_iter()
        .filter(|entry| entry["kind"] == "result")
        .count();
    assert!(portal_results >= 2);

    let connections = check_connections(&environment);
    assert_eq!(connections.len(), 2);
    assert_eq!(connections[0]["event"], "connected");
    assert_eq!(connections[0]["tenant"], "acme");
    assert_eq!(connections[1]["disconnect_reason"], "node_closed");
}

#[test]
fn every_call_of_a_process_at_a_fixed_pid_is_ledgered_with_that_pid() {
    let environment = Environment::new();
    let (instance, log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    let fixed = 0x5eed_0000_0000_0000 | u64::from(node_port);
    environment.intend(INSTALLATION, fixed, 1, 0);
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, _, _) = wait_for_session(&instance).await;
        let client = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("dawn-0"),
        )
        .await;
        assert_eq!(
            run_script(&client.dusk, Some(fixed), "echo hello").await,
            ["hello"]
        );
        let fresh = client
            .dusk
            .dusk_request()
            .send()
            .promise
            .await
            .unwrap()
            .get()
            .unwrap()
            .get_result()
            .unwrap();
        assert!(ps(&fresh).await.unwrap() > 0);
        wait_until(
            "the ledger to hold the reap",
            Duration::from_secs(10),
            || {
                with_action(&ledger(&log), "Dusk.waitpid")
                    .iter()
                    .any(|entry| entry["kind"] == "result")
            },
        )
        .await;
    });
    shutdown(instance);
    let entries = ledger(&log);
    verify_ledger(&environment, &log);
    let pid = fixed.to_string();
    for action in [
        "Dusk.process",
        "Dusk.run",
        "Process.portal",
        "ShPortal.sh",
        "Stream.send",
        "Stream.done",
        "Process.pid",
        "Dusk.kill",
        "Dusk.waitpid",
    ] {
        let matching = with_action(&entries, action);
        assert!(!matching.is_empty(), "no {action} entry");
        for entry in matching {
            assert_eq!(entry["pid"], pid.as_str(), "{entry}");
            assert_eq!(entry["principal"], "dawn-0");
        }
    }
    let fresh_cap = with_action(&entries, "Dusk.dusk")
        .into_iter()
        .find(|entry| entry["kind"] == "result" && entry["principal"] == "dawn-0")
        .unwrap()["result_cap_ids"][0]
        .clone();
    let fresh_ps: Vec<&Value> = with_action(&entries, "Dusk.ps")
        .into_iter()
        .filter(|entry| entry["cap_id"] == fresh_cap)
        .collect();
    assert_eq!(fresh_ps.len(), 2);
    assert!(fresh_ps.iter().all(|entry| entry["pid"] == "0"));
}

#[test]
fn a_dawn_principal_creates_only_the_processes_twilight_intended() {
    let environment = Environment::new();
    let (instance, log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    let intended = 0x5eed_0000_0000_0000 | u64::from(node_port);
    let unintended = intended + 1;
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, _, _) = wait_for_session(&instance).await;
        let dawn = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("dawn-0"),
        )
        .await;
        let before = error_of(shell_server_request(&dawn.dusk, Some(intended)).promise).await;
        assert!(before.extra.contains("denied: not intended"), "{before}");
        environment.intend(INSTALLATION, intended, 1, 0);
        environment.intend("00000000000000000000000000000001", unintended, 9, 9);
        assert_eq!(
            run_script(&dawn.dusk, Some(intended), "echo intended").await,
            ["intended"]
        );
        assert_eq!(instance.shared.intended_processes.len(), 2);
        let elsewhere = error_of(shell_server_request(&dawn.dusk, Some(unintended)).promise).await;
        assert!(
            elsewhere.extra.contains("denied: not intended"),
            "{elsewhere}"
        );
        let operator = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-0"),
        )
        .await;
        assert_eq!(
            run_script(&operator.dusk, Some(unintended), "echo glass").await,
            ["glass"]
        );
        environment.unintend(INSTALLATION, intended);
        wait_until(
            "the tombstone to reach the table",
            Duration::from_secs(10),
            || instance.shared.intended_processes.len() == 1,
        )
        .await;
        let after = error_of(shell_server_request(&dawn.dusk, Some(intended)).promise).await;
        assert!(after.extra.contains("denied: not intended"), "{after}");
        let metrics = http_get(instance.addresses.admin, "/metrics");
        assert!(
            metrics.contains("nightfall_admission_refused_total{rule=\"process_without_intent\"}"),
            "{metrics}"
        );
        wait_until(
            "the ledger to hold the refusals",
            Duration::from_secs(10),
            || {
                with_action(&ledger(&log), "Dusk.process")
                    .iter()
                    .filter(|entry| entry["result_code"] == "denied")
                    .count()
                    >= 3
            },
        )
        .await;
    });
    shutdown(instance);
    let entries = ledger(&log);
    verify_ledger(&environment, &log);
    let refused: Vec<(String, String)> = with_action(&entries, "Dusk.process")
        .into_iter()
        .filter(|entry| entry["result_code"] == "denied")
        .map(|entry| {
            assert_eq!(entry["principal"], "dawn-0");
            assert_eq!(entry["intent_principal"], Value::Null);
            (
                entry["pid"].as_str().unwrap().to_string(),
                entry["event_detail"]["rule"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        refused,
        [
            (intended.to_string(), "process_without_intent".to_string()),
            (unintended.to_string(), "process_without_intent".to_string()),
            (intended.to_string(), "process_without_intent".to_string()),
        ]
    );
    let intended_text = intended.to_string();
    let admitted: Vec<&Value> = entries
        .iter()
        .filter(|entry| {
            entry["pid"] == intended_text.as_str()
                && entry["principal"] == "dawn-0"
                && entry["result_code"] != "denied"
                && entry["kind"] != "event"
        })
        .collect();
    assert!(
        admitted
            .iter()
            .any(|entry| entry["action"] == "ShPortal.sh")
    );
    for entry in admitted {
        assert_eq!(entry["intent_principal"], INTENT_PRINCIPAL, "{entry}");
        assert_eq!(entry["intent_campaign_id"], CAMPAIGN, "{entry}");
        assert_eq!(
            entry["intent_subject"],
            format!("campaign:{CAMPAIGN}"),
            "{entry}"
        );
    }
    let overrides = with_event(&entries, "admission_override");
    assert!(!overrides.is_empty());
    for event in &overrides {
        assert_eq!(event["principal"], "operator-0");
        assert_eq!(event["event_detail"]["role"], "operator");
        assert_eq!(event["pid"], unintended.to_string().as_str());
    }
    assert_eq!(overrides[0]["action"], "Dusk.process");
    assert_eq!(
        overrides[0]["event_detail"]["rule"],
        "process_without_intent"
    );
}

#[test]
fn a_revoked_node_is_dropped_live_and_refused_at_its_next_handshake() {
    let environment = Environment::new();
    let (instance, log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, _, _) = wait_for_session(&instance).await;
        let client = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-0"),
        )
        .await;
        assert!(ps(&client.dusk).await.is_ok());
        environment.node_state(DEVICE, None, Some("revoked"));
        wait_until(
            "the revoked session to close",
            Duration::from_secs(5),
            || local_session(&instance).is_none(),
        )
        .await;
        assert!(ps(&client.dusk).await.is_err());

        let _again = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        wait_until("the refusal to be ledgered", Duration::from_secs(5), || {
            !with_event(&ledger(&log), "session_refused").is_empty()
        })
        .await;
        assert!(local_session(&instance).is_none());
    });
    shutdown(instance);
    let entries = ledger(&log);
    let refused = with_event(&entries, "session_refused");
    assert_eq!(refused[0]["event_detail"]["reason"], "revoked");
    let connections = check_connections(&environment);
    assert_eq!(connections.len(), 2);
    assert_eq!(connections[1]["disconnect_reason"], "revoked");
}

#[test]
fn a_quarantine_flip_drops_client_membranes_and_keeps_the_node_link() {
    let environment = Environment::new();
    let (instance, log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, epoch, _) = wait_for_session(&instance).await;
        let client = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-0"),
        )
        .await;
        assert!(ps(&client.dusk).await.is_ok());
        environment.node_state(DEVICE, Some(INSTALLATION), Some("quarantined"));
        wait_until("the membrane to drop", Duration::from_secs(5), || {
            !with_event(&ledger(&log), "membrane_dropped").is_empty()
        })
        .await;
        let dropped = error_of(ps(&client.dusk)).await;
        assert_eq!(dropped.kind, capnp::ErrorKind::Disconnected, "{dropped}");
        assert_eq!(
            local_session(&instance).map(|session| session.1),
            Some(epoch)
        );

        let quarantined = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-1"),
        )
        .await;
        let denied = error_of(ps(&quarantined.dusk)).await;
        assert_eq!(denied.kind, capnp::ErrorKind::Unimplemented, "{denied}");
        let hostname = quarantined
            .dusk
            .hostname_request()
            .send()
            .promise
            .await
            .unwrap();
        assert!(
            !hostname
                .get()
                .unwrap()
                .get_result()
                .unwrap()
                .to_str()
                .unwrap()
                .is_empty()
        );

        environment.node_state(DEVICE, Some(INSTALLATION), None);
        wait_until(
            "the second membrane to drop",
            Duration::from_secs(5),
            || with_event(&ledger(&log), "membrane_dropped").len() >= 2,
        )
        .await;
        let released = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-2"),
        )
        .await;
        assert!(ps(&released.dusk).await.is_ok());
    });
    shutdown(instance);
    let entries = ledger(&log);
    let dropped = with_event(&entries, "membrane_dropped");
    assert!(
        dropped
            .iter()
            .any(|entry| entry["event_detail"]["reason"] == "lifecycle_changed")
    );
    assert_eq!(with_event(&entries, "session_open").len(), 1);
}

#[test]
fn a_node_that_stops_answering_heartbeats_is_closed() {
    let environment = Environment::new();
    let mut config = environment.config("nightfall-0", 0);
    config.fleet.heartbeat_seconds = 1;
    let (instance, _log) = environment.start(config);
    let (_node, node_port) = node();
    run(async {
        let bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        wait_for_session(&instance).await;
        bridge
            .paused
            .store(true, std::sync::atomic::Ordering::Release);
        wait_until(
            "the silent node to be closed",
            Duration::from_secs(15),
            || local_session(&instance).is_none(),
        )
        .await;
    });
    shutdown(instance);
    let connections = check_connections(&environment);
    assert_eq!(connections[1]["disconnect_reason"], "idle_timeout");
}

fn handoffs() -> u64 {
    let rendered = nightfall::admin::install_metrics().render();
    rendered
        .lines()
        .find(|line| line.starts_with("nightfall_shard_handoffs_total"))
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse::<f64>().ok())
        .map(|value| value as u64)
        .unwrap_or(0)
}

#[test]
fn an_inner_connection_accepted_on_another_shard_is_handed_to_the_node_shard() {
    nightfall::admin::install_metrics();
    let environment = Environment::new();
    let (instance, _log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, _, _) = wait_for_session(&instance).await;
        let mut attempts = 0;
        while handoffs() == 0 {
            attempts += 1;
            assert!(
                attempts <= 64,
                "no inner connection landed on the other shard"
            );
            let client = connect_client(
                instance.addresses.inner,
                namespace_id,
                environment.principal("operator-0"),
            )
            .await;
            assert!(ps(&client.dusk).await.unwrap() > 0);
        }
    });
    shutdown(instance);
}

#[test]
fn a_client_of_another_instance_is_relayed_to_the_instance_holding_the_node() {
    let environment = Environment::new();
    let (holder, _holder_log) = environment.start(environment.config("nightfall-0", 0));
    let (relayer, _relayer_log) = environment.start(environment.config("nightfall-1", 1));
    let (_node, node_port) = node();
    run(async {
        let _bridge = link_node(
            holder.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, _, _) = wait_for_session(&holder).await;
        wait_until(
            "the other instance to learn the node",
            Duration::from_secs(15),
            || {
                matches!(
                    relayer.shared.directory.lock().unwrap().route(namespace_id),
                    Route::Remote { .. }
                )
            },
        )
        .await;
        let client = connect_client(
            relayer.addresses.inner,
            namespace_id,
            environment.principal("operator-0"),
        )
        .await;
        assert!(ps(&client.dusk).await.unwrap() > 0);
    });
    shutdown(relayer);
    shutdown(holder);
}

#[test]
fn a_namespace_bound_to_another_identity_is_refused() {
    let environment = Environment::new();
    let (instance, log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (_, epoch, _) = wait_for_session(&instance).await;
        let _impostor = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(OTHER_DEVICE, INSTALLATION),
        )
        .await;
        wait_until(
            "the conflict to be ledgered",
            Duration::from_secs(5),
            || !with_event(&ledger(&log), "binding_conflict").is_empty(),
        )
        .await;
        assert_eq!(
            local_session(&instance).map(|session| session.1),
            Some(epoch)
        );
    });
    shutdown(instance);
    let entries = ledger(&log);
    let conflict = with_event(&entries, "binding_conflict");
    assert_eq!(conflict[0]["device_id"], OTHER_DEVICE);
    assert_eq!(conflict[0]["event_detail"]["bound_device_id"], DEVICE);
    assert!(conflict[0]["epoch"].is_null());
}

#[test]
fn unknown_server_names_get_an_alert_and_unheld_nodes_a_failing_bootstrap() {
    let environment = Environment::new();
    let (instance, _log) = environment.start(environment.config("nightfall-0", 0));
    run(async {
        let mut stream = tokio::net::TcpStream::connect(instance.addresses.fleet)
            .await
            .unwrap();
        let config = environment.node_identity(DEVICE, INSTALLATION);
        let mut connection =
            rustls::ClientConnection::new(config, "unknown.dusk.test".try_into().unwrap()).unwrap();
        let mut hello = Vec::new();
        connection.write_tls(&mut hello).unwrap();
        stream.write_all(&hello).await.unwrap();
        let mut alert = Vec::new();
        stream.read_to_end(&mut alert).await.unwrap();
        assert_eq!(alert, nightfall::sni::UNRECOGNIZED_NAME_ALERT);

        let client = connect_client(
            instance.addresses.inner,
            0x0123_4567_89ab_cdef,
            environment.principal("operator-0"),
        )
        .await;
        let failure = error_of(ps(&client.dusk)).await;
        assert_eq!(failure.kind, capnp::ErrorKind::Disconnected);
        assert!(
            failure
                .extra
                .contains("node 0123456789abcdef is not connected"),
            "{failure}"
        );
    });
    shutdown(instance);
}

#[test]
fn draining_closes_every_session_and_writes_an_empty_census() {
    let environment = Environment::new();
    let (mut instance, _log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    let bridge_runtime = std::thread::spawn({
        let fleet = instance.addresses.fleet;
        let identity = environment.node_identity(DEVICE, INSTALLATION);
        move || {
            run(async move {
                let _bridge = link_node(fleet, node_port, identity).await;
                tokio::time::sleep(Duration::from_secs(8)).await;
            })
        }
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while local_session(&instance).is_none() {
        assert!(std::time::Instant::now() < deadline, "no session");
        std::thread::sleep(Duration::from_millis(20));
    }
    instance.drain(Duration::from_millis(200));
    assert!(local_session(&instance).is_none());
    assert!(instance.shared.ready().is_err());
    shutdown(instance);
    bridge_runtime.join().unwrap();
    let connections = check_connections(&environment);
    assert_eq!(connections.last().unwrap()["disconnect_reason"], "shutdown");
    let validator = Contract::Census.validator().unwrap();
    let census = environment.broker.records("dusk.census");
    let headers: Vec<Value> = census
        .iter()
        .filter_map(|record| record.payload.as_ref())
        .map(|payload| {
            validator.check(payload).unwrap();
            serde_json::from_slice::<Value>(payload).unwrap()
        })
        .filter(|message| message["record"] == "header")
        .collect();
    let last = headers.last().unwrap();
    assert_eq!(last["full"], true);
    assert_eq!(last["chunk_count"], 0);
}

#[test]
fn a_draining_instance_serves_and_announces_the_nodes_it_holds_and_takes_no_new_one() {
    let environment = Environment::new();
    let (mut instance, _log) = environment.start(environment.config("nightfall-0", 0));
    let (_first_node, first_port) = node();
    let (_second_node, second_port) = node();
    let shared = instance.shared.clone();
    let addresses = instance.addresses;
    let identities = [
        environment.node_identity(DEVICE, INSTALLATION),
        environment.node_identity(OTHER_DEVICE, INSTALLATION),
        environment.node_identity(DEVICE, OTHER_DEVICE),
    ];
    let principal = environment.principal("operator-0");
    let broker = environment.broker.clone();
    let linked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let checks = std::thread::spawn({
        let linked = linked.clone();
        move || {
            run_returning(async move {
                let [first, second, newcomer] = identities;
                let _first = link_node(addresses.fleet, first_port, first).await;
                let _second = link_node(addresses.fleet, second_port, second).await;
                let local_count = || shared.directory.lock().unwrap().local_count();
                wait_until("two sessions", Duration::from_secs(20), || {
                    local_count() == 2
                })
                .await;
                linked.store(true, std::sync::atomic::Ordering::Release);
                wait_until(
                    "the drain to close one session",
                    Duration::from_secs(10),
                    || local_count() == 1,
                )
                .await;
                let census_mark = broker.records("dusk.census").len();
                let held = *shared
                    .directory
                    .lock()
                    .unwrap()
                    .local_entries()
                    .next()
                    .unwrap()
                    .0;
                let client = connect_client(addresses.inner, held, principal).await;
                let reached = ps(&client.dusk).await.is_ok();
                let refused = match tokio::net::TcpStream::connect(addresses.fleet).await {
                    Err(_) => true,
                    Ok(tcp) => tokio::time::timeout(
                        Duration::from_secs(2),
                        tokio_rustls::TlsConnector::from(newcomer).connect(
                            rustls_pki_types::ServerName::try_from(support::harness::FLEET_NAME)
                                .unwrap(),
                            tcp,
                        ),
                    )
                    .await
                    .is_ok_and(|connected| connected.is_err()),
                };
                let census_of_one = || {
                    broker.records("dusk.census")[census_mark..]
                        .iter()
                        .filter_map(|record| record.payload.as_ref())
                        .map(|payload| serde_json::from_slice::<Value>(payload).unwrap())
                        .any(|message| {
                            message["record"] == "header"
                                && message["full"] == true
                                && message["session_count"] == 1
                        })
                };
                wait_until(
                    "a census of the session still held",
                    Duration::from_secs(4),
                    census_of_one,
                )
                .await;
                (reached, refused)
            })
        }
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !linked.load(std::sync::atomic::Ordering::Acquire) {
        assert!(
            std::time::Instant::now() < deadline,
            "the nodes did not link"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    instance.drain(Duration::from_secs(10));
    let (reached, refused) = checks.join().unwrap();
    assert!(
        reached,
        "a held node was not reachable through the inner listener"
    );
    assert!(refused, "a new node connected to a draining instance");
    shutdown(instance);
    let headers: Vec<Value> = environment
        .broker
        .records("dusk.census")
        .into_iter()
        .filter_map(|record| record.payload)
        .map(|payload| serde_json::from_slice::<Value>(&payload).unwrap())
        .filter(|message| message["record"] == "header" && message["full"] == true)
        .collect();
    let last = headers.last().unwrap();
    assert_eq!(last["chunk_count"], 0);
    assert_eq!(last["session_count"], 0);
}

#[test]
fn a_node_is_refused_until_the_node_states_are_read() {
    let environment = Environment::new();
    let (instance, _log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    instance
        .shared
        .readiness
        .node_state
        .store(false, std::sync::atomic::Ordering::Release);
    run(async {
        let tcp = tokio::net::TcpStream::connect(instance.addresses.fleet)
            .await
            .unwrap();
        let refused =
            tokio_rustls::TlsConnector::from(environment.node_identity(DEVICE, INSTALLATION))
                .connect(
                    rustls_pki_types::ServerName::try_from(support::harness::FLEET_NAME).unwrap(),
                    tcp,
                )
                .await;
        assert!(refused.is_err());
        assert_eq!(instance.shared.directory.lock().unwrap().local_count(), 0);
        instance
            .shared
            .readiness
            .node_state
            .store(true, std::sync::atomic::Ordering::Release);
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        wait_for_session(&instance).await;
    });
    shutdown(instance);
}

fn http_get(address: std::net::SocketAddr, path: &str) -> String {
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    std::io::Write::write_all(
        &mut stream,
        format!("GET {path} HTTP/1.1\r\nHost: nightfall\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn the_admin_listener_serves_health_readiness_and_metrics_without_tls() {
    let environment = Environment::new();
    let (instance, _log) = environment.start(environment.config("nightfall-0", 0));
    let health = http_get(instance.addresses.admin, "/healthz");
    assert!(health.starts_with("HTTP/1.1 200"), "{health}");
    let ready = http_get(instance.addresses.admin, "/readyz");
    assert!(ready.starts_with("HTTP/1.1 200"), "{ready}");
    let metrics = http_get(instance.addresses.admin, "/metrics");
    assert!(metrics.starts_with("HTTP/1.1 200"), "{metrics}");
    assert!(metrics.contains("nightfall_sessions"), "{metrics}");
    let sessions = http_get(instance.addresses.admin, "/v1/sessions");
    assert!(sessions.starts_with("HTTP/1.1 404"), "{sessions}");
    shutdown(instance);
}

#[test]
fn a_reconnect_of_the_same_node_replaces_its_session_with_a_higher_epoch() {
    let environment = Environment::new();
    let mut config = environment.config("nightfall-0", 0);
    config.limits.insert(
        "session_setups_per_identity_per_5s".to_string(),
        toml::Value::Integer(5),
    );
    let (instance, log) = environment.start(config);
    let (_node, node_port) = node();
    run(async {
        let _first = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, first_epoch, _) = wait_for_session(&instance).await;
        let _second = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        wait_until("the second session", Duration::from_secs(10), || {
            local_session(&instance).is_some_and(|session| session.1 > first_epoch)
        })
        .await;
        wait_until(
            "the first session to close",
            Duration::from_secs(10),
            || with_event(&ledger(&log), "session_close").len() == 1,
        )
        .await;
        assert_eq!(local_session(&instance).unwrap().0, namespace_id);
    });
    shutdown(instance);
    let entries = ledger(&log);
    let closed = with_event(&entries, "session_close");
    assert_eq!(closed[0]["event_detail"]["reason"], "replaced");
    let connections = check_connections(&environment);
    assert!(
        connections
            .iter()
            .any(|message| message["disconnect_reason"] == "replaced")
    );
}

fn connected_elsewhere(
    instance: &str,
    device_id: &str,
    namespace_id: u64,
    epoch: u64,
) -> nightfall::kafka::OutgoingRecord {
    let record = nightfall::events::SessionRecord {
        identity: nightfall::directory::NodeIdentity::parse(device_id, INSTALLATION).unwrap(),
        namespace_id,
        epoch,
        instance: instance.to_string(),
        inner_address: format!("{instance}.inner:8444"),
        remote_address: "192.0.2.10:40000".to_string(),
        tenant: None,
        cert_fingerprint: "00".repeat(32),
        cert_not_after: nightfall::events::now(),
        connected_at: nightfall::events::now(),
    };
    nightfall::kafka::OutgoingRecord {
        topic: "dusk.connections".to_string(),
        partition: None,
        key: Some(record.key()),
        payload: Some(record.message(None).to_string().into_bytes()),
    }
}

#[test]
fn connection_events_of_other_instances_apply_quickly_and_replace_older_local_sessions() {
    let environment = Environment::new();
    let (instance, log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, epoch, _) = wait_for_session(&instance).await;
        for index in 0..3000u64 {
            environment
                .broker
                .produce(connected_elsewhere(
                    "nightfall-9",
                    OTHER_DEVICE,
                    0x00ab_0000 + index,
                    1,
                ))
                .unwrap();
        }
        wait_until(
            "every connection event to be applied",
            Duration::from_secs(20),
            || instance.shared.directory.lock().unwrap().remote_count() >= 3000,
        )
        .await;
        environment
            .broker
            .produce(connected_elsewhere(
                "nightfall-9",
                DEVICE,
                namespace_id,
                epoch + 1,
            ))
            .unwrap();
        wait_until(
            "the replaced session to close",
            Duration::from_secs(10),
            || local_session(&instance).is_none(),
        )
        .await;
        let remote = instance
            .shared
            .directory
            .lock()
            .unwrap()
            .remote(namespace_id)
            .cloned()
            .unwrap();
        assert_eq!(&*remote.instance, "nightfall-9");
        assert_eq!(remote.epoch, epoch + 1);
    });
    shutdown(instance);
    let entries = ledger(&log);
    assert_eq!(
        with_event(&entries, "session_close")[0]["event_detail"]["reason"],
        "replaced"
    );
}

async fn admin_request(
    address: std::net::SocketAddr,
    config: std::sync::Arc<rustls::ClientConfig>,
    method: &str,
    path: &str,
) -> String {
    let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut tls = tokio_rustls::TlsConnector::from(config)
        .connect(
            rustls_pki_types::ServerName::try_from(support::harness::ADMIN_NAME).unwrap(),
            tcp,
        )
        .await
        .unwrap();
    tls.write_all(
        format!("{method} {path} HTTP/1.1\r\nHost: nightfall\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut response = Vec::new();
    if let Err(error) = tls.read_to_end(&mut response).await {
        assert!(!response.is_empty(), "{error}");
    }
    String::from_utf8(response).unwrap()
}

fn anonymous(environment: &Environment) -> std::sync::Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(environment.pki.internal.certificate.clone())
        .unwrap();
    std::sync::Arc::new(
        rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth(),
    )
}

#[test]
fn the_admin_api_lists_describes_and_kills_sessions_for_admins_only() {
    let environment = Environment::new();
    let mut config = environment.config("nightfall-0", 0);
    environment.with_admin_tls(&mut config);
    let (instance, log) = environment.start(config);
    let (_node, node_port) = node();
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, _, _) = wait_for_session(&instance).await;
        let namespace = format!("{namespace_id:016x}");
        let admin = instance.addresses.admin;

        let health = admin_request(admin, anonymous(&environment), "GET", "/healthz").await;
        assert!(health.starts_with("HTTP/1.1 200"), "{health}");
        let refused = admin_request(admin, anonymous(&environment), "GET", "/v1/sessions").await;
        assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
        let operator = admin_request(
            admin,
            environment.principal("operator-0"),
            "GET",
            "/v1/sessions",
        )
        .await;
        assert!(operator.starts_with("HTTP/1.1 403"), "{operator}");

        let client = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-0"),
        )
        .await;
        let shell = shell_server(&client.dusk, None).await;

        let listed = admin_request(
            admin,
            environment.principal("admin-0"),
            "GET",
            "/v1/sessions",
        )
        .await;
        assert!(listed.starts_with("HTTP/1.1 200"), "{listed}");
        assert!(listed.contains(&namespace), "{listed}");
        let described = admin_request(
            admin,
            environment.principal("admin-0"),
            "GET",
            &format!("/v1/sessions/{namespace}"),
        )
        .await;
        assert!(
            described.contains("\"principal\":\"operator-0\""),
            "{described}"
        );
        let provenance = admin_request(
            admin,
            environment.principal("admin-0"),
            "GET",
            &format!("/v1/sessions/{namespace}/provenance"),
        )
        .await;
        assert!(
            provenance.contains("\"action\":\"Dusk.process\""),
            "{provenance}"
        );

        let session: Value =
            serde_json::from_str(described.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let session_id = session["clients"][0]["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let killed_client = admin_request(
            admin,
            environment.principal("admin-0"),
            "POST",
            &format!("/v1/clients/{session_id}/kill"),
        )
        .await;
        assert!(killed_client.starts_with("HTTP/1.1 202"), "{killed_client}");
        assert!(pid(&shell).await.is_err());
        let unknown_client = admin_request(
            admin,
            environment.principal("admin-0"),
            "POST",
            &format!("/v1/clients/{session_id}/kill"),
        )
        .await;
        assert!(
            unknown_client.starts_with("HTTP/1.1 404"),
            "{unknown_client}"
        );
        let client = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-0"),
        )
        .await;
        let shell = shell_server(&client.dusk, None).await;

        let denied = environment.pki.internal.principal("admin-1");
        let fingerprint = nightfall_ledger::entry::sha256_hex(denied.certificate.as_ref());
        let denied_config =
            environment.client_config(&environment.pki.internal.certificate, &denied);
        let allowed = admin_request(admin, denied_config.clone(), "GET", "/v1/sessions").await;
        assert!(allowed.starts_with("HTTP/1.1 200"), "{allowed}");
        nightfall::server::apply_permissions(
            &instance.shared,
            nightfall_membrane::permissions::Permissions::from_toml(&format!(
                "deny_certificates = [\"{fingerprint}\"]\n{}",
                support::harness::PERMISSIONS
            ))
            .unwrap(),
        );
        let refused = admin_request(admin, denied_config, "GET", "/v1/sessions").await;
        assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");

        let killed = admin_request(
            admin,
            environment.principal("admin-0"),
            "POST",
            &format!("/v1/sessions/{namespace}/kill"),
        )
        .await;
        assert!(killed.starts_with("HTTP/1.1 202"), "{killed}");
        wait_until(
            "the killed session to close",
            Duration::from_secs(5),
            || local_session(&instance).is_none(),
        )
        .await;
        assert!(pid(&shell).await.is_err());
        let missing = admin_request(
            admin,
            environment.principal("admin-0"),
            "GET",
            &format!("/v1/sessions/{namespace}"),
        )
        .await;
        assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
    });
    shutdown(instance);
    let entries = ledger(&log);
    let closed = &with_event(&entries, "session_close")[0]["event_detail"];
    assert_eq!(closed["reason"], "killed");
    assert_eq!(closed["by"], "admin-0");
    let dropped: Vec<&Value> = with_event(&entries, "membrane_dropped")
        .into_iter()
        .filter(|entry| entry["event_detail"]["reason"] == "killed")
        .collect();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0]["event_detail"]["by"], "admin-0");
}

fn device_report(mut device: nightfall_provisioning::provision_capnp::device_report::Builder) {
    device.set_hardware_fingerprint(&[7u8; 32]);
    device.set_installation_hint("test");
    device.set_dusk_version("0.1.0");
    device.set_impl("nix");
    device.set_target_os("linux");
    device.set_target_arch("x86_64");
    device.set_hostname("enrolling-node");
}

#[test]
fn a_node_enrolls_with_the_fleet_token_and_connects_with_its_new_certificate() {
    let environment = Environment::new();
    let step_ca = support::step_ca::FakeStepCa::start(
        environment.provisioner.public_jwk(),
        environment.pki.fleet_client.clone(),
    );
    std::fs::write(
        environment.paths().file("step-ca-root.crt"),
        &step_ca.root_pem,
    )
    .unwrap();
    let mut config = environment.config("nightfall-0", 0);
    config.step_ca.url = step_ca.url.clone();
    let (instance, _log) = environment.start(config);
    let (_node, node_port) = node();
    let enrolled = run_returning(async {
        let provisioning = support::harness::connect_provisioning(
            instance.addresses.fleet,
            support::harness::anonymous_config(&environment.pki.fleet_server.certificate),
        )
        .await;
        let mut stolen = provisioning.client.assign_request();
        stolen
            .get()
            .init_credential()
            .set_fleet_token("not the fleet token");
        device_report(stolen.get().init_device());
        let refused = error_of(async { stolen.send().promise.await.map(|_| ()) }).await;
        assert!(refused.extra.contains("denied"), "{refused}");

        let mut assign = provisioning.client.assign_request();
        assign
            .get()
            .init_credential()
            .set_fleet_token(support::harness::FLEET_TOKEN);
        device_report(assign.get().init_device());
        let assigned = assign.send().promise.await.unwrap();
        let assignment = assigned.get().unwrap().get_assignment().unwrap();
        let device_id = assignment.get_device_id().unwrap().to_string().unwrap();
        let installation_id = assignment
            .get_installation_id()
            .unwrap()
            .to_string()
            .unwrap();
        let challenge = assignment.get_challenge().unwrap().to_vec();

        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut parameters = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        parameters.distinguished_name = rcgen::DistinguishedName::new();
        parameters.subject_alt_names = vec![
            rcgen::SanType::URI(format!("urn:dusk:device:{device_id}").try_into().unwrap()),
            rcgen::SanType::URI(
                format!("urn:dusk:installation:{installation_id}")
                    .try_into()
                    .unwrap(),
            ),
        ];
        let csr = parameters.serialize_request(&key).unwrap();
        let mut enroll = provisioning.client.enroll_request();
        enroll
            .get()
            .init_credential()
            .set_fleet_token(support::harness::FLEET_TOKEN);
        device_report(enroll.get().init_device());
        enroll.get().set_challenge(&challenge);
        enroll.get().set_csr(csr.der());
        let issued = enroll.send().promise.await.unwrap();
        let issued = issued.get().unwrap().get_issued().unwrap();
        let chain: Vec<rustls_pki_types::CertificateDer<'static>> = issued
            .get_certificate_chain()
            .unwrap()
            .iter()
            .map(|certificate| {
                rustls_pki_types::CertificateDer::from(certificate.unwrap().to_vec())
            })
            .collect();
        assert!(issued.get_renew_after_unix_ms() < issued.get_not_after_unix_ms());

        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(environment.pki.fleet_server.certificate.clone())
            .unwrap();
        let identity = std::sync::Arc::new(
            rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(roots)
            .with_client_auth_cert(
                chain,
                rustls_pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
            )
            .unwrap(),
        );
        let _bridge = link_node(instance.addresses.fleet, node_port, identity).await;
        wait_for_session(&instance).await;
        (device_id, installation_id)
    });
    shutdown(instance);
    let (device_id, installation_id) = enrolled;
    let connections = check_connections(&environment);
    assert_eq!(connections[0]["device_id"], device_id.as_str());
    assert_eq!(connections[0]["installation_id"], installation_id.as_str());
    assert_eq!(connections[0]["tenant"], "acme");
    let enrollments: Vec<Value> = environment
        .broker
        .records("dusk.enrollments")
        .into_iter()
        .filter_map(|record| record.payload)
        .map(|payload| serde_json::from_slice(&payload).unwrap())
        .collect();
    let outcomes: Vec<(&str, &str)> = enrollments
        .iter()
        .map(|event| {
            (
                event["operation"].as_str().unwrap(),
                event["outcome"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            ("assign", "denied"),
            ("assign", "assigned"),
            ("enroll", "issued")
        ]
    );
    assert_eq!(step_ca.signed.lock().unwrap().len(), 1);
}

#[test]
fn a_permissions_reload_drops_changed_clients_and_denies_new_ones_at_the_handshake() {
    let environment = Environment::new();
    let (instance, log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, _, _) = wait_for_session(&instance).await;
        let dawn = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("dawn-0"),
        )
        .await;
        let operator = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-0"),
        )
        .await;
        assert!(ps(&dawn.dusk).await.unwrap() > 0);
        assert!(ps(&operator.dusk).await.unwrap() > 0);

        let denied = environment.pki.internal.principal("operator-5");
        let fingerprint = nightfall_ledger::entry::sha256_hex(denied.certificate.as_ref());
        let narrowed = support::harness::PERMISSIONS.replacen(
            "\"Dusk.ps\", \"Dusk.kill\"",
            "\"Dusk.kill\"",
            1,
        );
        assert_ne!(narrowed, support::harness::PERMISSIONS);
        nightfall::server::apply_permissions(
            &instance.shared,
            nightfall_membrane::permissions::Permissions::from_toml(&format!(
                "deny_certificates = [\"{fingerprint}\"]\ndeny_principals = [\"dawn-9\"]\n{narrowed}"
            ))
            .unwrap(),
        );
        wait_until(
            "the dawn client to be dropped",
            Duration::from_secs(5),
            || !with_event(&ledger(&log), "membrane_dropped").is_empty(),
        )
        .await;
        let dropped = error_of(ps(&dawn.dusk)).await;
        assert_eq!(dropped.kind, capnp::ErrorKind::Disconnected, "{dropped}");
        assert!(ps(&operator.dusk).await.unwrap() > 0);

        let narrowed_dawn = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("dawn-1"),
        )
        .await;
        let refused = error_of(ps(&narrowed_dawn.dusk)).await;
        assert_eq!(refused.kind, capnp::ErrorKind::Unimplemented, "{refused}");

        for identity in [
            environment.principal("dawn-9"),
            environment.client_config(&environment.pki.internal.certificate, &denied),
        ] {
            let client = connect_client(instance.addresses.inner, namespace_id, identity).await;
            let failure = error_of(ps(&client.dusk)).await;
            assert!(
                matches!(
                    failure.kind,
                    capnp::ErrorKind::Disconnected | capnp::ErrorKind::PrematureEndOfFile
                ),
                "{failure}"
            );
        }
        let allowed = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-6"),
        )
        .await;
        assert!(ps(&allowed.dusk).await.unwrap() > 0);
    });
    shutdown(instance);
    let entries = ledger(&log);
    let dropped = with_event(&entries, "membrane_dropped");
    assert!(dropped.iter().any(|entry| entry["principal"] == "dawn-0"
        && entry["event_detail"]["reason"] == "permissions_changed"));
    assert!(
        dropped
            .iter()
            .all(|entry| entry["principal"] != "operator-0"
                || entry["event_detail"]["reason"] == "node_session_closed")
    );
}

#[test]
fn a_node_session_closes_when_its_certificate_expires() {
    let environment = Environment::new();
    let (instance, log) = environment.start(environment.config("nightfall-0", 0));
    let (_node, node_port) = node();
    run(async {
        let not_after = time::OffsetDateTime::now_utc() + Duration::from_secs(4);
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity_until(DEVICE, INSTALLATION, not_after),
        )
        .await;
        wait_for_session(&instance).await;
        wait_until(
            "the session to close at notAfter",
            Duration::from_secs(15),
            || local_session(&instance).is_none(),
        )
        .await;
    });
    shutdown(instance);
    let connections = check_connections(&environment);
    assert_eq!(connections[1]["disconnect_reason"], "cert_expired");
    let closed = with_event(&ledger(&log), "session_close")[0].clone();
    assert_eq!(closed["event_detail"]["reason"], "cert_expired");
}

#[test]
fn a_node_beyond_max_sessions_is_refused_before_tls() {
    let environment = Environment::new();
    let mut config = environment.config("nightfall-0", 0);
    config.fleet.max_sessions = 1;
    let (instance, _log) = environment.start(config);
    let (_node, node_port) = node();
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        wait_for_session(&instance).await;
        let tcp = tokio::net::TcpStream::connect(instance.addresses.fleet)
            .await
            .unwrap();
        let refused =
            tokio_rustls::TlsConnector::from(environment.node_identity(OTHER_DEVICE, INSTALLATION))
                .connect(
                    rustls_pki_types::ServerName::try_from(support::harness::FLEET_NAME).unwrap(),
                    tcp,
                )
                .await;
        assert!(refused.is_err());
        assert_eq!(instance.shared.directory.lock().unwrap().local_count(), 1);
    });
    shutdown(instance);
}

#[test]
fn the_fleet_listener_takes_the_node_address_from_a_proxy_header() {
    let environment = Environment::new();
    let mut config = environment.config("nightfall-0", 0);
    config.fleet.proxy_protocol = true;
    config.fleet.proxy_protocol_trusted_cidrs = vec!["127.0.0.1/32".to_string()];
    let (instance, _log) = environment.start(config);
    let (_node, node_port) = node();
    let claimed: std::net::SocketAddr = "198.51.100.7:40001".parse().unwrap();
    run(async {
        let _bridge = support::harness::link_node_with_header(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
            &nightfall::proxy::encode(claimed, instance.addresses.fleet),
        )
        .await;
        wait_for_session(&instance).await;
    });
    shutdown(instance);
    let connections = check_connections(&environment);
    assert_eq!(connections[0]["remote_address"], claimed.to_string());
}
