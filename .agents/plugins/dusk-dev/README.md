# dusk-dev

The dusk development plugin: the workflow skills, a race inspection and a code
review as agents, and the hooks that check the working agreements at the moment
they apply. Claude Code runs all three; Codex reads the skills. The agreements
themselves are the `dusk-developer` skill, which the repository's root
`AGENTS.md` points at and holds nothing besides.

## Enable it

**The repository is the marketplace.** Nothing is cloned or copied into it, and
nothing of the plugin sits outside `.agents/plugins/`, `.claude/` and `.codex/`:
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
| skills | `skills/` - `activate` loads what there is to know about dusk; `drive-issue` takes a GitHub issue to a merged pull request; `atomic-commit`, `splice`, `what`, `honest-to-god`; `authoring-a-program` and `adding-a-driver-method` for the code |
| agents | `agents/` - `race-screen`, which dispatches `race-inspector` when a diff could race, and `self-review`, the one code review |
| hooks | `hooks/` - one script per event; `python3 hooks/selftest.py` runs them against canned input |

## What the hooks enforce

In every session:

- No comment line is added to a `.rs`, `.capnp`, `.py`, `.toml`, `.c` or `.h` file.
- Nothing personal is written into any file: the git user's name, their email address and their home directory are refused, read fresh each time so the plugin stores none of them.
- A commit subject has no conventional-commit prefix, does not start with Say, carries no issue number, and no message carries a watermark trailer.
- A commit that changes only `docs/` has a subject starting `Update docs`, and one that changes only `tests/` starts `Add tests`, so a reviewer can skip the low-stakes commits by their first two words.
- `git commit` runs in the foreground with a 600000 ms timeout; nothing is staged with `git add -A` or `git add .` except the atomic-commit snapshot.
- No polling loop over a command's output; no `cp` of a `target` directory; no bare `git stash`.
- A program under `base/` is edited only after `authoring-a-program` is loaded; a change to the `Driver` trait only after `adding-a-driver-method` is loaded.
- `git push` never targets master and requires every commit on the branch to pass the subject checks.
- The `race-screen`, `race-inspector` and `self-review` agents read; they do not commit, and do not write into the working tree.
- A pull request body carries no watermark.
- Merging always asks.
