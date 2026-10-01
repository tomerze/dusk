"""What a person at a Dusk prompt can do, driven on a real terminal."""

from __future__ import annotations

import subprocess
import time

from conftest import ADDRESS, CLI_BINARY, NODE_BINARY, Terminal, wait_for

CTRL_C = b"\x03"
CTRL_D = b"\x04"


def answers(prompt, marker: str = "alive") -> bool:
    """True when the prompt runs a command and shows its result."""
    prompt.since()
    return f'"{marker}"' in prompt.type(f"echo {marker}", until=f'"{marker}"')


def idle(prompt) -> bool:
    """True when the prompt is back, drawn and waiting for a line."""
    return "○ ❯" in prompt.wait_for("○ ❯", timeout=15)


def test_a_prompt_takes_a_command_and_comes_back(prompt):
    prompt.type("echo alive", settle=3)
    assert idle(prompt), "the prompt runs a line and returns for the next"


def test_a_prompt_is_a_process_named_after_its_client(node, prompt):
    assert node.named("sh[prompt"), "the prompt shows up in ps"


def test_a_prompt_attaches_to_the_node_shell(node, prompt):
    assert len(node.named("sh[server]")) == 1, "one shell, made by the prompt"


def test_the_prompt_names_the_client_machine(node, prompt):
    named = node.processes()["Name"]
    assert any("⟷" in name for name in named), f"no client hostname in {named}"


def test_the_client_hostname_can_be_given(node):
    terminal = Terminal(
        [str(CLI_BINARY), node.address], environment={"DUSK_CLIENT_HOSTNAME": "snoopy"}
    )
    try:
        terminal.wait_for("❯")
        assert "sh[prompt ⟷ snoopy]" in node.processes()["Name"]
    finally:
        terminal.stop()


def test_leaving_a_prompt_leaves_the_shell_running(node, prompt):
    prompt.type("exit", settle=1)
    assert prompt.gone(), "the client is gone"
    assert len(node.named("sh[server]")) == 1, "the shell stays"
    assert node.named("sh[prompt") == [], "the view is gone"


def test_ctrl_d_leaves_the_prompt(prompt):
    prompt.press(CTRL_D, settle=1)
    assert prompt.gone(), "ctrl+d closes the prompt like exit"


def test_ctrl_c_hands_the_prompt_back(prompt):
    prompt.type("sleep 60000", settle=1.5)
    started = time.monotonic()
    prompt.press(CTRL_C)
    assert idle(prompt), "the prompt takes commands again"
    assert time.monotonic() - started < 20, "ctrl+c should be immediate"


def test_ctrl_c_stops_the_command_it_interrupted(node, prompt):
    prompt.type("sleep 60000", settle=1.5)
    prompt.press(CTRL_C)
    time.sleep(1)
    assert node.named("sleep") == [], "the sleep is gone from the node"


def test_ctrl_c_stops_a_function_that_calls_itself(node, prompt):
    prompt.type("hi() { ps; hi }", settle=1.5)
    prompt.type("hi", settle=3)
    prompt.press(CTRL_C, settle=3)
    assert idle(prompt), "the prompt survives its own recursion"
    assert node.alive(), "and so does the node"


def test_ctrl_c_twice_in_a_row_is_harmless(prompt):
    prompt.type("sleep 60000", settle=1.5)
    prompt.press(CTRL_C, settle=1)
    prompt.press(CTRL_C, settle=1)
    assert idle(prompt)


def test_ctrl_c_at_an_idle_prompt_leaves_it(prompt):
    prompt.press(CTRL_C, settle=1)
    assert prompt.gone(), "ctrl+c with nothing running closes the prompt"


def test_a_second_prompt_in_one_terminal_is_refused(prompt):
    shown = prompt.type(
        "sh --prompt", until="there is already an open prompt in this terminal"
    )
    assert "there is already an open prompt in this terminal" in shown
    assert idle(prompt), "the prompt is unharmed"


def test_a_refused_second_prompt_leaves_one_view(node, prompt):
    prompt.type("sh --prompt", until="there is already an open prompt")
    assert len(node.named("sh[prompt")) == 1


def test_a_prompt_at_a_pid_with_no_shell_does_not_wedge_the_node(node, prompt):
    prompt.type("sh --prompt 800", settle=3)
    assert node.alive(), "the node still answers other clients"


def test_two_terminals_share_one_shell(node, prompt):
    second = Terminal([str(CLI_BINARY), node.address])
    try:
        second.wait_for("❯")
        assert len(node.named("sh[server]")) == 1, "one shell"
        assert len(node.named("sh[prompt")) == 2, "a view each"
    finally:
        second.stop()


def test_a_function_does_not_reach_a_script(node, prompt):
    prompt.type("mine() { hostname }", settle=2)
    assert "no sh entry found for `mine`" in node.run("mine")


def test_a_function_outlives_the_terminal_that_defined_it(node, prompt):
    prompt.type("kept() { hostname }", settle=2)
    prompt.type("exit", settle=1)
    assert prompt.gone()

    second = Terminal([str(CLI_BINARY), node.address])
    try:
        second.wait_for("❯")
        second.since()
        assert "kept" in second.type("functions", until="kept")
    finally:
        second.stop()


def test_a_function_can_be_defined_and_called(prompt):
    prompt.type("greet() { echo greeted }", settle=2)
    assert idle(prompt), "the definition is taken"
    prompt.type("greet", settle=3)
    assert idle(prompt), "calling it leaves the prompt ready again"


def test_an_empty_body_undefines_a_function(prompt):
    prompt.type("gone() { hostname }", settle=1.5)
    prompt.type("gone() {}", settle=1.5)
    prompt.since()
    assert "no sh entry found" in prompt.type("gone", until="no sh entry found")


def test_help_documents_every_form_of_sh(prompt):
    shown = prompt.type("help sh", until="sh --prompt")
    for form in ["Usage", "sh --server", "sh --prompt", "sh -d"]:
        assert form in shown, f"help sh never mentions {form}"


def test_killing_the_view_ends_the_client(node, prompt):
    view = node.named("sh[prompt")[0]
    node.run(f"kill {view}")
    assert prompt.gone(), "the client goes when its view does"
    assert len(node.named("sh[server]")) == 1, "the shell is untouched"


def test_killing_the_shell_leaves_the_prompt_usable(node, prompt):
    node.expect_errors = True
    shell = node.named("sh[server]")[0]
    node.run(f"kill {shell}")
    prompt.type("echo rebuild", settle=3)
    assert idle(prompt), "the shell is rebuilt underneath"


def test_a_node_that_goes_and_comes_back_keeps_the_prompt(node, prompt):
    node.stop()
    time.sleep(2)
    assert prompt.running(), "the client waits for its node"

    revived = subprocess.Popen(
        [str(NODE_BINARY), f"{ADDRESS}:{node.port}"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        wait_for(node.port)
        time.sleep(3)
        assert prompt.running(), "the client is still there for the new node"
        assert '"' in node.run("hostname"), "and the new node answers"
    finally:
        revived.terminate()


def test_leaving_a_prompt_whose_node_is_gone_does_not_hang(node, prompt):
    node.stop()
    time.sleep(2)
    prompt.type("exit", settle=1)
    assert prompt.gone(), "exit should not wait for a node that is gone"


def test_clear_keeps_the_prompt(prompt):
    prompt.type("clear", settle=2)
    assert idle(prompt)


def test_a_long_line_is_taken(prompt):
    prompt.type("echo " + "x" * 200, settle=3)
    assert idle(prompt), "the prompt survives a long line"


def test_an_unknown_command_is_reported_and_survivable(prompt):
    shown = prompt.type("definitely_not_a_program", until="no sh entry found")
    assert "no sh entry found" in shown
    assert idle(prompt)
