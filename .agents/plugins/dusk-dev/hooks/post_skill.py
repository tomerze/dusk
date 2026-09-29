from harness import read_input, save_json, session_state


def main():
    hook_input = read_input()
    tool_input = hook_input.get("tool_input", {})
    skill = (tool_input.get("skill") or "").split(":")[-1]
    if not skill:
        return
    session_path, session = session_state(hook_input.get("session_id", ""))
    if skill not in session["loaded_skills"]:
        session["loaded_skills"].append(skill)
        save_json(session_path, session)


if __name__ == "__main__":
    main()
