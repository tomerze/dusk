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

#[test]
fn test_clients_run_on_the_worker_pool_instead_of_a_thread_each() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python(&format!(
        r#"
import os
os.environ["DUSK_PY_WORKERS"] = "2"
import dusk

def threads():
    return len(os.listdir("/proc/self/task"))

before = threads()
nodes = [dusk.Dusk("{LISTEN_ADDRESS}", {port}) for _ in range(12)]
for node in nodes:
    assert list(node.sh("hostname"))
after = threads()
assert after - before <= 2, (before, after)
for node in nodes:
    node.disconnect()
print("threads", before, after)
"#
    ));
    assert!(ok, "{output}");
    assert!(output.contains("threads"), "{output}");
}

#[test]
fn test_a_worker_count_that_is_not_a_whole_number_above_zero_is_refused() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python(&format!(
        r#"
import os
os.environ["DUSK_PY_WORKERS"] = "0"
import dusk

try:
    dusk.Dusk("{LISTEN_ADDRESS}", {port})
except RuntimeError as error:
    assert "DUSK_PY_WORKERS" in str(error), error
else:
    raise AssertionError("a pool of 0 workers was started")
print("refused")
"#
    ));
    assert!(ok, "{output}");
    assert!(output.contains("refused"), "{output}");
}

#[test]
fn test_disconnect_stops_a_command_nobody_reads_and_no_connection_comes_back() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python(&format!(
        r#"
import os
import time
import dusk

def links_to_the_node():
    sockets = set()
    for descriptor in os.listdir("/proc/self/fd"):
        try:
            target = os.readlink(f"/proc/self/fd/{{descriptor}}")
        except OSError:
            continue
        if target.startswith("socket:["):
            sockets.add(target.removeprefix("socket:[").removesuffix("]"))
    links = []
    for table in ("/proc/net/tcp", "/proc/net/tcp6"):
        with open(table) as entries:
            next(entries)
            for entry in entries:
                fields = entry.split()
                if (
                    fields[9] in sockets
                    and int(fields[2].split(":")[-1], 16) == {port}
                    and fields[3] == "01"
                ):
                    links.append(int(fields[1].split(":")[-1], 16))
    return links

node = dusk.Dusk("{LISTEN_ADDRESS}", {port})
assert len(links_to_the_node()) == 1, links_to_the_node()
output = node.sh("; ".join(["echo chatty"] * 100) + "; sleep 600000")
time.sleep(1)
node.disconnect()
deadline = time.monotonic() + 10
while links_to_the_node():
    assert time.monotonic() < deadline, links_to_the_node()
    time.sleep(0.05)
time.sleep(1)
assert links_to_the_node() == [], links_to_the_node()
values = []
try:
    for value in output:
        values.append(value)
except RuntimeError:
    pass
assert len(values) <= 100, len(values)
assert links_to_the_node() == [], links_to_the_node()
print("stopped")
"#
    ));
    assert!(ok, "{output}");
    assert!(output.contains("stopped"), "{output}");
}

#[test]
fn test_a_worker_count_corrected_after_a_refusal_is_taken_by_the_next_client() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python(&format!(
        r#"
import os
os.environ["DUSK_PY_WORKERS"] = "0"
import dusk

try:
    dusk.Dusk("{LISTEN_ADDRESS}", {port})
except RuntimeError as error:
    assert "DUSK_PY_WORKERS" in str(error), error
else:
    raise AssertionError("a pool of 0 workers was started")
os.environ["DUSK_PY_WORKERS"] = "2"
node = dusk.Dusk("{LISTEN_ADDRESS}", {port})
assert list(node.sh("hostname"))
node.disconnect()
print("taken")
"#
    ));
    assert!(ok, "{output}");
    assert!(output.contains("taken"), "{output}");
}

#[test]
fn test_a_process_forked_after_its_first_client_connects_on_workers_of_its_own() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python(&format!(
        r#"
import os
import time
import dusk

parent = dusk.Dusk("{LISTEN_ADDRESS}", {port})
assert list(parent.sh("hostname"))
child = os.fork()
if child == 0:
    try:
        node = dusk.Dusk("{LISTEN_ADDRESS}", {port})
        assert list(node.sh("hostname"))
        node.disconnect()
    except BaseException as error:
        print("the forked client failed:", error, flush=True)
        os._exit(1)
    os._exit(0)
deadline = time.monotonic() + 30
while True:
    finished, status = os.waitpid(child, os.WNOHANG)
    if finished:
        break
    if time.monotonic() > deadline:
        os.kill(child, 9)
        os.waitpid(child, 0)
        raise AssertionError("the forked process's client never answered")
    time.sleep(0.05)
assert os.waitstatus_to_exitcode(status) == 0, status
assert list(parent.sh("hostname"))
parent.disconnect()
print("forked")
"#
    ));
    assert!(ok, "{output}");
    assert!(output.contains("forked"), "{output}");
}

#[test]
fn test_disconnect_is_not_held_up_by_commands_that_fill_the_connection() {
    let port = gen_port();
    let _node = DuskNixImpl::new(LISTEN_ADDRESS, port);
    let (ok, output) = python(&format!(
        r#"
import threading
import time
import dusk

node = dusk.Dusk("{LISTEN_ADDRESS}", {port})
outputs = []
deadline = time.monotonic() + 30
while len(outputs) < 70:
    assert time.monotonic() < deadline, len(outputs)
    try:
        outputs.append(node.sh("sleep 600000"))
    except RuntimeError:
        time.sleep(0.05)
disconnected = threading.Event()

def disconnect():
    node.disconnect()
    disconnected.set()

threading.Thread(target=disconnect, daemon=True).start()
assert disconnected.wait(30), "disconnect waited behind the running commands"
print("disconnected")
"#
    ));
    assert!(ok, "{output}");
    assert!(output.contains("disconnected"), "{output}");
}
