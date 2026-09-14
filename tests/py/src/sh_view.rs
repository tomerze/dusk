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
