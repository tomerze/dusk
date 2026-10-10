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
