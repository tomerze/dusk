#[path = "support/fake_nightfall.rs"]
mod fake_nightfall;
#[path = "support/swtpm.rs"]
mod swtpm;

use std::time::Duration;

use dusk_program::value::Value;
use fake_nightfall::*;
use swtpm::Swtpm;

fn tpm_key(stored: &Stored) -> Vec<u8> {
    let Some((Value::List(parts), flags)) = &stored.private_key else {
        panic!("no TPM key is stored: {stored:?}");
    };
    assert_eq!(*flags, KEPT_SECRET);
    let [Value::Bytes(_), Value::Bytes(public)] = parts.as_slice() else {
        panic!("the stored TPM key is not a private and a public area: {stored:?}");
    };
    public.clone()
}

#[test]
fn a_node_with_a_tpm_keeps_its_key_there_and_reloads_it_after_the_tpm_resets() {
    let Some(tpm) = Swtpm::start() else {
        return;
    };
    let mut fleet = Fleet::start(
        "tpm-reload",
        Behaviour {
            links: vec![LinkMode::StoredThenStopNode],
            ..Behaviour::default()
        },
    );
    fleet.tpm = Some(tpm.socket());
    let node = fleet.start_node("");
    let Event::Enrolled { certificate, .. } =
        fleet.expect(FIRST_LINK_TIMEOUT, "enrollment", |event| {
            matches!(event, Event::Enrolled { .. })
        })
    else {
        unreachable!()
    };
    fleet.expect(Duration::from_secs(10), "link", |event| {
        matches!(event, Event::Linked { connection: 1, certificate: linked, .. } if *linked == certificate)
    });
    let stored = fleet.stored(1);
    tpm_key(&stored);
    assert_eq!(stored.certificate(), certificate);
    assert!(stored.staged_private_key.is_none(), "{stored:?}");
    fleet.expect(Duration::from_secs(10), "the node stopping", |event| {
        matches!(event, Event::Stopped)
    });
    join_stopped(node);

    tpm.reset();
    let _restarted = fleet.start_node("");
    fleet.expect(
        FIRST_LINK_TIMEOUT,
        "the restarted node's link",
        |event| matches!(event, Event::Linked { connection: 2, certificate: linked, .. } if *linked == certificate),
    );
    assert_eq!(
        fleet.provisioning_count(),
        3,
        "the restarted node provisioned again: {:#?}",
        fleet.seen
    );
}

#[test]
fn renewal_replaces_a_tpm_key_with_a_new_key_in_the_same_tpm() {
    let Some(tpm) = Swtpm::start() else {
        return;
    };
    let mut fleet = Fleet::start(
        "tpm-renewal-new-key",
        Behaviour {
            certificate_lifetime_seconds: 16,
            links: vec![LinkMode::StoredThenServe, LinkMode::StoredThenServe],
            ..Behaviour::default()
        },
    );
    fleet.tpm = Some(tpm.socket());
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
    let first_stored = fleet.stored(1);
    let Event::Renewed {
        presented,
        certificate: second,
        public_key: second_key,
        ..
    } = fleet.expect(Duration::from_secs(20), "renewal", |event| {
        matches!(event, Event::Renewed { .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(presented, first);
    assert_ne!(second_key, first_key);
    fleet.expect(
        Duration::from_secs(10),
        "a link with the renewed certificate",
        |event| matches!(event, Event::Linked { connection: 2, certificate, .. } if *certificate == second),
    );
    let second_stored = fleet.stored(2);
    assert_eq!(second_stored.certificate(), second);
    assert_ne!(tpm_key(&second_stored), tpm_key(&first_stored));
    assert!(
        second_stored.staged_private_key.is_none(),
        "{second_stored:?}"
    );
}

#[test]
fn a_node_with_a_tpm_attests_its_key() {
    let Some(tpm) = Swtpm::start() else {
        return;
    };
    let mut fleet = Fleet::start(
        "tpm-attest",
        Behaviour {
            attest_tpm: true,
            links: vec![LinkMode::StoredThenServe],
            ..Behaviour::default()
        },
    );
    fleet.tpm = Some(tpm.socket());
    let _node = fleet.start_node("");
    let Event::Assigned {
        installation_id,
        report,
        ..
    } = fleet.expect(FIRST_LINK_TIMEOUT, "assignment", |event| {
        matches!(event, Event::Assigned { .. })
    })
    else {
        unreachable!()
    };
    let evidence = report
        .tpm
        .clone()
        .expect("the device report carries the TPM's evidence");
    assert_eq!(
        report.hardware_fingerprint,
        sha256(&evidence.endorsement_key)
    );
    assert_eq!(
        evidence.endorsement_certificate_chain,
        vec![tpm.issuer_certificate()]
    );
    let Event::Enrolled {
        certificate,
        tpm_bound,
        ..
    } = fleet.expect(Duration::from_secs(10), "enrollment", |event| {
        matches!(event, Event::Enrolled { installation_id: enrolled, .. } if *enrolled == installation_id)
    })
    else {
        unreachable!()
    };
    assert!(tpm_bound);
    fleet.expect(Duration::from_secs(10), "link", |event| {
        matches!(event, Event::Linked { connection: 1, certificate: linked, .. } if *linked == certificate)
    });
    let stored = fleet.stored(1);
    assert_eq!(tpm_key(&stored), evidence.node_key);
    assert!(stored.hardware_fingerprint.is_none(), "{stored:?}");
}

#[test]
fn renewing_a_tpm_bound_certificate_keeps_its_key() {
    let Some(tpm) = Swtpm::start() else {
        return;
    };
    let mut fleet = Fleet::start(
        "tpm-renewal",
        Behaviour {
            certificate_lifetime_seconds: 16,
            attest_tpm: true,
            links: vec![LinkMode::StoredThenServe, LinkMode::StoredThenServe],
            ..Behaviour::default()
        },
    );
    fleet.tpm = Some(tpm.socket());
    let _node = fleet.start_node("");
    let Event::Enrolled {
        certificate: first,
        public_key: first_key,
        tpm_bound: true,
        ..
    } = fleet.expect(FIRST_LINK_TIMEOUT, "enrollment", |event| {
        matches!(event, Event::Enrolled { .. })
    })
    else {
        panic!("the enrollment is not TPM-bound: {:#?}", fleet.seen);
    };
    let first_stored = fleet.stored(1);
    let Event::Renewed {
        presented,
        certificate: second,
        public_key: second_key,
        tpm_bound,
    } = fleet.expect(Duration::from_secs(20), "renewal", |event| {
        matches!(event, Event::Renewed { .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(presented, first);
    assert_ne!(second, first);
    assert_eq!(second_key, first_key);
    assert!(tpm_bound);
    fleet.expect(
        Duration::from_secs(10),
        "a link with the renewed certificate",
        |event| matches!(event, Event::Linked { connection: 2, certificate, .. } if *certificate == second),
    );
    let second_stored = fleet.stored(2);
    assert_eq!(second_stored.certificate(), second);
    assert_eq!(second_stored.private_key, first_stored.private_key);
    assert!(
        second_stored.staged_private_key.is_none(),
        "{second_stored:?}"
    );
}

#[test]
fn a_credential_made_for_another_key_is_never_answered() {
    let Some(tpm) = Swtpm::start() else {
        return;
    };
    let mut fleet = Fleet::start(
        "tpm-foreign-credential",
        Behaviour {
            attest_tpm: true,
            credential_for_another_key: true,
            ..Behaviour::default()
        },
    );
    fleet.tpm = Some(tpm.socket());
    let _node = fleet.start_node("");
    for attempt in 1..=2 {
        fleet.expect(
            FIRST_LINK_TIMEOUT,
            "an assignment",
            |event| matches!(event, Event::Assigned { report, .. } if report.tpm.is_some()),
        );
        assert!(
            fleet
                .seen
                .iter()
                .all(|event| !matches!(event, Event::Enrolled { .. })),
            "attempt {attempt}: {:#?}",
            fleet.seen
        );
    }
    fleet.quiet(Duration::from_secs(5), "an enrollment", |event| {
        matches!(event, Event::Enrolled { .. })
    });
}
