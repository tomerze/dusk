import re

from harness import context, read_input, save_json, session_state

ISSUE = re.compile(r"(?:^|(?<!\bpr)(?<!\bpull request)\s)#\d+\b|github\.com/[\w.-]+/[\w.-]+/issues/\d+|\bissue\s+#?\d+", re.I)
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
            "Nothing about dusk is loaded in this session. Invoke /dusk-dev:activate before "
            "changing anything: it carries what there is to know about the codebase and how it "
            "is built."
        )


if __name__ == "__main__":
    main()
