import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from harness import read_input, session_state


def state(session):
    if session.get("agents"):
        return "waiting on " + ", ".join(session["agents"])
    standing = session.get("standing")
    if standing == "ready":
        return "handed over, awaiting review"
    if standing == "dilemma":
        return "awaiting a verdict"
    if session.get("committed"):
        return "working, nothing handed over"
    return "working"


def main():
    hook_input = read_input()
    _, session = session_state(hook_input.get("session_id", ""))
    sys.stdout.write("dusk: " + state(session))


if __name__ == "__main__":
    main()
