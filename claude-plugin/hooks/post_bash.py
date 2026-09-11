import re

from harness import current_branch, drive_state, head, read_input, save_json, session_state

def main():
    hook_input = read_input()
    command = hook_input.get("tool_input", {}).get("command", "")
    cwd = hook_input.get("cwd", ".")
    branch = current_branch(cwd)
    if not branch or branch in ("master", "main", "HEAD"):
        return
    _, session = session_state(hook_input.get("session_id", ""))
    path, drive = drive_state(branch)
    changed = False
    if re.search(r"\bgit\s+commit\b", command):
        if session.get("drive") and not drive.get("issue"):
            drive["issue"] = session["drive"].get("issue")
            drive["phase"] = drive.get("phase") or "working"
            changed = True
    if re.search(r"\bgit\s+push\b", command):
        drive["phase"] = "pushed"
        changed = True
    if changed:
        save_json(path, drive)


if __name__ == "__main__":
    main()
