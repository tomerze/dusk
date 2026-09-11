from harness import (
    agent_type,
    block,
    current_branch,
    drive_state,
    git,
    head,
    lint_branch,
    read_input,
    save_json,
)

def record(cwd, key):
    branch = current_branch(cwd)
    path, drive = drive_state(branch)
    drive[key] = head(cwd)
    save_json(path, drive)


def clear(cwd, key):
    branch = current_branch(cwd)
    path, drive = drive_state(branch)
    drive.pop(key, None)
    save_json(path, drive)


def main():
    hook_input = read_input()
    handler = HANDLERS.get(agent_type(hook_input))
    if handler:
        handler(hook_input.get("cwd", "."), hook_input.get("last_assistant_message") or "")


if __name__ == "__main__":
    main()
