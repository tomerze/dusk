import json
import re

from harness import current_branch, drive_state, read_input, save_json


def main():
    hook_input = read_input()
    output = hook_input.get("tool_output") or hook_input.get("tool_response") or ""
    if not isinstance(output, str):
        output = json.dumps(output)
    match = re.search(r'"number":\s*(\d+)', output)
    if not match:
        return
    cwd = hook_input.get("cwd", ".")
    branch = current_branch(cwd)
    path, drive = drive_state(branch, cwd)
    drive["pr"] = int(match.group(1))
    drive["phase"] = "opened"
    save_json(path, drive)


if __name__ == "__main__":
    main()
