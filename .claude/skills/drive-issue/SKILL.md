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
cp /home/<user>/git/dusk/Cargo.lock .               # Cargo.lock is gitignored
```

Why each matters:

- **`vendor/capnproto` is the only submodule**, and the vendored Cap'n Proto
  compiler is the only heavy build-time dependency in the tree. Nothing else is
  fetched at build time: `dusk_llm` talks to an external endpoint over HTTP
  (`DUSK_LLM_URL`) and has no build script, so no model is downloaded, verified
  or embedded.
- **`Cargo.lock` is gitignored.** A fresh resolve can pick dependency versions
  newer than the manifests were written against, and you will lose an hour to a
  compile error that is not yours. Copy the user's lockfile.
- **Do not symlink build inputs in from another checkout.** A symlink whose
  target is deleted while you work turns into a failure nowhere near its cause —
  `fs::create_dir_all` on a dangling symlink fails with `File exists`, not with
  anything naming the link.
- Never work in the user's checkout, and never `cd` out of the worktree.

## Step 3 — Branch from current master, and check it again later

```
git fetch origin
git log origin/master --oneline -3
```

Branch from `origin/master`, not from whatever the worktree started on.

**Re-check `origin/master` before you push, and re-run the build after you
rebase.** Dusk work takes hours and the user merges their own PRs in the
meantime. Master does not only change code — it can delete the build
infrastructure you set your worktree up around, so treat a rebase as
invalidating your whole environment, not just your numbers.

Past failure: while driving #6, master moved twice. The second move (#11)
deleted `dusk_llm/build.rs`, `dusk_llm/model.json`, the `vendor/ik_llama.cpp`
submodule and the compile-time sh-entry side-channel. The setup this skill had
prescribed an hour earlier no longer applied, and a step from it — a symlink
into the main checkout — went dangling mid-run and failed a commit's clippy hook
with an error that named neither the symlink nor the branch that removed it.

## Step 4 — Building and measuring

- `cargo check` is cheap and always allowed. **Do not run `cargo test` or
  `cargo nextest` unless the user asked** — tests are a separate workstream.
  An issue that asks for tests is asking; running them is then part of
  delivering, and shipping a test you never executed is not.
- `cargo build --release --bin dusk` is the real build. It is a plain Rust
  release build; the only unusual cost is compiling the vendored Cap'n Proto
  compiler the first time.
- If the issue wants a before/after comparison, **build and measure the
  unmodified tree first and keep the numbers** — once the target directory is
  overwritten, recovering the baseline costs another full build. A second
  worktree at `origin/master` is the cheapest way to get it back.
- Run benchmarks **sequentially on an idle machine**. A benchmark sharing the
  machine with a compile reports numbers that are off by 8×. **Check whether the
  machine is actually idle first** — other worktrees under `.claude/worktrees/`
  may have their own sessions compiling right now (`ps aux | grep rustc` shows
  whose target directory each one is writing to).
- **Never state a number you did not measure**, and never explain a slow build
  before timing it. Measure, then decide.

## Step 5 — Commit

Use the `atomic-commit` skill and follow it. Reference the issue as `(#N)` in the
subject.

- **Run `git commit` in the foreground with an explicit long timeout**
  (`timeout: 600000`). Do not background it, and do not poll for it — the
  harness re-invokes you when a background command exits, so a `sleep` loop
  watching your own output file is pure waste. The reason to give it a long
  timeout rather than the default is that pre-commit stashes unstaged work
  before the hooks run: a commit killed mid-hook can strand that stash.
- If a commit is killed anyway, the stash is recoverable from the patch named in
  the hook's `[INFO] Stashing unstaged files to <path>` line: `git apply <path>`.
  **`~/.cache/pre-commit/` is shared by every worktree on the machine.** A patch
  in there may belong to another session — check that its diff is yours before
  applying it, and never apply one you did not create.
- The hook set is fmt, clippy, clang-format, clang-tidy, ruff (lint and format)
  and pyright, with `fail_fast: true`. Only fmt and clippy run for a Rust-only
  change; the rest match on C or Python paths and skip. Expect a second attempt
  when `fmt` rewrites a file and fails the run — re-stage what it touched and
  commit again.
- Stage explicit paths, never `git add .` — measurement harnesses, `Cargo.lock`
  and anything else you created during setup must stay out of the commit.

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

## Keeping this skill true

This file describes an environment that changes under it. When a step here turns
out to be wrong — a submodule that no longer exists, a build cost that no longer
applies, a gotcha that was fixed — **say so and fix the skill**, in its own
commit, separate from the issue you were driving. A skill that misdirects the
next agent is a defect exactly like a wrong comment.

---

## Checklist

☐ Issue and comments read over MCP; definition of done written out
☐ Worktree entered; `vendor/capnproto` initialised, lockfile copied
☐ `origin/master` fetched; branch cut from it
☐ Baseline built and measured first, if the issue asks for a comparison
☐ `origin/master` re-checked before pushing; rebased, then the build re-run
☐ `cargo build --release --bin dusk` green; no tests run unless asked
☐ One commit per concern, committed in the foreground with a long timeout
☐ PR body written in the first person, addressed to no one
☐ Pushed; PR opened over the MCP with `Closes #N`
☐ Review requested; **waited**
☐ Merged only after explicit approval; issue confirmed closed
