#![allow(unused_imports)]

use assert_cmd::assert::OutputAssertExt;
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};
use lazy_static::lazy_static;
use predicates::prelude::*;
use rexpect::process::wait::WaitStatus;
use rexpect::spawn;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

lazy_static! {
    static ref DUSK_CLI_BIN: PathBuf = {
        Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/debug/dusk"
        ))
        .to_path_buf()
    };
}

pub fn get_dusk_cli_bin() -> &'static std::path::Path {
    &DUSK_CLI_BIN
}

#[test]
fn test_run_ps() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let bin_path = get_dusk_cli_bin();

    let mut cmd = Command::new(bin_path);
    cmd.arg(format!("{}:{}", LISTEN_ADDRESS, port))
        .arg("ps")
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("sh"))
        .stdout(predicate::str::contains("program_id"));
}

#[test]
fn test_multiple_ps_calls() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let bin_path = get_dusk_cli_bin();

    // Run ps multiple times on the same server
    for i in 0..3 {
        let mut cmd = Command::new(bin_path);
        let output = cmd
            .arg(format!("{}:{}", LISTEN_ADDRESS, port))
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
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let bin_path = get_dusk_cli_bin();

    // Simulate multiple concurrent clients connecting
    let handles: Vec<_> = (0..3)
        .map(|_| {
            thread::spawn(move || {
                let mut cmd = Command::new(bin_path);
                cmd.arg(format!("{}:{}", LISTEN_ADDRESS, port))
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
    let mut cmd = Command::new(bin_path);
    let output = cmd
        .arg(format!("{}:{}", LISTEN_ADDRESS, port))
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
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let bin_path = get_dusk_cli_bin();

    let mut cmd = Command::new(bin_path);
    let output = cmd
        .arg(format!("{}:{}", LISTEN_ADDRESS, port))
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

#[test]
fn test_multiple_servers_parallel_connections() {
    // Create multiple servers on different ports
    let servers: Vec<_> = (0..3)
        .map(|_| {
            let port = gen_port();
            let server = DuskNixImpl::new(LISTEN_ADDRESS, port);
            (server, port)
        })
        .collect();

    let bin_path = get_dusk_cli_bin();

    // Connect to each server in parallel
    let handles: Vec<_> = servers
        .iter()
        .enumerate()
        .map(|(i, (_server, port))| {
            let port = *port;
            thread::spawn(move || {
                // Run ps command on each server
                let mut cmd = Command::new(bin_path);
                let output = cmd
                    .arg(format!("{}:{}", LISTEN_ADDRESS, port))
                    .arg("ps")
                    .assert()
                    .success()
                    .get_output()
                    .stdout
                    .clone();

                let output_str = String::from_utf8_lossy(&output);

                // Verify we got ps output from this server
                assert!(
                    output_str.contains("program_id"),
                    "Server {} should show program_id in ps output",
                    i
                );
                assert!(
                    output_str.contains("init"),
                    "Server {} should have init process",
                    i
                );

                // Each server should have exactly one init
                let init_count = output_str.matches("init").count();
                assert_eq!(
                    init_count, 1,
                    "Server {} should have exactly 1 init, found {}",
                    i, init_count
                );
            })
        })
        .collect();

    // Wait for all parallel connections to complete
    for handle in handles {
        handle.join().unwrap();
    }

    // After parallel connections, verify each server is still functional
    // by connecting to them all again in parallel
    let handles: Vec<_> = servers
        .iter()
        .enumerate()
        .map(|(i, (_server, port))| {
            let port = *port;
            thread::spawn(move || {
                let mut cmd = Command::new(bin_path);
                let output = cmd
                    .arg(format!("{}:{}", LISTEN_ADDRESS, port))
                    .arg("ps")
                    .assert()
                    .success()
                    .get_output()
                    .stdout
                    .clone();

                let output_str = String::from_utf8_lossy(&output);

                // Verify server still has exactly one init after multiple connections
                let init_count = output_str.matches("init").count();
                assert_eq!(
                    init_count, 1,
                    "Server {} should still have exactly 1 init after second connection, found {}",
                    i, init_count
                );
            })
        })
        .collect();

    // Wait for second round
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_interactive_shell() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let bin_path = get_dusk_cli_bin();

    // Spawn interactive shell with PTY using rexpect
    let mut p = spawn(
        &format!("{} {}:{}", bin_path.display(), LISTEN_ADDRESS, port),
        Some(5000),
    )
    .expect("Failed to spawn interactive shell");

    // Verify shell is running by sending a command
    p.send_line("ps").expect("Failed to send ps command");

    // Send exit command
    p.send_line("exit").expect("Failed to send exit command");

    // Wait for process to exit cleanly - this verifies it worked
    let wait_result = p.process.wait().expect("Failed to wait for process");
    // WaitStatus::Exited(_, 0) means successful exit
    match wait_result {
        WaitStatus::Exited(_, 0) => {
            // Success!
        }
        other => {
            panic!("Process should exit with status 0, got: {:?}", other);
        }
    }
}

#[test]
fn test_two_clients_same_server() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    let bin_path = get_dusk_cli_bin();

    // Spawn two clients connecting to the same server
    let mut client1 = spawn(
        &format!("{} {}:{}", bin_path.display(), LISTEN_ADDRESS, port),
        Some(5000),
    )
    .expect("Failed to spawn first client");

    let mut client2 = spawn(
        &format!("{} {}:{}", bin_path.display(), LISTEN_ADDRESS, port),
        Some(5000),
    )
    .expect("Failed to spawn second client");

    // Verify both clients are working
    client1
        .send_line("ps")
        .expect("Failed to send ps to client1");
    client2
        .send_line("ps")
        .expect("Failed to send ps to client2");

    // Exit both clients
    client1
        .send_line("exit")
        .expect("Failed to send exit to client1");
    client2
        .send_line("exit")
        .expect("Failed to send exit to client2");

    // Verify both exited cleanly
    let wait1 = client1.process.wait().expect("Failed to wait for client1");
    match wait1 {
        WaitStatus::Exited(_, 0) => {}
        other => panic!("Client1 should exit with status 0, got: {:?}", other),
    }

    let wait2 = client2.process.wait().expect("Failed to wait for client2");
    match wait2 {
        WaitStatus::Exited(_, 0) => {}
        other => panic!("Client2 should exit with status 0, got: {:?}", other),
    }
}

#[test]
fn test_multiple_interactive_shells_parallel() {
    // Create multiple servers
    let servers: Vec<_> = (0..3)
        .map(|_| {
            let port = gen_port();
            let server = DuskNixImpl::new(LISTEN_ADDRESS, port);
            (server, port)
        })
        .collect();

    let bin_path = get_dusk_cli_bin();

    // Spawn all interactive shells first (connect all three)
    let mut processes: Vec<_> = servers
        .iter()
        .map(|(_server, port)| {
            spawn(
                &format!("{} {}:{}", bin_path.display(), LISTEN_ADDRESS, port),
                Some(5000),
            )
            .expect("Failed to spawn interactive shell")
        })
        .collect();

    // Verify shells are working by sending commands
    for p in processes.iter_mut() {
        p.send_line("ps").expect("Failed to send ps command");
    }

    // Now disconnect them all by sending exit
    for p in processes.iter_mut() {
        p.send_line("exit").expect("Failed to send exit command");
    }

    // Wait for all to exit cleanly - this verifies they worked
    for process in processes {
        let wait_result = process.process.wait().expect("Failed to wait for process");
        // WaitStatus::Exited(_, 0) means successful exit
        match wait_result {
            WaitStatus::Exited(_, 0) => {
                // Success!
            }
            other => {
                panic!("Process should exit with status 0, got: {:?}", other);
            }
        }
    }
}
