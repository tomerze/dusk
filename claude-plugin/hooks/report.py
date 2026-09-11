import html
import os

from harness import LOCATION, branch_name, current_branch, review_dir

SECTIONS = (
    ("decisions.md", "Decisions"),
    ("comments.md", "Comments that belong in commit messages"),
    ("terminology.md", "Terminology introduced"),
    ("strings.md", "User-facing strings introduced"),
)
PAGE = """<!doctype html>
<meta charset="utf-8">
<title>Review of {branch}</title>
<style>
body {{ font: 15px/1.5 system-ui, sans-serif; max-width: 60rem; margin: 2rem auto; padding: 0 1rem; color: #222; }}
h1 {{ font-size: 1.6rem; }}
h2 {{ font-size: 1.2rem; margin-top: 2.5rem; border-bottom: 1px solid #ccc; }}
h3 {{ font-size: 1rem; margin: 1.5rem 0 0.25rem; }}
.item {{ margin: 0.25rem 0 0.25rem 1rem; }}
.item::before {{ content: "– "; color: #888; }}
label {{ display: block; margin: 1rem 0; color: #555; }}
input {{ width: 100%; font: inherit; padding: 0.25rem; }}
a {{ color: #0b57d0; }}
</style>
<h1>Review of {branch}</h1>
<label>Checkout path, for the links that open a line in VS Code
<input id="root" type="text"></label>
{sections}
<script>
const input = document.getElementById("root");
const guessed = decodeURIComponent(location.pathname).replace(/\\/review\\/[^/]+\\/report\\.html$/, "");
input.value = localStorage.getItem("dusk-dev-root") || guessed;
function apply() {{
  const root = input.value.replace(/\\/+$/, "");
  localStorage.setItem("dusk-dev-root", root);
  for (const anchor of document.querySelectorAll("a[data-file]")) {{
    anchor.href = `vscode://file/${{root}}/${{anchor.dataset.file}}:${{anchor.dataset.line}}`;
  }}
}}
input.addEventListener("change", apply);
apply();
</script>
"""


def linkify(line):
    escaped = html.escape(line)
    return LOCATION.sub(
        lambda match: f'<a data-file="{match.group(1)}" data-line="{match.group(2)}" href="#">{match.group(0)}</a>',
        escaped,
    )


def section(title, text):
    parts = [f"<h2>{html.escape(title)}</h2>"]
    for line in text.splitlines():
        if line.startswith("# ") or not line.strip():
            continue
        if line.startswith("## "):
            parts.append(f"<h3>{html.escape(line[3:])}</h3>")
        elif line.startswith("- "):
            parts.append(f'<div class="item">{linkify(line[2:])}</div>')
        else:
            parts.append(f"<p>{linkify(line)}</p>")
    return "\n".join(parts)


def render(cwd, branch):
    directory = review_dir(cwd, branch)
    directory.mkdir(parents=True, exist_ok=True)
    sections = []
    for name, title in SECTIONS:
        path = directory / name
        if path.exists():
            sections.append(section(title, path.read_text()))
    page = PAGE.format(branch=html.escape(branch_name(branch)), sections="\n".join(sections))
    (directory / "report.html").write_text(page)
    return directory / "report.html"


if __name__ == "__main__":
    cwd = os.getcwd()
    print(render(cwd, current_branch(cwd)))
