# dusk-dev

The dusk development harness as a plugin: the workflow skills, the review
agents, and the hooks that check the working agreements at the moment they
apply. Claude Code runs all three; Codex reads the skills. The agreements
themselves are the `dusk-developer` skill, which the repository's root
`AGENTS.md` points at and holds nothing besides.

## Enable it

**The repository is the marketplace.** Nothing is cloned or copied into it, and
nothing of the harness sits outside `.agents/plugins/`, `.claude/` and `.codex/`:
the manifests name this plugin where it already sits, at
`.agents/plugins/dusk-dev/`.

| manifest | read by |
|---|---|
| `.agents/plugins/marketplace.json` | Codex - the marketplace, with this plugin at `./.agents/plugins/dusk-dev` |
| `.agents/plugins/dusk-dev/.claude-plugin/marketplace.json` | Claude Code - the same marketplace, this directory being both |
| `.agents/plugins/dusk-dev/.claude-plugin/plugin.json` | Claude Code |
| `.agents/plugins/dusk-dev/.codex-plugin/plugin.json` | Codex |

In a checkout, both tools register the marketplace from the repository's own
settings for anyone who trusts the folder: `.claude/settings.json` enables the
plugin for Claude Code outright, and `.codex/config.toml` declares the
marketplace and marks it enabled for Codex, so Codex's install is one line.

```
codex plugin add dusk-dev@dusk
```

From outside a checkout it is two, with `<owner>/<repo>` as
`git remote get-url origin` gives it.

```
codex plugin marketplace add <owner>/<repo>
codex plugin add dusk-dev@dusk
```

Claude Code installs the same way once the marketplace is known.

```
claude plugin install dusk-dev@dusk
```

Codex copies the plugin into its own home at install time, so re-run
`codex plugin add dusk-dev@dusk` to pick up a change to it. Claude Code loads a
directory marketplace in place, and `claude --plugin-dir ./.agents/plugins/dusk-dev`
plus `/reload-plugins` runs an edit without reinstalling anything.

A Codex session gets the skills two ways, and it needs the install for only one
of them: `AGENTS.md` is read automatically and points at
`skills/dusk-developer/SKILL.md` on disk, while installing the plugin is what
puts every skill in scope by name as `dusk-dev:<skill>`. The hooks and the
agents are Claude Code's; Codex reads the skills.

## What is in it

| part | where |
|---|---|
| project instructions | `skills/dusk-developer` - the orientation map for the codebase and the working agreements every change is held to. The repository's root `AGENTS.md` carries none of it: it points here, which is how Codex and anything else without a skill loader find it |
| skills | `skills/` - `activate` is the workflow on whatever branch is checked out, `drive-issue` is activate plus a GitHub issue; `atomic-commit`, `splice`, `what`, `honest-to-god`; `authoring-a-program` and `adding-a-driver-method` for the code |
| agents | `agents/` - `atomic-commit`, `self-review`, `race-screen` (which dispatches `race-inspector`), `comment-review`, `terminology-review`, `string-review`, `dilemma-triage`, `dilemma-screen`, `decision-ranker` |
| hooks | `hooks/` - one script per event; `python3 hooks/selftest.py` runs them against canned input |
| status line | `hooks/statusline.py` - the session's state, wired from `.claude/settings.json` |

## What the hooks enforce

- No comment line is added to a `.rs`, `.capnp`, `.py`, `.toml`, `.c` or `.h` file.
- Nothing personal is written into any file: the git user's name, their email address and their home directory are refused, read fresh each time so the harness stores none of them.
- A commit subject has no conventional-commit prefix, does not start with Say, carries no issue number, and no message carries a watermark trailer.
- `git commit` runs in the foreground with a 600000 ms timeout; nothing is staged with `git add -A` or `git add .` except the atomic-commit snapshot.
- No polling loop over a command's output; no `cp` of a `target` directory; no bare `git stash`.
- `cargo test` and `cargo nextest` ask first.
- A program under `base/` is edited only after `authoring-a-program` is loaded; a change to the `Driver` trait only after `adding-a-driver-method` is loaded.
- `git push` never targets master and requires every commit on the branch to pass the subject checks. It never waits for the split; the hand-off does.
- A pull request is one per drive, carries `Closes #N`, and carries the decisions; merging always asks.
- Once the session has committed or pushed, every turn is marked: it ends with `Ready for review.`, opens with a `DILEMMA:` line and a question, opens with `WAITING ON SUBAGENTS:` while an agent it dispatched is still running, is a `/what` report, or answers what the user asked and ends with `Still ready for review.` / `Still waiting for dilemma verdict.`; nothing else ends a turn.
- A marked dilemma is put to `dilemma-screen` before the turn is allowed to end, and comes back refused if the screen says the question does not block the work. A decision `dilemma-triage` already marked `ask-human` is not screened.
- The status line shows that state: working, waiting on named agents, handed over, or awaiting a verdict. Once a hand-off is accepted, `ready to review <pull request url>` is its first line - the url comes from the check that accepted the hand-off, so the line cannot name a pull request that was not verified open at the pushed tip.
- A message that ends with `Ready for review.` is checked: the line is last, the worktree is gone, the pull request is open at the pushed tip.
- `Ready for review.` goes out once per branch. After it, a fix is committed, pushed and said on the pull request, and the turn ends `Still ready for review.` - the branch is never handed over a second time, because the human is already reading it.

## Three phases

**Writing.** The rules above are checked as each action is taken, `dilemma-triage`
judges each decision as it is made, and `comment-review` reads each commit that
adds a comment line as it lands.

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
committed - it is copied into the pull request body, one heading per file:

| file | written from |
|---|---|
| `decisions.md` | every decision, put to `dilemma-triage` as it is made, with its verdict on whether the human must be asked; `decision-ranker` heads the file with the ranking before the pull request, and the pull-request hook copies that into the body |
| `comments.md` | `comment-review`, run as a commit lands if its diff adds a comment line; a comment that belongs in the commit message comes straight back to the session, and a commit that added none is recorded as not reviewed |
| `terminology.md` | `terminology-review`, the words the branch introduces and the word the codebase already had |
| `strings.md` | `string-review`, every user-facing string the branch adds or changes and where a person sees it |
| `report.html` | rendered from the four, whenever one of them changes |

Open `report.html` in a browser: every `file:line` in it is a link that opens
that line in VS Code. The path it points into is the checkout at the top of the
page, guessed from where the file is opened and remembered per browser.
