---
name: drive-issue
description: Take a GitHub issue in this repository from "here is the issue number" to a merged PR - read the issue, work in a git worktree, branch from current master, get the tree buildable, split the work with the atomic-commit skill, have the race-screen and self-review agents read the branch, push, open the single PR that closes the issue, ask for review, and merge only once the user approves. Use this skill whenever the user hands over a dusk issue number or issue URL and asks to drive, complete, or finish it. Also use it for "drive-issue continue", which resumes a drive whose PR is already open - absorbing the user's own edits and review comments into the existing commits rather than opening anything new.
---

# Drive a Dusk Issue to a Merged PR

The mechanics of shipping an **issue**: reading it, the worktree it is built in,
the branch cut for it, the review it gets before anyone else reads it, and the
pull request that closes it.

## Step 0 - Load what there is to know

Invoke the `dusk-dev:activate` skill before anything else. It loads the
`dusk-developer` skill and says what the hooks refuse in every session.

---

## Step 1 - Read the issue

The repository is whatever `origin` points at - read it rather than assuming:

```
git remote get-url origin
gh issue view <N> --comments
```

or `mcp__github__issue_read` with `method="get"` and `method="get_comments"`
when the GitHub MCP is connected.

Write its definition of done out as a checklist. Every bullet is something the
PR description has to answer for - if it asks for measurements you owe numbers,
if it asks for documented incompatibilities you owe a list.

## Step 2 - Enter a worktree and make it buildable

Use the `EnterWorktree` tool, and rename the branch it cuts. `EnterWorktree(name=…)`
puts the worktree at `.claude/worktrees/<name>` but names the branch
`worktree-<name>`, so the two disagree from the start:

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
  **The first drive of a branch has nothing parked: build it cold once, and
  park it on the way out.**

  **Name the worktree after the branch, every time.** Cargo keys artifacts by
  absolute path, so a parked directory is only warm coming back to the path it
  was built at.

  **A parked directory moved to a different path is worse than useless: it
  breaks the build.** `dusk_capnp`'s build script installs the vendored Cap'n
  Proto compiler into `target/` and records its absolute path, so at a new path
  the compiler is there but its includes are not, and the build fails with
  `Import failed: /capnp/c++.capnp` and `capnp compile: Premature end of file`.
  If you have already done it:

  ```
  rm -rf target/debug/build/dusk_capnp-*
  ```

  **A build killed mid-write leaves a truncated artifact cargo still thinks is
  fresh.** Measured: a session that ended during a release build left
  `libtokio-<hash>.rlib` at exactly 4 MiB, and every build after it failed with
  `crate tokio required to be available in rlib format, but was not found in
  this form`. Delete that one `.rlib` and its `.rmeta`, and cargo rebuilds only
  that crate.
- **Do not symlink build inputs in from another checkout.** A symlink whose
  target is deleted while you work turns into a failure nowhere near its cause.
- Never work in the user's checkout, and never `cd` out of the worktree.

## Step 3 - Branch from current master, and check it again later

```
git fetch origin
git log origin/master --oneline -3
```

Branch from `origin/master`, not from whatever the worktree started on. Check
it again before the push: the user merges their own PRs while a drive runs, and
a rebase can invalidate the setup Step 2 made, not only the numbers.

## Step 4 - The commits

Work in one commit and amend it as you go. When the tree is final, split it with
the `atomic-commit` skill, so the branch is the commits the work is. **Do not
put the issue number in a commit subject**: `Closes #N` in the PR body carries
it, and a trailing number in `git log --oneline` reads as a pull-request merge
number, which it is not.

Run `git commit` in the foreground with `timeout: 600000`; the hook refuses it
otherwise.

## Step 5 - Review before the pull request

Dispatch two agents over the branch, together:

- **`dusk-dev:race-screen`** reads the diff for anything that can interleave on
  the cooperative executor, and dispatches `dusk-dev:race-inspector` when it
  finds some.
- **`dusk-dev:self-review`** reads every commit against the working agreements
  and the issue's definition of done, ending `VERDICT: pass` or
  `VERDICT: findings`.

Fix what they find in the commits it belongs to, and say in the PR body what
they found and what was done about it.

## Step 6 - The pull request

Push the branch and open **one** pull request for it. The body carries:

- **`Closes #N`**, so the merge closes the issue. When the drive is
  [spliced](../splice/SKILL.md) into a stack, only the PR that finishes the
  issue says it; the others say **`Part of #N`**.
- **One section per acceptance criterion in the issue**, in the issue's own
  terms - the definition of done from Step 1, answered.
- Measured numbers in a table, naming the host, the method and the run count.
- Every incompatibility, workaround, deliberate omission, and decision the
  reviewer might have made differently.

Check the pull request is open and carries the tip you pushed before handing it
over - `headRefOid` must equal `git rev-parse HEAD`.

## Step 7 - Remove the worktree before the hand-off

**Remove the worktree before you ask for review.** Git lets one worktree hold a
branch at a time, so for as long as yours is checked out on it the user cannot
check it out to review it. Everything is pushed by this point, so the worktree
holds nothing the remote does not - except its build directory, which is parked
so the next session starts warm:

```
mv target ~/.cache/dusk-target/<branch>
ExitWorktree(action="remove")
```

Three things get in the way of the removal, all checked on git 2.53.0:

- `ExitWorktree` only removes a worktree that `EnterWorktree` made **in the same
  session**. A session launched straight into one gets a no-op.
- **`ExitWorktree(action="remove")` refuses while the branch carries commits the
  original branch does not**, which after a drive is every commit of the work.
  `discard_changes: true` is what gets past it, and it is safe **only once
  `origin/<branch>` carries the tip** - check that first:

  ```
  git rev-parse HEAD origin/<branch>        # the two must match
  ExitWorktree(action="remove", discard_changes=true)
  ```

- `git worktree remove` refuses on a worktree holding a submodule. `--force` is
  what gets past that, if the removal is ever done by hand from outside.

Then give the user the PR URL and the branch to check out, as a line they can
paste - `git checkout <branch>` - and wait.

## Step 8 - Merge on explicit approval

Merge only when the user approves this pull request, explicitly; the hook asks
before any merge. Afterwards confirm the issue closed, and report the merge
commit.

## `drive-issue continue` - resuming a drive already in flight

`drive-issue continue` means: the PR for this issue is already open, something
has changed since, carry on. Nothing gets created except the worktree - Step 7
removed it to free the branch, so make one again and check the branch out into
it:

```
EnterWorktree(name=<branch>)
git checkout <branch>
```

A returning worktree is a new one, so Step 2 applies to it again. The user's
own edits may still be sitting uncommitted in their checkout rather than on the
branch - ask before treating what you fetch as the whole story.

- **Squash, change, re-split - not one fixup at a time.** For anything
  substantial, `git reset --soft <base>` the affected commits into one, make the
  changes there, and run `atomic-commit` over it again before you push.
- **The user's edits in the tree are theirs.** Read them before staging: they are
  a decision, not a draft to improve on.
- **Force-push the rewritten branch**, update the PR body, and re-run whatever
  the change invalidated. Do not open a new PR.

## Keeping this skill true

When a step here turns out to be wrong - a submodule that no longer exists, a
build cost that no longer applies, a gotcha that was fixed - **say so and fix the
skill**, in its own commit. A skill that misdirects the next agent is a defect
exactly like a wrong comment.

## Checklist

☐ `activate` invoked first
☐ Issue and comments read; definition of done written out
☐ Worktree entered; `vendor/capnproto` initialised, lockfile copied
☐ `origin/master` fetched; branch cut from it, and checked again before the push
☐ Baseline built and measured first, if the issue asks for a comparison
☐ Branch split with `atomic-commit`
☐ `race-screen` and `self-review` run, their findings fixed or stated
☐ PR body carries `Closes #N` and one section per acceptance criterion
☐ Worktree removed, so the branch is free for the user to check out
☐ Merged only after explicit approval; issue confirmed closed

## I do not write comments

Not one - not `//`, not `///`, not `#` in a schema. The user writes every comment
in this codebase. When something genuinely needs saying in one, I say it to the
user in my reply and let them decide; my explanations go in the commit message.
See [dusk-developer](../dusk-developer/SKILL.md#i-do-not-write-comments).
