import re

from harness import context, read_input, save_json, session_state

ISSUE = re.compile(r"(?:^|\s)#\d+\b|github\.com/[\w.-]+/[\w.-]+/issues/\d+|\bissue\s+#?\d+", re.I)
SLASH = re.compile(r"^\s*/(?:dusk-dev:)?([a-z][a-z0-9-]*)")


def main():
    hook_input = read_input()
    prompt = hook_input.get("user_input") or hook_input.get("prompt") or ""
    if prompt.lstrip().startswith("<task-notification>"):
        return
    session_path, session = session_state(hook_input.get("session_id", ""))
    slash = SLASH.match(prompt)
    if slash:
        skill = slash.group(1)
        if skill not in session["loaded_skills"]:
            session["loaded_skills"].append(skill)
        if skill == "drive-issue":
            number = re.search(r"\d+", prompt[slash.end():])
            session["drive"] = {"issue": int(number.group()) if number else None}
        save_json(session_path, session)
        return
    loaded = session["loaded_skills"]
    if ISSUE.search(prompt) and "drive-issue" not in loaded:
        context(
            "An issue is named. Issue work in this repository goes through the drive-issue skill: "
            "invoke /dusk-dev:drive-issue before doing anything else."
        )
    elif not ({"activate", "drive-issue"} & set(loaded)) and not session.get("activate_nudged"):
        session["activate_nudged"] = True
        save_json(session_path, session)
        context(
            "The dusk working rules are not in force in this session. Invoke /dusk-dev:activate "
            "before changing anything: it carries how decisions are recorded, how the branch is "
            "committed and split, how it is handed over, and when it may be merged."
        )


if __name__ == "__main__":
    main()
