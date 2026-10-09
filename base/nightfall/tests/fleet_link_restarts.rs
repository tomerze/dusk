#[path = "support/fake_nightfall.rs"]
mod fake_nightfall;

use std::time::Duration;

use fake_nightfall::*;

fn replace_and_restart(
    name: &str,
    behaviour: Behaviour,
    validity_seconds: (i64, i64),
) -> (Fleet, String, String) {
    let mut fleet = Fleet::start(
        name,
        Behaviour {
            links: vec![
                LinkMode::ReplaceCertificateThenStopNode { validity_seconds },
                LinkMode::StoredThenServe,
            ],
            ..behaviour
        },
    );
    let node = fleet.start_node("");
    let Event::Assigned {
        installation_id, ..
    } = fleet.expect(FIRST_LINK_TIMEOUT, "assignment", |event| {
        matches!(event, Event::Assigned { .. })
    })
    else {
        unreachable!()
    };
    let Event::Replaced { certificate } = fleet.expect(
        Duration::from_secs(20),
        "the replaced certificate",
        |event| matches!(event, Event::Replaced { .. }),
    ) else {
        unreachable!()
    };
    fleet.expect(Duration::from_secs(10), "the node stopping", |event| {
        matches!(event, Event::Stopped)
    });
    join_stopped(node);
    drop(fleet.start_node(""));
    (fleet, installation_id, certificate)
}

#[test]
fn an_expired_certificate_within_grace_is_renewed_instead_of_enrolling() {
    let now = now_seconds();
    let (mut fleet, installation_id, expired) =
        replace_and_restart("expired", Behaviour::default(), (now - 7200, now - 3600));
    let Event::Renewed {
        presented,
        certificate,
        ..
    } = fleet.expect(FIRST_LINK_TIMEOUT, "renewal", |event| {
        matches!(event, Event::Renewed { .. } | Event::AssignAttempted)
    })
    else {
        panic!("the node enrolled instead of renewing: {:#?}", fleet.seen)
    };
    assert_eq!(presented, expired);
    fleet.expect(Duration::from_secs(10), "a link with the renewed certificate", |event| {
        matches!(event, Event::Linked { connection: 2, certificate: linked, .. } if *linked == certificate)
    });
    let stored = fleet.stored(2);
    assert_eq!(stored.installation(), installation_id);
    assert_eq!(stored.certificate(), certificate);
}

#[test]
fn a_certificate_expired_beyond_grace_enrolls_a_new_installation() {
    let now = now_seconds();
    let (mut fleet, old_installation, expired) = replace_and_restart(
        "beyond-grace",
        Behaviour {
            refuse_renew_as_beyond_grace: true,
            ..Behaviour::default()
        },
        (now - 7200, now - 3600),
    );
    let Event::RenewRefused { presented } =
        fleet.expect(FIRST_LINK_TIMEOUT, "a refused renewal", |event| {
            matches!(event, Event::RenewRefused { .. } | Event::AssignAttempted)
        })
    else {
        panic!(
            "the node enrolled without trying to renew: {:#?}",
            fleet.seen
        )
    };
    assert_eq!(presented, expired);
    let Event::Assigned {
        installation_id,
        report,
        ..
    } = fleet.expect(Duration::from_secs(10), "assignment", |event| {
        matches!(event, Event::Assigned { .. })
    })
    else {
        unreachable!()
    };
    assert_ne!(installation_id, old_installation);
    assert_eq!(report.installation_hint, old_installation);
    let Event::Enrolled { certificate, .. } =
        fleet.expect(Duration::from_secs(10), "enrollment", |event| {
            matches!(event, Event::Enrolled { .. })
        })
    else {
        unreachable!()
    };
    fleet.expect(Duration::from_secs(10), "link", |event| {
        matches!(event, Event::Linked { connection: 2, certificate: linked, .. } if *linked == certificate)
    });
    assert_eq!(fleet.stored(2).installation(), installation_id);
}

#[test]
fn a_certificate_past_its_renewal_point_is_renewed_before_the_first_link() {
    let now = now_seconds();
    let (mut fleet, installation_id, current) = replace_and_restart(
        "renew-at-start",
        Behaviour::default(),
        (now - 3000, now + 600),
    );
    let Event::Renewed {
        presented,
        certificate,
        ..
    } = fleet.expect(FIRST_LINK_TIMEOUT, "renewal", |event| {
        matches!(
            event,
            Event::Renewed { .. } | Event::Linked { connection: 2, .. } | Event::AssignAttempted
        )
    })
    else {
        panic!(
            "the node linked or enrolled before renewing: {:#?}",
            fleet.seen
        )
    };
    assert_eq!(presented, current);
    fleet.expect(Duration::from_secs(10), "a link with the renewed certificate", |event| {
        matches!(event, Event::Linked { connection: 2, certificate: linked, .. } if *linked == certificate)
    });
    let stored = fleet.stored(2);
    assert_eq!(stored.installation(), installation_id);
    assert_eq!(stored.certificate(), certificate);
}
