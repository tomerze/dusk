---
name: drive-issue
description: Take a GitHub issue in this repository from "here is the issue number" to a merged PR — read it over the GitHub MCP, work in a git worktree, branch from current master, get the tree buildable, commit through the atomic-commit skill, push, open a PR that links the issue, ask for code review, and merge only once the user approves. Use this skill whenever the user hands over a dusk issue number or issue URL and asks to drive, complete, or finish it.
---

# Drive a Dusk Issue to a Merged PR

The mechanics of shipping an issue in this repository. Nothing here is about how
to write the code — `CLAUDE.md` and the working agreements own that. This is the
git, build, and GitHub sequence around it.

**The user's approval is the only gate you may not skip.** Merging without it is
the one unrecoverable step.

---

## Step 1 — Read the issue over the MCP

```
mcp__github__issue_read(method="get",          owner="tomerze", repo="dusk", issue_number=N)
mcp__github__issue_read(method="get_comments", owner="tomerze", repo="dusk", issue_number=N)
```

Write its definition of done out as a checklist. Every bullet is something the
PR description has to answer for — if it asks for measurements you owe numbers,
if it asks for documented incompatibilities you owe a list.

## Step 2 — Enter a worktree and make it buildable

Use the `EnterWorktree` tool. A fresh dusk worktree does not build until:

```
git submodule update --init vendor/capnproto        # else dusk_capnp's build.rs fails
git submodule update --init vendor/ik_llama.cpp     # else dusk_llm's build.rs fails
cp /home/<user>/git/dusk/Cargo.lock .               # Cargo.lock is gitignored
ln -s <main checkout>/dusk/src/dusk_llm/models dusk/src/dusk_llm/models
```

Why each matters:

- **`Cargo.lock` is gitignored.** A fresh resolve can pick dependency versions
  newer than the manifests were written against, and you will lose an hour to a
  compile error that is not yours. Copy the user's lockfile.
- **The model symlink** skips a 2.9 GiB download. `dusk_llm/model.json` names the
  URL and SHA-256; `build.rs` only downloads when `models/<file>` is absent, so a
  symlink to the main checkout's copy is enough.
- Never work in the user's checkout, and never `cd` out of the worktree.

## Step 3 — Branch from current master, and check it again later

```
git fetch origin
git log origin/master --oneline -3
```

Branch from `origin/master`, not from whatever the worktree started on.

**Re-check `origin/master` before you push.** Dusk work takes hours and the user
merges their own PRs in the meantime; if master moved, rebase and re-run both the
build and any measurements, because numbers taken on a stale base are not what
the reviewer will see.

## Step 4 — Building and measuring

- `cargo check` is cheap and always allowed. **Do not run `cargo test` or
  `cargo nextest` unless the user asked** — tests are a separate workstream.
- `cargo build --release --bin dusk` is the real build. Budget for it: it
  compiles the vendored C++ (ik_llama.cpp), runs the warm-up tool to produce the
  KV snapshot, and links a ~3 GB binary with the model embedded. Sixteen minutes
  from clean on a laptop.
- Building only `-p dusk_llm` can fail with "could find entries-info … dir": the
  system prompt is assembled from `target/.dusk_sh_entries/*.json`, which the
  program crates' `#[sh_entry]` macros write. Build the CLI first.
- If the issue wants a before/after comparison, **build and measure the
  unmodified tree first and keep the numbers** — once the target directory is
  overwritten, recovering the baseline costs another full build. A second
  worktree at `origin/master` is the cheapest way to get it back.
- Run benchmarks **sequentially on an idle machine**. A benchmark sharing the
  machine with a compile reports numbers that are off by 8×.

## Step 5 — Commit

Use the `atomic-commit` skill and follow it. Reference the issue as `(#N)` in the
subject.

- **Never put a timeout on `git commit`.** The pre-commit hooks stash unstaged
  work; a killed commit can lose it.
- Expect two or three attempts: `fmt` and `clang-format` modify files and fail
  the run. Re-stage what they touched and commit again. The full hook set is
  fmt, clippy, clang-format, clang-tidy, ruff, pyright.
- Stage explicit paths, never `git add .` — measurement harnesses, the `models`
  symlink, and `Cargo.lock` must stay out of the commit.

## Step 6 — Push and open the PR over the MCP

```
git push -u origin <branch>
mcp__github__create_pull_request(owner="tomerze", repo="dusk", head=…, base="master", title=…, body=…)
```

`gh` is not installed on this machine; use the MCP. If it returns
`403 Resource not accessible by personal access token`, the token lacks
Pull requests: write — say so and ask the user to grant it rather than
improvising another route.

**Write the PR body in the first person, as the author of the change.** It is a
public record of what the commit does, not a message to the user — the same way
rustc's output is not addressed to anyone in particular. So: no "as we
discussed", no "let me know if you want", no "say the word and I'll switch", no
second person at all. A decision that needs the reviewer's attention is stated as
what was chosen, what was rejected, and what reversing it would cost — then the
reviewer decides in review, in their own words.

The PR body must carry:

- **`Closes #N`**, so the merge closes the issue.
- **One section per acceptance criterion in the issue**, in the issue's own
  terms. Measured numbers in a table, naming the host, the method, and the run
  count.
- **Every incompatibility, workaround and deliberate omission.**
- **Every decision the reviewer might have made differently** — with the
  alternative and what switching would cost. Dusk's working agreements say design
  decisions are the user's; the PR body is where you hand back the ones you had
  to make to keep moving.
- Anything **broken but out of scope** that you tripped over.

## Step 7 — Ask for review, then stop

Give the user the PR URL and ask them to review. Then wait.

Do not merge, do not tidy the branch, and do not read silence or a question as
approval. When comments arrive, verify a claim before implementing it and say so
if you think it is mistaken.

## Step 8 — Merge on explicit approval

```
mcp__github__merge_pull_request(owner="tomerze", repo="dusk", pullNumber=…, merge_method=…)
```

Ask which merge method if it is not obvious from the repository's history.
Afterwards confirm the issue closed and report the merge commit.

---

## Checklist

☐ Issue and comments read over MCP; definition of done written out
☐ Worktree entered; both submodules initialised, lockfile copied, model symlinked
☐ `origin/master` fetched; branch cut from it
☐ Baseline built and measured first, if the issue asks for a comparison
☐ `origin/master` re-checked before pushing; rebased and re-measured if it moved
☐ `cargo build --release --bin dusk` green; no tests run unless asked
☐ One commit per concern, hooks allowed to finish, issue referenced
☐ PR body written in the first person, addressed to no one
☐ Pushed; PR opened over the MCP with `Closes #N`
☐ Review requested; **waited**
☐ Merged only after explicit approval; issue confirmed closed
