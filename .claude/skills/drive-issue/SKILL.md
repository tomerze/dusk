---
name: drive-issue
description: Take a GitHub issue in this repository from "here is the issue number" to a merged PR — read it over the GitHub MCP, work in a git worktree, branch from current master, get the tree buildable, commit through the atomic-commit skill, push, open the single PR that links the issue, ask for code review, and merge only once the user approves. Use this skill whenever the user hands over a dusk issue number or issue URL and asks to drive, complete, or finish it. Also use it for "drive-issue continue", which resumes a drive whose PR is already open — absorbing the user's own edits and review comments into the existing commits rather than opening anything new.
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
- **A full disk reports itself as a compiler bug.** When the volume fills, rustc
  and the linker fail with `No space left on device` mixed in among
  `rustc-LLVM ERROR` and `ld terminated with signal 7 [Bus error]`, which read
  like a broken toolchain. Run `df -h` before believing any of it. Every
  worktree carries its own `target`, so measure them with `du -sh` rather than
  assuming a size — they differ by more than an order of magnitude. Reclaiming
  space means deleting the user's build caches, so **ask first**, and name the
  measured sizes and which branches are already merged so the choice is theirs.

## Step 4b — Driving the node by hand

Running the thing beats reasoning about it, and it is not the test suite, so it
is available even when tests are not. Build with
`cargo build --bin dusk_node --bin dusk`, start the node, and drive it with
one-shot CLI commands (`./target/debug/dusk 127.0.0.1:9090 "ps"`).

- **The `dusk_node` binary always binds 9090**, because
  `artifacts/dusk_node/src/lib.rs` hardcodes it — that is a property of the
  prebuilt deliverable, not of the runtime. `dusk_nix::run` takes whatever
  address and port you hand its `InitArgs`, which is how
  `tests/common/src/lib.rs` runs many nodes at once. So check
  `pgrep -af dusk_node` before starting one: another session's node may already
  hold 9090, in which case yours exits and your CLI silently talks to *theirs*.
  Do not kill a node you did not start. Point your own elsewhere — and if you do
  that by editing the hardcoded port, `git diff` that file before committing,
  since pre-commit stashes unstaged work and the hooks will never see it.
- A node built from a different revision answers with the wrong schema, and the
  error names neither the port nor the process: a changed capnp union surfaces as
  `Enum value or union discriminant <n> was not present in the schema`, which
  reads like a schema bug in the branch you just wrote.

## Step 5 — One commit while you work, split it just before review

**Everything goes into a single commit until the work is finished.** Amend it as
you go. Do not split it, do not absorb anything into it, do not reach for
`--fixup` and an autosquash rebase while there is still work coming: each of
those rewrites history that the next change rewrites again, and that churn costs
far more than the whole split costs once.

The split happens **once**, at the end, when the tree is final and just before
Step 6: run the `atomic-commit` skill over that one commit and cut it into the
commits the work actually is.

Review works the same way. A round of comments that changes anything substantial
is not a handful of fixups aimed at five different commits — squash the affected
commits, or the whole branch, back into one, make the changes there, and split
again. Threading a change into commits you are about to rewrite anyway is the
work done twice.

**Do not put the issue number in the commit subject.** A subject is one
imperative sentence about what the commit does; `(#N)` is metadata about why the
work was scheduled, not part of that sentence, and it is already carried by the
`Closes #N` in the PR body. Trailing issue numbers in `git log --oneline` also
read as pull-request merge numbers, which they are not.

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

**Say what you are about to open before you open it.** A pull request appears in
the user's repository under their name; they should read that it is coming in
your message, not discover it as a link in the same breath that announces it is
already there.

**A drive-issue session produces exactly one pull request.** Whatever else the
work turns up — a stale skill, a broken config, a fix to something adjacent —
becomes another commit on the same branch, never a second PR. Asking to drive an
issue is asking for one thing to review and one thing to merge; two PRs make the
user do the bookkeeping the skill exists to do for them.

So `create_pull_request` is called at most once per session. After the PR is
open, more work means: commit onto the same branch, push, and
`update_pull_request` the body to cover it. If a second PR has already been
opened, fold its commits onto the one branch and close it as superseded.

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

## Step 7 — Give the branch back, ask for review, then stop

**Remove the worktree before you ask.** Git lets one worktree hold a branch at a
time, so for as long as yours is checked out on it the user cannot check it out
to review it: `git checkout <branch>` in their own checkout fails, and the first
they hear of the reason is the error. Everything is pushed by this point, so the
worktree holds nothing the remote does not:

```
ExitWorktree(action="remove")
```

Two things get in the way of that, both checked on git 2.53.0:

- `ExitWorktree` only removes a worktree that `EnterWorktree` made **in the same
  session**. A session launched straight into one gets a no-op.
- Step 2 initialised `vendor/capnproto`, and `git worktree remove` refuses on a
  worktree holding a submodule — *working trees containing submodules cannot be
  moved or removed*. `--force` is what gets past it.

So when the tool declines, remove it with git, from outside the worktree:

```
git -C ~/git/dusk worktree remove --force .claude/worktrees/<name>
```

That takes the directory and its `target` and leaves the branch, which is the
half the user needs.

**Check the pull request is still open, and that it has what you pushed**, before
handing it over:

```
mcp__github__pull_request_read(method="get", owner="tomerze", repo="dusk", pullNumber=N)
```

`merged: true` means the review is over. Anything pushed to that branch after the
merge is on no branch anyone will merge, so it needs a **new** pull request, not
another push and a link to a closed one. `head.sha` must also equal the tip you
pushed; if it does not, the push did not land.

This has gone wrong twice, both times silently. #44 and #46 were each merged from
the state of the branch before the last push, so in both cases the commit that
push carried never reached master — and both times it was the commit that fixed
the very instruction the pull request existed to fix.

Then give the user the PR URL and the branch to check out, as a line they can
paste — `git checkout <branch>` — and ask them to review. Removing the worktree
is what made that line work; the user reads the diff and runs the node from
their own checkout, and they should not have to open the PR to learn the branch
name. Then wait.

**End that message with `Ready for review.` on its own line, and write nothing
after it.** It is the one sentence the user is looking for, and it means every
check above has actually passed — not that the work is nearly there, not that it
is pushed and something is still running. If anything is outstanding, say what it
is instead and do not write the line at all. Nothing follows it: no summary, no
caveat, no offer, no question. The line is the end of the message.

Do not merge, do not tidy the branch, and do not read silence or a question as
approval. When comments arrive, verify a claim before implementing it and say so
if you think it is mistaken.

Bring the worktree back when the drive resumes — review comments to answer, or
`drive-issue continue`:

```
EnterWorktree(name=…)
git checkout <branch>
```

A returning worktree is a new one, so Step 2 applies to it again — submodule,
lockfile — and its first build is cold. Measure from the second.

## Step 8 — Merge on explicit approval

```
mcp__github__merge_pull_request(owner="tomerze", repo="dusk", pullNumber=…, merge_method=…)
```

Ask which merge method if it is not obvious from the repository's history.
Afterwards confirm the issue closed and report the merge commit.

---

## `drive-issue continue` — resuming a drive already in flight

`drive-issue continue` means: the PR for this issue is already open, something
has changed since, carry on. Usually the user has edited the tree themselves, or
left review comments, or asked for something the last round missed.

Everything in Steps 1–8 still applies. What is different is that **nothing gets
created except the worktree**. Step 7 removed it to free the branch, so make one
again and check the branch out into it. The branch and the PR already exist; find
them rather than opening new ones. The user's own edits may still be sitting
uncommitted in their checkout rather than on the branch — ask before treating
what you fetch as the whole story.

```
git status                 # what the user changed in the tree
git log --oneline <base>.. # the commits already on the branch
mcp__github__pull_request_read(method="get_review_comments", …)
```

Then:

- **Squash, change, re-split — not one fixup at a time.** For anything
  substantial, `git reset --soft <base>` the affected commits, or the whole
  branch, into one; make the changes there; and run `atomic-commit` over it
  again just before you push. Only a one-line answer to a single comment is
  worth `git commit --fixup=<sha>` plus
  `GIT_SEQUENCE_EDITOR=: git rebase -i --autosquash <base>`. Anything larger is
  cheaper to re-split than to thread through five commits, and either way the
  history states what the work is, never the order you discovered it in.
- **The user's edits in the tree are theirs.** Read them before staging: they are
  a decision, not a draft to improve on. If one looks wrong, say so rather than
  quietly rewriting it.
- **Force-push the rewritten branch.** A pushed branch is not a reason to append
  instead — the history has to state what the work is, not the order it was
  discovered in.
- **Re-run whatever the change invalidated**, and update the numbers in the PR
  body if they moved.
- **Update the PR body**, do not open a new PR. If a new rule or decision came
  out of this round, it belongs in the body's decisions section so the reviewer
  sees it in one place.

Finish the same way Step 7 does: hand back the PR URL and the branch to check
out, then wait. `continue` never merges on its own either.

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
☐ Worked in one commit; split once with `atomic-commit` just before pushing
☐ Committed in the foreground with a long timeout
☐ No issue number in any commit subject
☐ PR body written in the first person, addressed to no one
☐ Pushed; **one** PR opened over the MCP, with `Closes #N`
☐ Anything found later: another commit on the same branch, body updated — never a second PR
☐ Worktree removed, so the branch is free for the user to check out
☐ PR confirmed still open and carrying the pushed tip, not already merged
☐ Branch named in the hand-back, as `git checkout <branch>`
☐ Review requested, the message ending `Ready for review.` and nothing after it
☐ **Waited**
☐ Merged only after explicit approval; issue confirmed closed

On `drive-issue continue`, the issue is already read and the branch and PR
already exist; the worktree is the one thing made again. Nothing else is created,
and the work absorbs into the commits that are already there.

## I do not write comments

Not one — not `//`, not `///`, not `#` in a schema. The user writes every comment
in this codebase. When something genuinely needs saying in one, I say it to the
user in my reply and let them decide; my explanations go in the commit message.
See [CLAUDE.md](../../CLAUDE.md#i-do-not-write-comments).
