#![allow(unused_imports)]

use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};
use std::process::Command;

#[allow(dead_code)]
fn run_python_code(code: &str) -> bool {
    let python = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.venv/bin/python");
    let status = Command::new(python)
        .arg("-c")
        .arg(code)
        .status()
        .expect("failed to run python code");

    status.success()
}

#[test]
fn test_sanity() {
    let port = gen_port();
    let address = LISTEN_ADDRESS;
    let _dusk = DuskNixImpl::new(address, port);

    let code = format!(
        r#"
import dusk
client = dusk.Dusk("{address}", {port})
client.disconnect()
"#,
        address = address,
        port = port
    );

    assert!(run_python_code(&code));
}

#[test]
fn test_multiple_clients_same_server() {
    let port = gen_port();
    let address = LISTEN_ADDRESS;
    let _dusk = DuskNixImpl::new(address, port);

    let code = format!(
        r#"
import dusk
client = dusk.Dusk("{address}", {port})
another_client = dusk.Dusk("{address}", {port})
client.disconnect()
another_client.disconnect()
"#,
        address = address,
        port = port
    );

    assert!(run_python_code(&code));
}
