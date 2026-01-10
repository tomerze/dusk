#![allow(unused_imports)]

use dusk_tests::{gen_port, DuskNixImpl, LISTEN_ADDR};
use std::process::Command;
use std::sync::OnceLock;

static DUSK_PY_INSTALLED: OnceLock<()> = OnceLock::new();

#[allow(dead_code)]
fn ensure_dusk_py_installed() {
    DUSK_PY_INSTALLED.get_or_init(|| {
        // Build the Python extension once using maturin.
        // The uv/cargo lock mechanisms ensure only one build happens at a time.
        let status = std::process::Command::new("uv")
            .arg("run")
            .arg("maturin")
            .arg("develop")
            .arg("--uv")
            .arg("-m")
            .arg("artifacts/dusk_py/Cargo.toml")
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .status()
            .expect("Failed to run maturin develop");

        assert!(status.success(), "maturin develop failed");
    });
}

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
    // Build/install the Python extension once for all tests.
    ensure_dusk_py_installed();

    let port = gen_port();
    let address = LISTEN_ADDR;
    let _dusk = DuskNixImpl::new(address, port);

    let code = format!(
        r#"
import dusk
client = dusk.Dusk("{addr}", {port})
client.disconnect()
"#,
        addr = address,
        port = port
    );

    assert!(run_python_code(&code), "python api sanity test failed");
}
