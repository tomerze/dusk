import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HOOKS = Path(__file__).resolve().parent


def run(script, hook_input):
    env = dict(os.environ, DUSK_HARNESS_DATA=str(DATA), PYTHONPATH=str(HOOKS), DUSK_NO_DILEMMA_SCREEN="1")
    completed = subprocess.run(
        [sys.executable, str(HOOKS / script)],
        input=json.dumps(hook_input),
        capture_output=True,
        text=True,
        env=env,
        check=False,
    )
    return completed.returncode, completed.stdout, completed.stderr


def bash(command, **tool_input):
    return {"session_id": "selftest", "cwd": str(REPO), "tool_name": "Bash", "tool_input": {"command": command, **tool_input}}


def edit(path, old, new):
    return {"session_id": "selftest", "cwd": str(REPO), "tool_name": "Edit", "tool_input": {"file_path": path, "old_string": old, "new_string": new}}


def agent(name, message):
    return {"session_id": "selftest", "cwd": str(REPO), "agent_type": f"dusk-dev:{name}", "last_assistant_message": message}


def expect(name, result, code, contains=None):
    actual, stdout, stderr = result
    text = stdout + stderr
    ok = actual == code and (contains is None or contains in text)
    print(("ok   " if ok else "FAIL ") + name + ("" if ok else f" -> exit {actual}: {text.strip()[:200]}"))
    return ok


def concurrent_list():
    sys.path.insert(0, str(HOOKS))
    os.environ["DUSK_HARNESS_DATA"] = str(DATA)
    import harness

    path = DATA / "sessions" / "listed.json"
    harness.save_json(path, {"agents": ["a", "b"]})
    harness.amend(path, lambda state: state["agents"].append("c"))
    harness.amend(path, lambda state: state["agents"].remove("a"))
    after = harness.read_json(path, {})
    return (0 if after.get("agents") == ["b", "c"] else 1, str(after), "")


def pushed_branches():
    sys.path.insert(0, str(HOOKS))
    import post_bash

    cases = {
        "git push -u origin topic": "topic",
        "git push -q --force-with-lease origin claude-plugin": "claude-plugin",
        "git push origin HEAD:main": "main",
        "git push": None,
    }
    wrong = {c: post_bash.pushed_branch(c) for c, want in cases.items() if post_bash.pushed_branch(c) != want}
    return (0 if not wrong else 1, str(wrong), "")


def first_write():
    sys.path.insert(0, str(HOOKS))
    os.environ["DUSK_HARNESS_DATA"] = str(DATA)
    import harness

    path = DATA / "sessions" / "fresh.json"
    if path.exists():
        path.unlink()
    _, mine = harness.session_state("fresh")
    mine["activate_nudged"] = True
    harness.save_json(path, mine)
    after = harness.read_json(path, {})
    return (0 if after.get("activate_nudged") and "committed" in after and "standing" in after else 1, str(after), "")


def concurrent_write():
    sys.path.insert(0, str(HOOKS))
    os.environ["DUSK_HARNESS_DATA"] = str(DATA)
    import harness

    path = DATA / "sessions" / "concurrent.json"
    harness.save_json(path, {"loaded_skills": ["activate"], "committed": False})
    _, mine = harness.session_state("concurrent")
    harness.AS_LOADED.pop(str(path), None)
    harness.save_json(path, {"loaded_skills": ["activate"], "committed": True})
    harness.AS_LOADED[str(path)] = {"loaded_skills": ["activate"], "committed": False}
    mine["driver_checks"] = {"a.rs": True}
    harness.save_json(path, mine)
    after = harness.read_json(path, {})
    return (0 if after.get("committed") and after.get("driver_checks") else 1, str(after), "")


def identity():
    done = subprocess.run(["git", "config", "--get", "user.name"], capture_output=True, text=True, check=False)
    return done.stdout.strip() or str(Path.home().name)


def waiting_session():
    path = DATA / "sessions" / "waiting.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"committed": True, "agents": ["self-review", "race-screen"]}))
    return "waiting"


def session_field(name, key, value):
    path = DATA / "sessions" / f"{name}.json"
    found = json.loads(path.read_text()).get(key) if path.exists() else None
    return (0 if found == value else 1, str(found), "")


def background_session(name, text):
    output = DATA / "tasks" / f"{name}.output"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(text)
    path = DATA / "sessions" / f"background-{name}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"committed": True, "background": [{"id": name, "output": str(output)}]}))
    return f"background-{name}"


def statusline():
    return run("statusline.py", {"session_id": waiting_session()})


def background_statusline():
    return run("statusline.py", {"session_id": background_session("running", "compiling")})


def handed_over_session(url):
    path = DATA / "sessions" / "handed-over.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"committed": True, "standing": "ready", "pull_request": url}))
    return "handed-over"


def review_statusline(url):
    return run("statusline.py", {"session_id": handed_over_session(url)})


def handed_session(branch):
    path = DATA / "sessions" / "handed.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"committed": True, "handed": branch, "loaded_skills": ["activate"]}))
    return "handed"


def standing_session(state):
    path = DATA / "sessions" / f"standing-{state}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"committed": True, "standing": state, "loaded_skills": ["activate"]}))
    return f"standing-{state}"


def committed_session():
    path = DATA / "sessions" / "committed.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"committed": True, "loaded_skills": ["activate"]}))
    return "committed"


def review_file(name, text, present=True):
    path = REPO / "review" / "topic" / name
    found = path.exists() and text in path.read_text()
    return (0 if found == present else 1, "", "")


def handed_review(name, message, branch):
    path = DATA / "sessions" / "handed-review.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"committed": True, "handed": branch}))
    return {"session_id": "handed-review", "cwd": str(REPO), "agent_type": f"dusk-dev:{name}", "last_assistant_message": message}


def pull_request_from_another_branch(body):
    path = DATA / "sessions" / "handed-pull-request.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"committed": True, "handed": "topic"}))
    subprocess.run(["git", "checkout", "-q", "master"], cwd=REPO, check=True)
    result = run(
        "pre_pull_request.py",
        {
            "session_id": "handed-pull-request",
            "cwd": str(REPO),
            "tool_name": "mcp__github__update_pull_request",
            "tool_input": {"body": body},
        },
    )
    subprocess.run(["git", "checkout", "-q", "topic"], cwd=REPO, check=True)
    return result


def branch_review_file(branch, name, text):
    path = REPO / "review" / branch / name
    return (0 if path.exists() and text in path.read_text() else 1, str(path), "")


with tempfile.TemporaryDirectory() as temp:
    DATA = Path(temp) / "data"
    REPO = Path(temp) / "repo"
    REPO.mkdir()
    subprocess.run(["git", "init", "-q", "-b", "master"], cwd=REPO, check=True)
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "Root"], cwd=REPO, check=True)
    subprocess.run(["git", "checkout", "-q", "-b", "topic"], cwd=REPO, check=True)
    triage = "DECISION: Keep x\nDECIDED: keep it\nALTERNATIVES: drop it\nREVERSAL: one line\nVERDICT: decide-alone\nWHY: local"
    results = [
        expect("commit with a prefix is denied", run("pre_bash.py", bash('git commit -m "feat: add x"', timeout=600000)), 2, "conventional-commit prefix"),
        expect("commit starting with Say is denied", run("pre_bash.py", bash('git commit -m "Say how it opens"', timeout=600000)), 2, "starts with Say"),
        expect("commit with a watermark is denied", run("pre_bash.py", bash('git commit -m "Add x" -m "Co-Authored-By: someone"', timeout=600000)), 2, "watermark"),
        expect("commit with a short timeout is denied", run("pre_bash.py", bash('git commit -m "Add x"')), 2, "timeout 600000"),
        expect("plain commit is allowed", run("pre_bash.py", bash('git commit -m "Add x"', timeout=600000)), 0),
        expect("git add -A is denied", run("pre_bash.py", bash("git add -A && git commit -m 'Add x'", timeout=600000)), 2, "named paths"),
        expect("the snapshot may use git add -A", run("pre_bash.py", bash("git add -A && git commit --no-verify -m 'WIP snapshot'", timeout=600000)), 0),
        expect("polling is denied", run("pre_bash.py", bash("until grep -q done out.log; do sleep 5; done")), 2, "Never poll"),
        expect("copying target is denied", run("pre_bash.py", bash("cp -a ../dusk/target .")), 2, "target"),
        expect("a heredoc written to a file is text", run("pre_bash.py", bash("cat > notes.md <<'EOF'\ncp -a ../dusk/target .\nEOF")), 0),
        expect("a heredoc fed to an interpreter is a command", run("pre_bash.py", bash("bash <<'EOF'\ncp -a ../dusk/target .\nEOF")), 2, "target directory"),
        expect("a heredoc piped into an interpreter is a command", run("pre_bash.py", bash("cat <<'EOF' | bash\ncp -a ../dusk/target .\nEOF")), 2, "target directory"),
        expect("the word snapshot in a heredoc does not exempt git add -A", run("pre_bash.py", bash("git add -A && cat > notes.md <<'EOF'\nsnapshot\nEOF", timeout=600000)), 2, "named paths"),
        expect("a path that merely contains sh does not make a heredoc a command", run("pre_bash.py", bash("cat > base/sh/README.md <<'EOF'\ngit stash\nEOF")), 0),
        expect("a read-only agent may redirect stderr into stdout", run("pre_bash.py", {**bash("cargo check 2>&1 | tail -40"), "agent_type": "dusk-dev:self-review"}), 0),
        expect("a read-only agent may grep for a greater-than", run("pre_bash.py", {**bash("grep -rn 'a>b' ."), "agent_type": "dusk-dev:self-review"}), 0),
        expect("a read-only agent may write outside the tree", run("pre_bash.py", {**bash("grep -rn x . > /tmp/notes.txt"), "agent_type": "dusk-dev:self-review"}), 0),
        expect("a read-only agent may not write into the tree", run("pre_bash.py", {**bash("grep -rn x . > out.txt"), "agent_type": "dusk-dev:self-review"}), 2, "reads"),
        expect("bare stash is denied", run("pre_bash.py", bash("git stash")), 2, "stash"),
        expect("stash push is allowed", run("pre_bash.py", bash("git stash push -u -m tag")), 0),
        expect("cargo test asks", run("pre_bash.py", bash("cargo nextest run")), 0, '"ask"'),
        expect("push to master is denied", run("pre_bash.py", bash("git push origin master")), 2, "master"),
        expect("push does not wait for the split", run("pre_bash.py", bash("git push -u origin topic")), 0),
        expect("a read-only agent may not commit", run("pre_bash.py", {**bash('git commit -m "Add x"', timeout=600000), "agent_type": "dusk-dev:self-review"}), 2, "reads"),
        expect("a name in an edit is denied", run("pre_edit.py", edit(str(REPO / "src/lib.rs"), "let a = 1;", "let author = \"" + identity() + "\";")), 2, "Nothing personal"),
        expect("a home directory in an edit is denied", run("pre_edit.py", edit(str(REPO / "docs/x.md"), "a", "see " + str(Path.home()) + "/notes")), 2, "Nothing personal"),
        expect("a comment line in Rust is denied", run("pre_edit.py", edit(str(REPO / "src/lib.rs"), "let a = 1;", "// the answer\nlet a = 1;")), 2, "do not write comments"),
        expect("an existing comment line passes", run("pre_edit.py", edit(str(REPO / "src/lib.rs"), "// kept\nlet a = 1;", "// kept\nlet a = 2;")), 0),
        expect("a program edit needs the authoring skill", run("pre_edit.py", edit(str(REPO / "base/sleep/src/lib.rs"), "a", "b")), 2, "authoring-a-program"),
        expect("a hash inside a python string is not a comment", run("pre_edit.py", edit(str(REPO / "hooks/x.py"), "a = 1", 'PROMPT = \"\"\"\n# Heading\n\"\"\"')), 0),
        expect("a file outside the repository is free", run("pre_edit.py", edit("/tmp/x.py", "a", "# note")), 0),
        expect("a file in another worktree of the repository is not", run("pre_edit.py", edit(str(REPO / "src/lib.rs"), "let a = 1;", "// note\nlet a = 1;")), 2, "do not write comments"),
        expect("a markdown edit is free", run("pre_edit.py", edit(str(REPO / "docs/x.md"), "a", "# b")), 0),
        expect("a stop with nothing pending passes", run("stop.py", {"session_id": "selftest", "cwd": str(REPO), "last_assistant_message": "Done."}), 0),
        expect("the hand-off is checked against the branch that was pushed", run("stop.py", {"session_id": handed_session("other-branch"), "cwd": str(REPO), "last_assistant_message": "Ready for review."}), 2, "other-branch"),
        expect("a Ready line that is not last is blocked", run("stop.py", {"session_id": "selftest", "cwd": str(REPO), "last_assistant_message": "Ready for review.\n\nAlso this."}), 2, "last line"),
        expect("a turn after a commit must hand over or ask", run("stop.py", {"session_id": committed_session(), "cwd": str(REPO), "last_assistant_message": "Here is where things stand."}), 2, "DILEMMA"),
        expect("an unmarked question after a commit is blocked", run("stop.py", {"session_id": "committed", "cwd": str(REPO), "last_assistant_message": "Which of the two should it be?"}), 2, "DILEMMA"),
        expect("a wait with no agent running is blocked", run("stop.py", {"session_id": committed_session(), "cwd": str(REPO), "last_assistant_message": "WAITING ON SUBAGENTS: self-review"}), 2, "None is"),
        expect("a wait while an agent runs passes", run("stop.py", {"session_id": waiting_session(), "cwd": str(REPO), "last_assistant_message": "WAITING ON SUBAGENTS: self-review, race-screen"}), 0),
        expect("a background wait with nothing running is blocked", run("stop.py", {"session_id": committed_session(), "cwd": str(REPO), "last_assistant_message": "WAITING ON BACKGROUND WORK: the cross-compile matrix"}), 2, "None is"),
        expect("a background wait while a command runs passes", run("stop.py", {"session_id": background_session("running", "compiling"), "cwd": str(REPO), "last_assistant_message": "WAITING ON BACKGROUND WORK: the cross-compile matrix"}), 0),
        expect("a background wait after the command exited is blocked", run("stop.py", {"session_id": background_session("done", "compiling\n[exited with code 0]\n"), "cwd": str(REPO), "last_assistant_message": "WAITING ON BACKGROUND WORK: the cross-compile matrix"}), 2, "None is"),
        expect("a subagent wait names the background line when none is an agent", run("stop.py", {"session_id": committed_session(), "cwd": str(REPO), "last_assistant_message": "WAITING ON SUBAGENTS: the cross-compile matrix"}), 2, "WAITING ON BACKGROUND WORK"),
        expect("the status line names a running background command", background_statusline(), 0, "waiting on background work: running"),
        expect("dispatching an agent records it", run("pre_agent.py", {"session_id": "dispatch", "cwd": str(REPO), "tool_input": {"subagent_type": "dusk-dev:self-review"}}), 0),
        expect("the agent landed in the session", session_field("dispatch", "agents", ["self-review"]), 0),
        expect("the status line names the state", statusline(), 0, "waiting on self-review"),
        expect("a handed-over status line leads with the pull request", review_statusline("https://github.com/o/r/pull/7"), 0, "ready to review https://github.com/o/r/pull/7\ndusk:"),
        expect("an answer with nothing standing is blocked", run("stop.py", {"session_id": committed_session(), "cwd": str(REPO), "last_assistant_message": "It reads the reflog.\n\nStill ready for review."}), 2, "None did"),
        expect("an answer restating the standing hand-off passes", run("stop.py", {"session_id": standing_session("ready"), "cwd": str(REPO), "last_assistant_message": "It reads the reflog.\n\nStill ready for review."}), 0),
        expect("the wrong standing line is blocked", run("stop.py", {"session_id": standing_session("dilemma"), "cwd": str(REPO), "last_assistant_message": "It reads the reflog.\n\nStill ready for review."}), 2, "what stands is the dilemma"),
        expect("an answer restating the standing dilemma passes", run("stop.py", {"session_id": standing_session("dilemma"), "cwd": str(REPO), "last_assistant_message": "It reads the reflog.\n\nStill waiting for dilemma verdict."}), 0),
        expect("a marked dilemma after a commit passes", run("stop.py", {"session_id": "committed", "cwd": str(REPO), "last_assistant_message": "DILEMMA: the name\n\nWhich of the two should it be?"}), 0),
        expect("a quoted Ready line is not a claim", run("stop.py", {"session_id": "selftest", "cwd": str(REPO), "last_assistant_message": '- a.py:1 - "Ready for review." - the hand-off prints it'}), 0),
        expect("a malformed triage answer is blocked", run("subagent_stop.py", agent("dilemma-triage", "DECISION: x\nVERDICT: maybe")), 2, "six lines"),
        expect("a triage answer is recorded", run("subagent_stop.py", agent("dilemma-triage", triage)), 0),
        expect("the decision landed in the file", review_file("decisions.md", "## Keep x"), 0),
        expect("a ranking is recorded", run("subagent_stop.py", agent("decision-ranker", "RANKED:\n1. Keep x - attention: low - local")), 0),
        expect("the ranking heads the file", review_file("decisions.md", "## Ranked"), 0),
        expect("a review without a verdict is blocked", run("subagent_stop.py", agent("self-review", "looks fine")), 2, "VERDICT"),
        expect("a verdict the agent bolded is read", run("subagent_stop.py", agent("self-review", "**VERDICT: pass**")), 0),
        expect("the split needs the snapshot tag", run("subagent_stop.py", agent("atomic-commit", "done")), 2, "harness-snapshot"),
        expect("a comment review without a verdict is blocked", run("subagent_stop.py", agent("comment-review", "nothing")), 2, "COMMENTS"),
        expect("a terminology review is recorded", run("subagent_stop.py", agent("terminology-review", "| shard | [src/a.rs:12](../../src/a.rs#L12) | a partition, no existing word |\n**TERMS: found**")), 0),
        expect("the term landed in the file", review_file("terminology.md", "| shard | [src/a.rs:12]"), 0),
        expect("the table has a header to edit in", review_file("terminology.md", "| Name | Link | Explanation |"), 0),
        expect("a review with findings but no row is blocked", run("subagent_stop.py", agent("terminology-review", "- shard - src/a.rs:12 - a partition - none\nTERMS: found")), 2, "table row"),
        expect("a string review is recorded", run("subagent_stop.py", agent("string-review", '- I looked at every crate\n| "no such pid" | [src/b.rs:7](../../src/b.rs#L7) | kill prints it |\nSTRINGS: found')), 0),
        expect("the narration was dropped", review_file("strings.md", "- I looked", present=False), 0),
        expect("the string landed in the file", review_file("strings.md", '"no such pid"'), 0),
        expect("a review follows the pushed branch, not the checkout's", run("subagent_stop.py", handed_review("string-review", '| "gone" | [src/a.rs:1](../../src/a.rs#L1) | a prompt |\n**STRINGS: found**', "pushed-branch")), 0),
        expect("the review landed under the pushed branch", branch_review_file("pushed-branch", "strings.md", '"gone"'), 0),
        expect("a pull request reads the pushed branch's decisions, not the checkout's", pull_request_from_another_branch("What it does\n\n## Decisions\n\n- Keep x"), 0),
        expect("the report links a location to vscode", review_file("report.html", 'data-file="src/a.rs" data-line="12"'), 0),
        expect("a slow writer does not lose a fast one's field", concurrent_write(), 0),
        expect("a first write keeps the keys it did not touch", first_write(), 0),
        expect("a push names the branch it pushed, not the checkout's", pushed_branches(), 0),
        expect("two writers of one list keep both changes", concurrent_list(), 0),
        expect("the report carries every section", review_file("report.html", "User-facing strings introduced"), 0),
    ]
    sys.exit(0 if all(results) else 1)
