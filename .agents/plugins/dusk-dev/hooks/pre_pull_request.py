import re

from harness import allow_with_input, decisions_path, deny, read_input, reviewed
from pre_bash import pull_request_problems


def decisions_section(text):
    ranked = re.search(r"^## Ranked\n(.*?)(?=^## |\Z)", text, re.S | re.M)
    if ranked:
        return ranked.group(1).strip()
    titles = [line[3:].strip() for line in text.splitlines() if line.startswith("## ")]
    return "\n".join(f"- {title}" for title in titles)


def main():
    hook_input = read_input()
    original = hook_input.get("tool_input", {})
    tool_input = dict(original)
    cwd = hook_input.get("cwd", ".")
    body = tool_input.get("body") or ""
    creating = hook_input.get("tool_name", "").endswith("create_pull_request")
    if not creating and original.get("body") is None:
        return
    branch = reviewed(cwd, hook_input)
    path = decisions_path(cwd, branch)
    if path.exists() and "## Decisions" not in body:
        decisions = decisions_section(path.read_text())
        if decisions:
            tool_input["body"] = body.rstrip() + "\n\n## Decisions\n\n" + decisions + "\n"
            body = tool_input["body"]
    problems = pull_request_problems(cwd, body, creating, hook_input)
    if problems:
        deny("Pull request: " + "; ".join(problems))
    if tool_input.get("body") != original.get("body"):
        allow_with_input(tool_input)


if __name__ == "__main__":
    main()
