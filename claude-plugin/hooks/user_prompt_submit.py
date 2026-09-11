import re

from harness import context, read_input, save_json, session_state

ISSUE = re.compile(r"(?:^|\s)#\d+\b|github\.com/tomerze/dusk/issues/\d+|\bissue\s+#?\d+", re.I)
SLASH = re.compile(r"^\s*/(?:dusk-dev:)?([a-z][a-z0-9-]*)")


def main():
    hook_input = read_input()
    prompt = hook_input.get("user_input") or hook_input.get("prompt") or ""
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
    if ISSUE.search(prompt) and "drive-issue" not in session["loaded_skills"]:
        context(
            "An issue is named. Issue work in this repository goes through the drive-issue skill: "
            "invoke /dusk-dev:drive-issue before doing anything else."
        )


if __name__ == "__main__":
    main()
