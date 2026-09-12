# dusk-dev

The dusk development harness as a Claude Code plugin: the workflow skills, the
review agents, and the hooks that check the working agreements at the moment
they apply. The agreements themselves are the repository's root `AGENTS.md`,
which Claude Code and Codex both read.

## Enable it

The repository registers the plugin for anyone who trusts the folder
(`.claude/settings.json`). Install it once:

```
claude plugin install dusk-dev@dusk
```

To run a checkout's copy while changing it, start Claude Code with
`claude --plugin-dir ./claude-plugin` and run `/reload-plugins` after an edit.

## What is in it

| part | where |
|---|---|
| project instructions | the repository's root `AGENTS.md`, read by Claude Code and by Codex alike |
| skills | `skills/` — `activate` is the workflow on whatever branch is checked out, `drive-issue` is activate plus a GitHub issue; `atomic-commit`, `splice`, `what`, `honest-to-god`; `authoring-a-program` and `adding-a-driver-method` for the code |
| agents | `agents/` — `atomic-commit`, `self-review`, `race-screen` (which dispatches `race-inspector`), `comment-review`, `terminology-review`, `string-review`, `dilemma-triage`, `decision-ranker` |
| hooks | `hooks/` — one script per event; `python3 hooks/selftest.py` runs them against canned input |

## What the hooks enforce

- No comment line is added to a `.rs`, `.capnp`, `.py`, `.toml`, `.c` or `.h` file.
- A commit subject has no conventional-commit prefix, does not start with Say, carries no issue number, and no message carries a watermark trailer.
- `git commit` runs in the foreground with a 600000 ms timeout; nothing is staged with `git add -A` or `git add .` except the atomic-commit snapshot.
- No polling loop over a command's output; no `cp` of a `target` directory; no bare `git stash`.
- `cargo test` and `cargo nextest` ask first.
- A program under `base/` is edited only after `authoring-a-program` is loaded; a change to the `Driver` trait only after `adding-a-driver-method` is loaded.
- `git push` never targets master, requires every commit on the branch to pass the subject checks, and asks unless the atomic-commit agent has split the tip.
- A pull request is one per drive, carries `Closes #N`, and carries the decisions; merging always asks.
- Once the session has committed or pushed, every turn ends with `Ready for review.` or opens with a `DILEMMA:` line and a question; nothing else ends a turn.
- A message that ends with `Ready for review.` is checked: the line is last, the worktree is gone, the pull request is open at the pushed tip.

## Three phases

**Writing.** The rules above are checked as each action is taken, `dilemma-triage`
judges each decision as it is made, and `comment-review` reads each commit as it
lands.

**Getting ready to review.** The `atomic-commit` agent splits the branch into the
commits the work is. That is the one thing that gates the push, because a human
reads what is pushed, and nobody can read a WIP blob.

**Ready for review.** The hand-off goes out, and `self-review`, `race-screen`,
`terminology-review`, `string-review` and `decision-ranker` run **alongside** the
human, never ahead of them. They gate nothing: what they find goes on the pull
request as it lands. The session is not finished until they have reported, but
the human was never waiting on them.

## The review folder

A branch collects what its reviewer needs under `review/<branch>/`, written by
the hooks from what each agent answers. The folder is ignored by git and never
committed — it is copied into the pull request body, one heading per file:

| file | written from |
|---|---|
| `decisions.md` | every decision, put to `dilemma-triage` as it is made, with its verdict on whether the human must be asked; `decision-ranker` heads the file with the ranking before the pull request, and the pull-request hook copies that into the body |
| `comments.md` | `comment-review`, run on every commit as it lands; a comment that belongs in the commit message comes straight back to the session |
| `terminology.md` | `terminology-review`, the words the branch introduces and the word the codebase already had |
| `strings.md` | `string-review`, every user-facing string the branch adds or changes and where a person sees it |
| `report.html` | rendered from the four, whenever one of them changes |

Open `report.html` in a browser: every `file:line` in it is a link that opens
that line in VS Code. The path it points into is the checkout at the top of the
page, guessed from where the file is opened and remembered per browser.
