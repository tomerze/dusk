import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HOOKS = Path(__file__).resolve().parent


def run(script, hook_input):
    env = dict(os.environ, DUSK_HARNESS_DATA=str(DATA), PYTHONPATH=str(HOOKS))
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


def pull_request(body):
    return {"session_id": "selftest", "cwd": str(REPO), "tool_name": "mcp__github__create_pull_request", "tool_input": {"body": body}}


def expect(name, result, code, contains=None):
    actual, stdout, stderr = result
    text = stdout + stderr
    ok = actual == code and (contains is None or contains in text)
    print(("ok   " if ok else "FAIL ") + name + ("" if ok else f" -> exit {actual}: {text.strip()[:200]}"))
    return ok


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
    return (0 if after.get("activate_nudged") and "loaded_skills" in after and "driver_checks" in after else 1, str(after), "")


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


with tempfile.TemporaryDirectory() as temp:
    DATA = Path(temp) / "data"
    REPO = Path(temp) / "repo"
    REPO.mkdir()
    subprocess.run(["git", "init", "-q", "-b", "master"], cwd=REPO, check=True)
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "Root"], cwd=REPO, check=True)
    subprocess.run(["git", "checkout", "-q", "-b", "topic"], cwd=REPO, check=True)
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
        expect("push to master is denied", run("pre_bash.py", bash("git push origin master")), 2, "master"),
        expect("push to a topic branch is allowed", run("pre_bash.py", bash("git push -u origin topic")), 0),
        expect("a pull request with a watermark is denied", run("pre_pull_request.py", pull_request("Adds x\n\nGenerated with a tool")), 2, "watermark"),
        expect("a plain pull request passes", run("pre_pull_request.py", pull_request("Adds x")), 0),
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
        expect("a slow writer does not lose a fast one's field", concurrent_write(), 0),
        expect("a first write keeps the keys it did not touch", first_write(), 0),
    ]
    sys.exit(0 if all(results) else 1)
