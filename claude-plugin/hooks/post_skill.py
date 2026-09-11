import re

from harness import current_branch, drive_state, read_input, save_json, session_state


def main():
    hook_input = read_input()
    tool_input = hook_input.get("tool_input", {})
    skill = (tool_input.get("skill") or "").split(":")[-1]
    if not skill:
        return
    session_path, session = session_state(hook_input.get("session_id", ""))
    if skill not in session["loaded_skills"]:
        session["loaded_skills"].append(skill)
    if skill == "drive-issue":
        args = tool_input.get("args") or ""
        number = re.search(r"\d+", args)
        session["drive"] = {"issue": int(number.group()) if number else None}
        branch = current_branch(hook_input.get("cwd", "."))
        if branch and branch not in ("master", "main", "HEAD") and number:
            path, drive = drive_state(branch)
            drive["issue"] = int(number.group())
            drive.setdefault("phase", "working")
            save_json(path, drive)
    save_json(session_path, session)


if __name__ == "__main__":
    main()
