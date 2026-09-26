---
name: drive-issue
description: Take a GitHub issue in this repository from "here is the issue number" to a merged PR - read it over the GitHub MCP, work in a git worktree, branch from current master, get the tree buildable, record every decision through the dilemma-triage agent, split the work through the atomic-commit agent, have the self-review and race-screen agents sign it off, push, open the single PR that links the issue with the decisions ranked, ask for code review, and merge only once the user approves. Use this skill whenever the user hands over a dusk issue number or issue URL and asks to drive, complete, or finish it. Also use it for "drive-issue continue", which resumes a drive whose PR is already open - absorbing the user's own edits and review comments into the existing commits rather than opening anything new.
---

# Drive a Dusk Issue to a Merged PR

The mechanics of shipping an **issue**: reading it, the worktree it is built in,
the branch cut for it, and the pull request that closes it. Everything about how
the work itself is done - decisions, commits, the split, the hand-off, the review
agents, the merge - is the `harness` skill, and this one runs on top of it.

This skill takes an issue. Without one there is nothing here to run: work on a
branch the user is already on goes to `activate`, and to `harness` when the user
puts the session under it.

## Step 0 - Put the session under the harness

Invoke the `dusk-dev:activate` skill, then the `dusk-dev:harness` skill, before
anything else. Every step below assumes the harness's rules are in force, and
none of them is repeated here.

---

## Step 1 - Read the issue over the MCP

The repository is whatever `origin` points at - read it rather than assuming,
and use that `owner` and `repo` in every call below:

```
git remote get-url origin
mcp__github__issue_read(method="get",          owner=…, repo=…, issue_number=N)
mcp__github__issue_read(method="get_comments", owner=…, repo=…, issue_number=N)
```

Write its definition of done out as a checklist. Every bullet is something the
PR description has to answer for - if it asks for measurements you owe numbers,
if it asks for documented incompatibilities you owe a list.

## Step 2 - Enter a worktree and make it buildable

Use the `EnterWorktree` tool, and rename the branch it cuts. `EnterWorktree(name=…)`
puts the worktree at `.claude/worktrees/<name>` but names the branch
`worktree-<name>`, so the two disagree from the start and the `<branch>` in every
path below names nothing on disk:

```
git branch -m <name>
```

A fresh dusk worktree then does not build until:

```
git submodule update --init vendor/capnproto        # else dusk_capnp's build.rs fails
cp /home/<user>/git/dusk/Cargo.lock .               # Cargo.lock is gitignored
mv ~/.cache/dusk-target/<branch> target             # this branch's parked build directory - never `cp` one
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
- **Never copy a `target/` directory.** Not the user's - it is 9.6G, and on
  ext4 there is no reflink, so `cp -a --reflink=auto` copies every byte for
  real. Not with hardlinks either: `cp -al` shares inodes, and cargo rewrites
  fingerprint files in place, so your build's state lands in the tree the user
  builds in. **If you are about to type `cp` and `target` in the same command,
  stop.**

  A worktree's build directory is **parked** instead, and moved back - a
  rename, which costs nothing:

  ```
  mkdir -p ~/.cache/dusk-target
  mv target ~/.cache/dusk-target/<branch>            # before ExitWorktree
  mv ~/.cache/dusk-target/<branch> target            # after EnterWorktree
  ```

  Measured on this machine: **0.06s to move 340M**, against **1m35s** to
  compile one crate's dependency tree from cold and **0.08s** once it is warm.
  An empty `target/` costs that on every `check`, `clippy` and `build`, and a
  drive rebuilds its branch many times, so it is most of the waiting in a whole
  session. **The first drive of a branch has nothing parked: build it cold
  once, and park it on the way out.** That is the price, and it is paid once.

  **Name the worktree after the branch, every time.** Cargo keys artifacts by
  absolute path, so a parked directory is only warm coming back to the path it
  was built at - `sh-flip`, `sh-flip-2`, `sh-flip-3` are three cold trees.

  **A parked directory moved to a different path is worse than useless: it
  breaks the build.** `dusk_capnp`'s build script installs the vendored Cap'n
  Proto compiler into `target/` and records its absolute path, so at a new path
  the compiler is there but its includes are not, and the build fails with
  `Import failed: /capnp/c++.capnp` and `capnp compile: Premature end of file` -
  which reads like a broken submodule, not like a stale cache. Measured. If you
  have already done it:

  ```
  rm -rf target/debug/build/dusk_capnp-*
  ```

  That rebuilds the compiler for this path and keeps the rest of the directory
  warm.

  **One shared `CARGO_TARGET_DIR` for every worktree is not the answer either.**
  Measured: the same crate from a second path recompiled in the same 1m35s as
  cold, because that sharing is per-path too; all it adds is cargo's lock
  between concurrent builds. And **`~/.cache/pre-commit` is not the expensive
  part** - 332M, in `$HOME`, already shared by every worktree; the clippy hook
  is slow because it compiles into the worktree's own `target/`.
- **Do not symlink build inputs in from another checkout.** A symlink whose
  target is deleted while you work turns into a failure nowhere near its cause -
  `fs::create_dir_all` on a dangling symlink fails with `File exists`, not with
  anything naming the link.
- Never work in the user's checkout, and never `cd` out of the worktree.

## Step 3 - Branch from current master, and check it again later

```
git fetch origin
git log origin/master --oneline -3
```

Branch from `origin/master`, not from whatever the worktree started on.

`harness` says to re-check `origin/master` before the push and to re-run the
build after the rebase. In a worktree that can invalidate the setup Step 2 made,
not only the numbers: master can delete the build infrastructure the worktree
was built around.

Past failure: while driving #6, master moved twice. The second move (#11)
deleted `dusk_llm/build.rs`, `dusk_llm/model.json`, the `vendor/ik_llama.cpp`
submodule and the compile-time sh-entry side-channel. The setup this skill had
prescribed an hour earlier no longer applied, and a step from it - a symlink
into the main checkout - went dangling mid-run and failed a commit's clippy hook
with an error that named neither the symlink nor the branch that removed it.

## Step 4 - What a pull request carries for an issue

Open the pull request as `harness` says. For an issue the body also carries:

- **`Closes #N`**, so the merge closes the issue. The harness refuses a body
  without it. Exactly one pull request carries it: when a drive is
  [spliced](../splice/SKILL.md) into a stack, the others say **`Part of #N`**
  instead, because merging one of them leaves the issue unfinished and a closing
  keyword would close it anyway. The harness takes either.
- **One section per acceptance criterion in the issue**, in the issue's own
  terms - the definition of done from Step 1, answered.

**Do not put the issue number in the commit subject.** It is metadata about why
the work was scheduled, already carried by `Closes #N`, and a trailing number in
`git log --oneline` reads as a pull-request merge number, which it is not.

## Step 5 - Remove the worktree before the hand-off

**Remove the worktree before you ask for review.** Git lets one worktree hold a
branch at a time, so for as long as yours is checked out on it the user cannot
check it out to review it: `git checkout <branch>` in their own checkout fails,
and the first they hear of the reason is the error. Everything is pushed by this
point, so the worktree holds nothing the remote does not - except its review
folder and its build directory: the folder is copied out so the decisions
survive, the build directory parked so the next session starts warm:

```
mkdir -p ../../../review/<branch>
cp -r review/<branch>/. ../../../review/<branch>/
mv target ~/.cache/dusk-target/<branch>
ExitWorktree(action="remove")
```

**Copy the review folder out first.** The hooks write it to
`<cwd>/review/<branch>/`, so a drive run in a worktree keeps it inside the
worktree and `ExitWorktree` takes it along with everything else. The reviewer
loses nothing directly - the decisions are already in the pull request body -
but the review agents run *after* the hand-off, by which point the session is
back in the main checkout, and `decision-ranker` re-reads `decisions.md` to rank
it. Without the copy it finds nothing to rank, and `report.html` renders there
without the decisions.

Two things about that copy, both measured. The folder is named after the branch
with any `worktree-` prefix stripped - `branch_name` in
`.agents/plugins/dusk-dev/hooks/harness.py` - so it matches `<branch>` only once
Step 2's rename has run; a session launched straight into a worktree, which Step 2 never
touched, has its folder under the stripped name. And the destination has to
exist first: `/review/` is in `.gitignore`, so a checkout that has never been
reviewed has no `review/` at all, and `cp -r <folder> <missing>/` then exits 0
having copied the *contents* rather than the folder - the decisions land at
`review/decisions.md` and `decision-ranker` still finds nothing.

Three things get in the way of the removal itself, all checked on git 2.53.0:

- `ExitWorktree` only removes a worktree that `EnterWorktree` made **in the same
  session**. A session launched straight into one gets a no-op.
- **`ExitWorktree(action="remove")` refuses while the branch carries commits the
  original branch does not**, which after a drive is every commit of the work:
  *Worktree has N commits on worktree-<name>. Removing will discard this work
  permanently.* It counts them against the branch `EnterWorktree` created, under
  the name it created, even after Step 2's rename. `discard_changes: true` is
  what gets past it, and it is safe **only once `origin/<branch>` carries the
  tip** - check that first, and the local ref is then the only thing at stake:

  ```
  git rev-parse HEAD origin/<branch>        # the two must match
  ExitWorktree(action="remove", discard_changes=true)
  ```

  A deleted local ref costs the user nothing either, because
  `git checkout <branch>` recreates it from the remote when the name is
  unambiguous.
- Step 2 initialised `vendor/capnproto`, and `git worktree remove` refuses on a
  worktree holding a submodule - *working trees containing submodules cannot be
  moved or removed*. `--force` is what gets past that, if the removal is ever
  done by hand from outside the worktree.

**Do not reach for `git -C <the main checkout> worktree remove`.** A
worktree-isolated session cannot run it: the harness refuses a git command that
redirects to the shared checkout with `-C`, and says so rather than doing it.
`ExitWorktree` is the only route from inside.

The harness checks the worktree is gone before it accepts `Ready for review.`

Bring the worktree back when the drive resumes - review comments to answer, or
`drive-issue continue`:

```
EnterWorktree(name=…)
git checkout <branch>
```

A returning worktree is a new one, so Step 2 applies to it again - submodule,
lockfile - and its first build is cold. Measure from the second.

## Step 6 - After the merge

The merge happens as `harness` says, on explicit approval only. Afterwards
confirm the issue closed, and report the merge commit.

## `drive-issue continue` - resuming a drive already in flight

`drive-issue continue` means: the PR for this issue is already open, something
has changed since, carry on. Usually the user has edited the tree themselves, or
left review comments, or asked for something the last round missed.

Everything in Steps 0–6, and everything in `harness`, still applies. What is different is that **nothing gets
created except the worktree**. Step 5 removed it to free the branch, so make one
again and check the branch out into it. The branch and the PR already exist; find
them rather than opening new ones. The user's own edits may still be sitting
uncommitted in their checkout rather than on the branch - ask before treating
what you fetch as the whole story.

```
git status                 # what the user changed in the tree
git log --oneline <base>.. # the commits already on the branch
mcp__github__pull_request_read(method="get_review_comments", …)
```

Then:

- **Squash, change, re-split - not one fixup at a time.** For anything
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
  instead - the history has to state what the work is, not the order it was
  discovered in.
- **Re-run whatever the change invalidated**, and update the numbers in the PR
  body if they moved.
- **Update the PR body**, do not open a new PR. If a new rule or decision came
  out of this round, it belongs in the body's decisions section so the reviewer
  sees it in one place.

Finish the same way `harness` does: hand back the PR URL and the branch to check
out, then wait. `continue` never merges on its own either.

## Keeping this skill true

This file describes an environment that changes under it. When a step here turns
out to be wrong - a submodule that no longer exists, a build cost that no longer
applies, a gotcha that was fixed - **say so and fix the skill**, in its own
commit, separate from the issue being driven. A skill that misdirects the
next agent is a defect exactly like a wrong comment.

---

## Checklist

`harness` carries the checklist for the work itself. On top of it:

☐ `activate` and `harness` invoked first
☐ Issue and comments read over MCP; definition of done written out
☐ Worktree entered; `vendor/capnproto` initialised, lockfile copied
☐ `origin/master` fetched; branch cut from it
☐ Baseline built and measured first, if the issue asks for a comparison
☐ PR body carries `Closes #N` and one section per acceptance criterion
☐ Worktree removed, so the branch is free for the user to check out
☐ Issue confirmed closed after the merge

On `drive-issue continue`, the issue is already read and the branch and PR
already exist; the worktree is the one thing made again. Nothing else is created,
and the work absorbs into the commits that are already there.

## I do not write comments

Not one - not `//`, not `///`, not `#` in a schema. The user writes every comment
in this codebase. When something genuinely needs saying in one, I say it to the
user in my reply and let them decide; my explanations go in the commit message.
See [dusk-developer](../dusk-developer/SKILL.md#i-do-not-write-comments).

