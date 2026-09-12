from harness import amend, read_input, session_state


def dispatched(session, name):
    session["agents"] = (session.get("agents") or []) + [name]


def main():
    hook_input = read_input()
    tool_input = hook_input.get("tool_input", {})
    name = (tool_input.get("subagent_type") or "").split(":")[-1] or tool_input.get("description") or "agent"
    session_path, _ = session_state(hook_input.get("session_id", ""))
    amend(session_path, lambda session: dispatched(session, name))


if __name__ == "__main__":
    main()
