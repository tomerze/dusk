#[path = "support/fake_nightfall.rs"]
mod fake_nightfall;

use std::time::Duration;

use dusk_program::value::Value;
use fake_nightfall::*;

#[test]
fn first_start_enrolls_and_serves_dusk_and_a_restart_reuses_the_identity() {
    let mut fleet = Fleet::start(
        "first-start",
        Behaviour {
            links: vec![LinkMode::StoredThenStopNode],
            ..Behaviour::default()
        },
    );
    let node = fleet.start_node("");

    let Event::Assigned {
        installation_id,
        report,
        credential,
        server_name: provision_server_name,
    } = fleet.expect(FIRST_LINK_TIMEOUT, "assignment", |event| {
        matches!(event, Event::Assigned { .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(credential, "fleet");
    assert_eq!(provision_server_name.as_deref(), Some("provision.test"));
    assert_eq!(report.hardware_fingerprint.len(), 32);
    assert_eq!(report.installation_hint, "");
    assert_eq!(report.impl_name, "nix");
    assert_eq!(report.target_os, std::env::consts::OS);
    assert_eq!(report.target_arch, std::env::consts::ARCH);
    assert!(!report.dusk_version.is_empty());
    assert_eq!(report.hostname, dusk_core::driver::hostname().unwrap());

    let Event::Enrolled {
        certificate,
        public_key,
        ..
    } = fleet.expect(Duration::from_secs(10), "enrollment", |event| {
        matches!(event, Event::Enrolled { installation_id: enrolled, .. } if *enrolled == installation_id)
    })
    else {
        unreachable!()
    };
    let Event::Linked {
        certificate: linked_with,
        namespace_id: first_namespace,
        hostname,
        server_name: fleet_server_name,
        ..
    } = fleet.expect(Duration::from_secs(10), "link", |event| {
        matches!(event, Event::Linked { connection: 1, .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(linked_with, certificate);
    assert_ne!(first_namespace, 0);
    assert_eq!(hostname, dusk_core::driver::hostname().unwrap());
    assert_eq!(fleet_server_name.as_deref(), Some("fleet.test"));

    let stored = fleet.stored(1);
    assert_eq!(
        stored.installation_id,
        Some((Value::String(installation_id.clone()), KEPT))
    );
    assert_eq!(stored.certificate(), certificate);
    assert_eq!(stored.certificate_chain.as_ref().unwrap().1, KEPT);
    assert_eq!(stored.private_key.as_ref().unwrap().1, KEPT_SECRET);
    assert_eq!(stored.public_key(), public_key);
    assert!(stored.staged_private_key.is_none(), "{stored:?}");
    match &stored.device_id {
        Some((Value::String(machine_id), _)) if is_machine_id(machine_id) => {
            assert_eq!(
                fingerprint(machine_id.trim().as_bytes()),
                hex(&report.hardware_fingerprint)
            );
            assert!(stored.hardware_fingerprint.is_none(), "{stored:?}");
        }
        other => {
            assert_eq!(
                stored.hardware_fingerprint,
                Some((Value::Bytes(report.hardware_fingerprint.clone()), KEPT)),
                "{other:?}"
            );
        }
    }
    fleet.expect(Duration::from_secs(10), "the node stopping", |event| {
        matches!(event, Event::Stopped)
    });
    join_stopped(node);

    let _restarted = fleet.start_node("");
    let Event::Linked {
        certificate: relinked_with,
        namespace_id: second_namespace,
        ..
    } = fleet.expect(FIRST_LINK_TIMEOUT, "the restarted node's link", |event| {
        matches!(event, Event::Linked { connection: 2, .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(relinked_with, certificate);
    assert_ne!(second_namespace, first_namespace);
    assert_eq!(
        fleet.provisioning_count(),
        3,
        "the restarted node provisioned again: {:#?}",
        fleet.seen
    );
}

#[test]
fn renewal_replaces_the_key_and_certificate_and_links_again() {
    let mut fleet = Fleet::start(
        "renewal",
        Behaviour {
            certificate_lifetime_seconds: 16,
            links: vec![LinkMode::Serve, LinkMode::StoredThenServe],
            ..Behaviour::default()
        },
    );
    let _node = fleet.start_node("");
    let Event::Enrolled {
        certificate: first,
        public_key: first_key,
        ..
    } = fleet.expect(FIRST_LINK_TIMEOUT, "enrollment", |event| {
        matches!(event, Event::Enrolled { .. })
    })
    else {
        unreachable!()
    };
    fleet.expect(Duration::from_secs(10), "link", |event| {
        matches!(event, Event::Linked { connection: 1, certificate, .. } if *certificate == first)
    });
    let Event::Renewed {
        presented,
        certificate: second,
        public_key: second_key,
    } = fleet.expect(Duration::from_secs(20), "renewal", |event| {
        matches!(event, Event::Renewed { .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(presented, first);
    assert_ne!(second, first);
    assert_ne!(second_key, first_key);
    fleet.expect(Duration::from_secs(10), "the old link closing", |event| {
        matches!(event, Event::LinkClosed { connection: 1, .. })
    });
    fleet.expect(Duration::from_secs(10), "a link with the renewed certificate", |event| {
        matches!(event, Event::Linked { connection: 2, certificate, .. } if *certificate == second)
    });
    let stored = fleet.stored(2);
    assert_eq!(stored.certificate(), second);
    assert_eq!(stored.public_key(), second_key);
    assert!(stored.staged_private_key.is_none(), "{stored:?}");
}

#[test]
fn two_connect_processes_on_one_node_enroll_once_and_renew_once() {
    let mut fleet = Fleet::start(
        "two-processes",
        Behaviour {
            certificate_lifetime_seconds: 16,
            ..Behaviour::default()
        },
    );
    let command = fleet.command("");
    let _node = start_node(
        format!("sh -d \"{command}\"; {command}"),
        Some(fleet.kvs_file()),
    );
    let mut linked = Vec::new();
    while linked.len() < 2 {
        let Event::Linked {
            certificate,
            namespace_id,
            ..
        } = fleet.expect(FIRST_LINK_TIMEOUT, "both processes linking", |event| {
            matches!(event, Event::Linked { .. })
        })
        else {
            unreachable!()
        };
        linked.push((certificate, namespace_id));
    }
    assert_eq!(
        linked[0], linked[1],
        "the processes linked with two identities or from two namespaces"
    );
    let first = linked[0].0.clone();
    let Event::Renewed {
        certificate: second,
        ..
    } = fleet.expect(
        Duration::from_secs(20),
        "renewal",
        |event| matches!(event, Event::Renewed { presented, .. } if *presented == first),
    )
    else {
        unreachable!()
    };
    let mut relinked = 0;
    while relinked < 2 {
        fleet.expect(
            Duration::from_secs(15),
            "both processes linking with the renewed certificate",
            |event| matches!(event, Event::Linked { certificate, .. } if *certificate == second),
        );
        relinked += 1;
    }
    let enrollments = fleet
        .seen
        .iter()
        .filter(|event| matches!(event, Event::Assigned { .. } | Event::Enrolled { .. }))
        .count();
    assert_eq!(enrollments, 2, "{:#?}", fleet.seen);
    let renewals_of_the_first = fleet
        .seen
        .iter()
        .filter(|event| matches!(event, Event::Renewed { presented, .. } if *presented == first))
        .count();
    assert_eq!(renewals_of_the_first, 1, "{:#?}", fleet.seen);
}

#[test]
fn a_dropped_link_reconnects() {
    let mut fleet = Fleet::start(
        "drop",
        Behaviour {
            links: vec![LinkMode::DropAfterLinked],
            ..Behaviour::default()
        },
    );
    let _node = fleet.start_node("");
    let Event::Linked {
        namespace_id,
        certificate,
        ..
    } = fleet.expect(FIRST_LINK_TIMEOUT, "link", |event| {
        matches!(event, Event::Linked { connection: 1, .. })
    })
    else {
        unreachable!()
    };
    fleet.expect(
        Duration::from_secs(5),
        "the dropped link closing",
        |event| matches!(event, Event::LinkClosed { connection: 1, .. }),
    );
    let Event::Linked {
        namespace_id: relinked_namespace,
        certificate: relinked_with,
        ..
    } = fleet.expect(Duration::from_secs(10), "the reconnected link", |event| {
        matches!(event, Event::Linked { connection: 2, .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(relinked_namespace, namespace_id);
    assert_eq!(relinked_with, certificate);
    fleet.quiet(
        Duration::from_secs(2),
        "provisioning after a dropped link",
        |event| matches!(event, Event::AssignAttempted | Event::Renewed { .. }),
    );
}

#[test]
fn a_silent_link_is_closed_after_the_heartbeat_timeout() {
    let mut fleet = Fleet::start(
        "heartbeat",
        Behaviour {
            links: vec![LinkMode::Silent],
            ..Behaviour::default()
        },
    );
    let _node = fleet.start_node(" --heartbeat-timeout 2");
    let Event::LinkClosed { lived, .. } =
        fleet.expect(FIRST_LINK_TIMEOUT, "the silent link closing", |event| {
            matches!(event, Event::LinkClosed { connection: 1, .. })
        })
    else {
        unreachable!()
    };
    assert!(
        lived >= Duration::from_millis(1900),
        "closed after {lived:?}"
    );
    assert!(lived < Duration::from_secs(6), "closed after {lived:?}");
    fleet.expect(Duration::from_secs(10), "the next link", |event| {
        matches!(event, Event::Linked { connection: 2, .. })
    });
}

#[test]
fn traffic_keeps_a_link_open_past_the_heartbeat_timeout() {
    let mut fleet = Fleet::start("heartbeat-traffic", Behaviour::default());
    let _node = fleet.start_node(" --heartbeat-timeout 2");
    fleet.expect(FIRST_LINK_TIMEOUT, "link", |event| {
        matches!(event, Event::Linked { connection: 1, .. })
    });
    fleet.quiet(
        Duration::from_secs(8),
        "a link closing while the fleet talks on it",
        |event| matches!(event, Event::LinkClosed { .. }),
    );
}

#[test]
fn an_install_token_is_the_credential_when_one_is_given() {
    let mut fleet = Fleet::start("install-token", Behaviour::default());
    let token_file = fleet.directory.join("install-token");
    std::fs::write(&token_file, format!("{INSTALL_TOKEN}\n")).unwrap();
    let _node = fleet.start_node(&format!(" --install-token-file {}", token_file.display()));
    let Event::Assigned {
        installation_id,
        credential,
        ..
    } = fleet.expect(FIRST_LINK_TIMEOUT, "assignment", |event| {
        matches!(event, Event::Assigned { .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(credential, "install");
    let Event::Enrolled { certificate, .. } =
        fleet.expect(Duration::from_secs(10), "enrollment", |event| {
            matches!(event, Event::Enrolled { installation_id: enrolled, .. } if *enrolled == installation_id)
        })
    else {
        unreachable!()
    };
    fleet.expect(Duration::from_secs(10), "link", |event| {
        matches!(event, Event::Linked { connection: 1, certificate: linked, .. } if *linked == certificate)
    });
}
