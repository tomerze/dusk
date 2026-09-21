use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};
use std::process::Command;
use std::time::{Duration, Instant};

fn python(code: &str) -> (bool, String) {
    let interpreter = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.venv/bin/python");
    let output = Command::new(interpreter)
        .arg("-c")
        .arg(code)
        .output()
        .expect("run python");
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn connected(port: u16, body: &str) -> (bool, String) {
    python(&format!(
        "import dusk\nnode = dusk.Dusk(\"{LISTEN_ADDRESS}\", {port})\n{body}\nnode.disconnect()\n"
    ))
}

#[test]
fn a_command_returns_its_values() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = connected(port, "print(list(node.sh(\"hostname\")))");
    assert!(ok, "{output}");
    assert!(output.contains('\''), "{output}");
}

#[test]
fn a_command_can_be_run_many_times_on_one_object() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = connected(
        port,
        "print([len(list(node.sh(\"hostname\"))) for _ in range(10)])",
    );
    assert!(ok, "{output}");
    assert!(
        output.contains("[1, 1, 1, 1, 1, 1, 1, 1, 1, 1]"),
        "{output}"
    );
}

#[test]
fn an_object_takes_a_shell_when_it_connects() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = connected(
        port,
        "row = list(node.sh(\"ps\"))[0]\n\
         table = row[list(row)[0]]\n\
         print(\"shells:\", table[\"Name\"].count(\"sh[server]\"))\n\
         print(\"scripts:\", table[\"Name\"].count(\"sh[script]\"))",
    );
    assert!(ok, "{output}");
    assert!(
        output.contains("shells: 1"),
        "the object holds one shell:\n{output}"
    );
    assert!(
        output.contains("scripts: 0"),
        "a command is not a script process any more:\n{output}"
    );
}

#[test]
fn a_function_defined_by_one_command_is_there_for_the_next() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = connected(
        port,
        "list(node.sh(\"greet() { echo greeted }\"))\n\
         print(\"greet ->\", list(node.sh(\"greet\")))",
    );
    assert!(ok, "{output}");
    assert!(
        output.contains("greeted"),
        "the shell keeps what a command defined in it:\n{output}"
    );
}

#[test]
fn an_object_can_hold_a_shell_of_its_own() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python(&format!(
        "import dusk\n\
         first = dusk.Dusk(\"{LISTEN_ADDRESS}\", {port})\n\
         second = dusk.Dusk(\"{LISTEN_ADDRESS}\", {port}, sh_server_pid=4242)\n\
         list(first.sh(\"mine() {{ echo mine }}\"))\n\
         row = list(second.sh(\"ps\"))[0]\n\
         table = row[list(row)[0]]\n\
         print(\"shells:\", table[\"Name\"].count(\"sh[server]\"))\n\
         try:\n\
         \x20   list(second.sh(\"mine\"))\n\
         \x20   print(\"crossed: yes\")\n\
         except RuntimeError as error:\n\
         \x20   print(\"crossed:\", \"no sh entry found\" in str(error))\n\
         first.disconnect()\n\
         second.disconnect()\n"
    ));
    assert!(ok, "{output}");
    assert!(
        output.contains("shells: 2"),
        "two shells, one each:\n{output}"
    );
    assert!(
        output.contains("crossed: True"),
        "one object's functions stay out of the other's shell:\n{output}"
    );
}

#[test]
fn a_shell_at_a_pid_asked_for_twice_is_one_shell() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = connected(
        port,
        "print(list(node.sh(\"sh --server 4242\")))\n\
         print(list(node.sh(\"sh --server 4242\")))\n\
         row = list(node.sh(\"ps\"))[0]\n\
         table = row[list(row)[0]]\n\
         print(\"shells at 4242:\", table[\"PID\"].count(4242))",
    );
    assert!(ok, "{output}");
    assert!(
        output.contains("shells at 4242: 1"),
        "asking twice attaches, it does not duplicate:\n{output}"
    );
}

#[test]
fn a_shell_at_a_hex_pid_is_the_same_shell() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = connected(
        port,
        "list(node.sh(\"sh --server 4242\"))\n\
         list(node.sh(\"sh --server 0x1092\"))\n\
         row = list(node.sh(\"ps\"))[0]\n\
         table = row[list(row)[0]]\n\
         print(\"shells at 4242:\", table[\"PID\"].count(4242))",
    );
    assert!(ok, "{output}");
    assert!(
        output.contains("shells at 4242: 1"),
        "0x1092 and 4242 are the same pid:\n{output}"
    );
}

#[test]
fn a_prompt_without_a_terminal_raises_rather_than_hanging() {
    let port = gen_port();
    let node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    node.expect_errors();
    let started = Instant::now();
    let (ok, output) = connected(port, "node.prompt()");
    assert!(!ok, "a prompt with nowhere to open should raise:\n{output}");
    assert!(
        output.contains("there is no terminal to open a prompt on"),
        "{output}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it should raise at once, not hang"
    );
}

#[test]
fn a_connection_to_nothing_raises() {
    let port = gen_port();
    let started = Instant::now();
    let (ok, output) = python(&format!(
        "import dusk\ndusk.Dusk(\"{LISTEN_ADDRESS}\", {port})\n"
    ));
    assert!(!ok, "connecting to nothing should raise:\n{output}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it should raise at once, not hang"
    );
}

#[test]
fn two_objects_share_one_node() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python(&format!(
        "import dusk\n\
         first = dusk.Dusk(\"{LISTEN_ADDRESS}\", {port})\n\
         second = dusk.Dusk(\"{LISTEN_ADDRESS}\", {port})\n\
         print(list(first.sh(\"hostname\")) == list(second.sh(\"hostname\")))\n\
         first.disconnect()\n\
         second.disconnect()\n"
    ));
    assert!(ok, "{output}");
    assert!(output.contains("True"), "{output}");
}

#[test]
fn a_command_after_disconnect_raises() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python(&format!(
        "import dusk\n\
         node = dusk.Dusk(\"{LISTEN_ADDRESS}\", {port})\n\
         node.disconnect()\n\
         node.sh(\"hostname\")\n"
    ));
    assert!(!ok, "using a closed connection should raise:\n{output}");
    assert!(output.contains("Connection is closed"), "{output}");
}

#[test]
fn a_command_runs_through_the_shell_not_a_script() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = connected(
        port,
        "for _ in range(5):\n\
         \x20   list(node.sh(\"hostname\"))\n\
         row = list(node.sh(\"ps\"))[0]\n\
         table = row[list(row)[0]]\n\
         print(\"scripts:\", table[\"Name\"].count(\"sh[script]\"))",
    );
    assert!(ok, "{output}");
    assert!(
        output.contains("scripts: 0"),
        "five commands leave no script processes:\n{output}"
    );
}

#[test]
fn python_sees_the_help_for_a_program() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python("import dusk\nprint(dusk.Dusk.help(\"sh\"))\n");
    assert!(ok, "{output}");
    assert!(
        output.contains("--prompt"),
        "help names the prompt form:\n{output}"
    );
}

#[test]
fn commands_on_one_object_run_at_the_same_time() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = connected(
        port,
        "import threading, time\n\
         never_read = node.sh(\"sleep 20000\")\n\
         print(\"hostname while a command runs:\", len(list(node.sh(\"hostname\"))))\n\
         finished = []\n\
         def run():\n\
         \x20   finished.append(list(node.sh(\"sleep 2000\")))\n\
         threads = [threading.Thread(target=run) for _ in range(4)]\n\
         started = time.monotonic()\n\
         for thread in threads:\n\
         \x20   thread.start()\n\
         for thread in threads:\n\
         \x20   thread.join(30)\n\
         elapsed = time.monotonic() - started\n\
         print(\"finished:\", len(finished))\n\
         print(\"in parallel:\", elapsed < 6, round(elapsed, 1))",
    );
    assert!(ok, "{output}");
    assert!(
        output.contains("hostname while a command runs: 1"),
        "a command still running does not hold the object:\n{output}"
    );
    assert!(
        output.contains("finished: 4"),
        "every command finished, none got stuck:\n{output}"
    );
    assert!(
        output.contains("in parallel: True"),
        "four 2-second sleeps ran side by side, not one after another:\n{output}"
    );
}
