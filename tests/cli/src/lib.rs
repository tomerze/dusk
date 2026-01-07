#![allow(unused_imports)]

use assert_cmd::assert::OutputAssertExt;
use dusk_tests::{gen_port, get_dusk_cli_bin, DuskNixImpl, LISTEN_ADDR};
use predicates::prelude::*;
use std::process::Command;
use std::thread;

#[test]
fn test_run_ps() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDR, port);

    let bin_path = get_dusk_cli_bin();

    let mut cmd = Command::new(bin_path);
    cmd.arg(format!("{}:{}", LISTEN_ADDR, port))
        .arg("ps")
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("ps"))
        .stdout(predicate::str::contains("sh"))
        .stdout(predicate::str::contains("program_id"));
}

#[test]
fn test_multiple_ps_calls() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDR, port);

    let bin_path = get_dusk_cli_bin();

    // Run ps multiple times on the same server
    for i in 0..3 {
        let mut cmd = Command::new(&bin_path);
        let output = cmd
            .arg(format!("{}:{}", LISTEN_ADDR, port))
            .arg("ps")
            .assert()
            .success()
            .stdout(predicate::str::contains("init"))
            .stdout(predicate::str::contains("sh"))
            .stdout(predicate::str::contains("program_id"))
            .get_output()
            .stdout
            .clone();

        let output_str = String::from_utf8_lossy(&output);

        // Each call should show exactly one init
        let init_count = output_str.matches("init").count();
        assert_eq!(
            init_count, 1,
            "Expected exactly 1 init process on iteration {}, found {}",
            i, init_count
        );

        // Should show one sh for the connected client
        let sh_count = output_str.matches("sh").filter(|&m| m == "sh").count();
        assert!(
            sh_count >= 1,
            "Expected at least 1 sh process on iteration {}, found {}",
            i,
            sh_count
        );
    }
}

#[test]
fn test_concurrent_connections() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDR, port);

    let bin_path = get_dusk_cli_bin();

    // Simulate multiple concurrent clients connecting
    let handles: Vec<_> = (0..3)
        .map(|_| {
            let bin_path = bin_path.clone();
            thread::spawn(move || {
                let mut cmd = Command::new(bin_path);
                cmd.arg(format!("{}:{}", LISTEN_ADDR, port))
                    .arg("ps")
                    .assert()
                    .success();
            })
        })
        .collect();

    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }

    // After concurrent connections, verify state is still consistent
    let mut cmd = Command::new(&bin_path);
    let output = cmd
        .arg(format!("{}:{}", LISTEN_ADDR, port))
        .arg("ps")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output_str = String::from_utf8_lossy(&output);

    // Should still have exactly one init
    let init_count = output_str.matches("init").count();
    assert_eq!(
        init_count, 1,
        "After concurrent connections, should still have exactly 1 init, found {}",
        init_count
    );
}

#[test]
fn test_ps_output_structure() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDR, port);

    let bin_path = get_dusk_cli_bin();

    let mut cmd = Command::new(&bin_path);
    let output = cmd
        .arg(format!("{}:{}", LISTEN_ADDR, port))
        .arg("ps")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output_str = String::from_utf8_lossy(&output);

    // Verify output contains expected structure
    assert!(
        output_str.contains("program_id"),
        "Output should contain program_id header"
    );
    assert!(
        output_str.contains("init"),
        "Output should contain init process"
    );
    assert!(
        output_str.contains("sh"),
        "Output should contain sh process"
    );

    // Count exact occurrences of init (should be exactly 1)
    let init_count = output_str.matches("init").count();
    assert_eq!(
        init_count, 1,
        "Expected exactly 1 init process, found {}",
        init_count
    );

    // There should be at least one sh (for the client connection)
    let sh_count = output_str.matches("sh").filter(|&m| m == "sh").count();
    assert!(
        sh_count >= 1,
        "Expected at least 1 sh process, found {}",
        sh_count
    );
}
