use crate::get_dusk_cli_bin;
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};
use std::process::Command;
use std::time::{Duration, Instant};

fn node_address(port: u16) -> String {
    format!("{}:{}", LISTEN_ADDRESS, port)
}

fn run(port: u16, command: &str) -> String {
    let output = Command::new(get_dusk_cli_bin())
        .arg(node_address(port))
        .arg(command)
        .env("DUSK_NON_INTERACTIVE", "1")
        .output()
        .expect("run a dusk command");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn pid_of(port: u16, name: &str) -> u64 {
    let table = run(port, "ps");
    for line in table.lines() {
        if !line.contains("\"Name\"") {
            continue;
        }
        let names: Vec<&str> = line
            .split("\"Name\":[")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .expect("a Name column")
            .split(',')
            .collect();
        let pids: Vec<&str> = line
            .split("\"PID\":[")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .expect("a PID column")
            .split(',')
            .collect();
        for (index, entry) in names.iter().enumerate() {
            if entry.trim_matches('"') == name {
                return pids[index].trim().parse().expect("a numeric pid");
            }
        }
    }
    panic!("no `{name}` in ps:\n{table}");
}

fn count_of(port: u16, name: &str) -> usize {
    run(port, "ps").matches(name).count()
}

// ---------------------------------------------------------------- sh's forms

#[test]
fn a_bare_sh_says_what_to_use_instead() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    assert!(
        run(port, "sh").contains("sh takes a command, --server or --prompt"),
        "a bare sh should name the forms that work"
    );
}

#[test]
fn sh_server_starts_the_node_shell() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    assert!(run(port, "sh --server").contains("running in server mode"));
    assert_eq!(count_of(port, "sh[server]"), 1);
}

#[test]
fn sh_server_twice_attaches_rather_than_duplicating() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    run(port, "sh --server");
    run(port, "sh --server");
    run(port, "sh --server");
    assert_eq!(count_of(port, "sh[server]"), 1, "one shell at defaultPid");
}

#[test]
fn sh_server_takes_a_pid_in_decimal() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    run(port, "sh --server 4242");
    assert!(
        run(port, "ps").contains("4242"),
        "a shell at the pid asked for"
    );
}

#[test]
fn sh_server_takes_a_pid_in_hex() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    run(port, "sh --server 0x1092");
    assert!(
        run(port, "ps").contains("4242"),
        "0x1092 is 4242, the pid ps prints"
    );
}

#[test]
fn sh_server_at_two_pids_makes_two_shells() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    run(port, "sh --server");
    run(port, "sh --server 4242");
    assert_eq!(count_of(port, "sh[server]"), 2);
}

#[test]
fn a_prompt_is_refused_where_there_is_no_terminal() {
    let port = gen_port();
    let node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    node.expect_errors();
    assert!(
        run(port, "sh --prompt").contains("there is no terminal to open a prompt on"),
        "a refused prompt says why"
    );
}

#[test]
fn a_refused_prompt_leaves_no_process_behind() {
    let port = gen_port();
    let node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    node.expect_errors();
    for _ in 0..5 {
        run(port, "sh --prompt");
    }
    let table = run(port, "ps");
    assert!(
        !table.contains("\"S\""),
        "every refused prompt sweeps its own process:\n{table}"
    );
}

#[test]
fn a_refused_prompt_leaves_the_node_answering() {
    let port = gen_port();
    let node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    node.expect_errors();
    run(port, "sh --prompt 800");
    assert!(
        run(port, "hostname").contains("\""),
        "the node still answers after a refused prompt at an unused pid"
    );
}

#[test]
fn a_script_runs_a_command() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    assert!(run(port, "echo hello").contains("hello"));
}

#[test]
fn a_script_runs_two_statements() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let output = run(port, "echo one; echo two");
    assert!(output.contains("one") && output.contains("two"), "{output}");
}

#[test]
fn a_detached_script_leaves_the_node_alone() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    run(port, "sh -d 'echo detached'");
    assert!(run(port, "hostname").contains("\""));
}

#[test]
fn a_script_can_run_a_script() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    assert!(run(port, "sh 'echo nested'").contains("nested"));
}

// ------------------------------------------------------ kills and signals

#[test]
fn sweep_clears_a_process_that_never_ran() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    run(port, "sh --server 4242");
    let before = count_of(port, "sh[server]");
    run(port, "kill --signal 7 4242");
    assert_eq!(
        count_of(port, "sh[server]"),
        before,
        "sweep does nothing to a process that has run"
    );
}

#[test]
fn killing_a_pid_nobody_has_is_an_error_not_a_hang() {
    let port = gen_port();
    let node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    node.expect_errors();
    assert!(run(port, "kill 123456789").contains("couldn't find process"));
    assert!(run(port, "hostname").contains("\""), "the node is fine");
}

#[test]
fn a_terminated_command_is_reaped() {
    let port = gen_port();
    let node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    node.expect_errors();
    run(port, "sh -d 'sleep 60000'");
    std::thread::sleep(Duration::from_millis(500));
    let sleeping = pid_of(port, "sleep");
    run(port, &format!("kill {sleeping}"));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(count_of(port, "\"sleep\""), 0, "the killed program is gone");
}

// ------------------------------------------------------------- resilience

#[test]
fn a_burst_of_clients_does_not_take_the_node_down() {
    let port = gen_port();
    let node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    node.expect_errors();
    let mut clients = Vec::new();
    for _ in 0..25 {
        clients.push(
            Command::new(get_dusk_cli_bin())
                .arg(node_address(port))
                .arg("sleep 2000")
                .env("DUSK_NON_INTERACTIVE", "1")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("start a client"),
        );
    }
    for mut client in clients {
        let _ = client.wait();
    }
    assert!(
        run(port, "hostname").contains("\""),
        "the node survives more clients at once than it has session slots"
    );
}

#[test]
fn a_node_that_is_not_there_fails_rather_than_hanging() {
    let port = gen_port();
    let started = Instant::now();
    let output = Command::new(get_dusk_cli_bin())
        .arg(node_address(port))
        .arg("ps")
        .env("DUSK_NON_INTERACTIVE", "1")
        .output()
        .expect("run against nothing");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "a command against a node that is not there should not hang"
    );
    let reported = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(reported.contains("Disconnected"), "{reported}");
}

#[test]
fn commands_keep_working_after_many_of_them() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    for _ in 0..20 {
        assert!(run(port, "hostname").contains("\""));
    }
    assert_eq!(count_of(port, "sh[server]"), 0, "a script needs no shell");
}
