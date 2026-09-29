from harness import deny, read_input
from pre_bash import pull_request_problems


def main():
    hook_input = read_input()
    body = hook_input.get("tool_input", {}).get("body") or ""
    problems = pull_request_problems(body)
    if problems:
        deny("Pull request: " + "; ".join(problems))


if __name__ == "__main__":
    main()
