use crate::python;
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};

#[test]
fn test_a_host_name_connects() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python(&format!(
        r#"
import dusk
node = dusk.Dusk("localhost", {port})
assert list(node.sh("hostname"))
node.disconnect()
print("by name")
"#
    ));
    assert!(ok, "{output}");
    assert!(output.contains("by name"), "{output}");
}

#[test]
fn test_an_ipv6_address_connects() {
    let port = gen_port();
    let _node = DuskNixImpl::new("[::1]", port);
    let (ok, output) = python(&format!(
        r#"
import dusk
node = dusk.Dusk("::1", {port})
assert list(node.sh("hostname"))
node.disconnect()
print("over IPv6")
"#
    ));
    assert!(ok, "{output}");
    assert!(output.contains("over IPv6"), "{output}");
}

#[test]
fn test_a_refused_connection_names_its_kind_first() {
    let port = gen_port();
    let (ok, output) = python(&format!(
        r#"
import dusk
for address in ["{LISTEN_ADDRESS}", "no-such-host.invalid"]:
    try:
        dusk.Dusk(address, {port})
    except RuntimeError as error:
        assert str(error).startswith("Disconnected: "), error
    else:
        raise AssertionError(f"connected to {{address}}:{port}")
print("refused")
"#
    ));
    assert!(ok, "{output}");
    assert!(output.contains("refused"), "{output}");
}
