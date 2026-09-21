# Dusk

The project instructions for this repository are the **`dusk-developer` skill**,
in the `dusk-dev` plugin at `.agents/plugins/dusk-dev/skills/dusk-developer/SKILL.md`.
This file carries none of them itself.

**Claude Code:** load it - `/dusk-dev:dusk-developer`. The plugin is registered
by `.claude/settings.json` for anyone who trusts this folder.

**Codex:** the plugin is registered by `.codex/config.toml` for anyone who
trusts this repository; `codex plugin add dusk-dev@dusk` then puts every skill
below in scope by name.

**Every other agent** - anything that reads this file and has no skill loader -
read `.agents/plugins/dusk-dev/skills/dusk-developer/SKILL.md` now, before anything
else, and treat what it says as this file's content.

The skills beside it are the workflows, and one of them applies to whatever you
are about to do:

| skill | for |
|---|---|
| `activate` | any change on the branch that is checked out: the decisions, the commits, the hand-off, the review |
| `drive-issue` | `activate` plus a GitHub issue, from the issue number to the merged pull request |
| `atomic-commit` | splitting work into commits that are each one statement |
| `splice` | cutting a pull request that grew past what one person can read |
| `authoring-a-program` | writing a program under `base/` |
| `adding-a-driver-method` | adding a method to the `Driver` trait |
| `fix-strings` | applying the wording you edited into `review/<branch>/strings.md` back into the code |
| `fix-terminology` | applying the names you edited into `review/<branch>/terminology.md`, everywhere they are used |
| `what` | reporting where things stand |
| `honest-to-god` | saying only what is true, in as few words as it takes |

They live beside `dusk-developer` under `.agents/plugins/dusk-dev/skills/`.
