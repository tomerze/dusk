---
name: splice
description: Cut a pull request that is too large to review into smaller ones - take the 5-10 commits that are easiest to say yes to and that obviously make the project better, put them in their own PR, and leave the rest behind. Use when a PR has grown past what one person can read, when the user says splice, split the PR, cut this PR up, or asks for another PR out of this one, and whenever a branch is about to be handed over with more commits on it than a reviewer will sit through.
---

# Splice a Pull Request

A splice takes commits **out of an open PR and into another one**. The commits
do not change. What changes is how much a person has to agree to in one sitting.

This is the [atomic-commit](../atomic-commit/SKILL.md) argument one level up. That
skill exists because a reviewer cannot say yes to a 32-file commit; this one
exists because they cannot say yes to a 17-commit PR either. Same reader, same
objection, bigger unit.

Nothing here is about writing code. It is the git and GitHub sequence around a
branch that is already finished.

---

## What to splice out

**Take what is easiest to say yes to and obviously makes the project better.**
Not what is *related* - a splice is chosen by how cheap it is to approve, not by
subject. Five to ten commits is the size that fits in one reading.

A commit belongs in the splice when a reviewer can read it alone and be sure it
is an improvement without holding the rest of the branch in their head: a
one-line bug fix, an ergonomics fix to something that already shipped, a macro
or helper the branch happened to need, a name that was wrong. What stays behind
is the design - the part that only makes sense as a whole.

**Never splice a test away from the code it tests, or a doc page away from the
change it documents.** Each PR carries its own tests and its own prose. A PR that
merges with its tests in the next one merges untested.

## Step 1 - Find out what actually stands alone. Do not guess

For every candidate, try it on master and see:

```bash
git worktree add --detach <scratch> origin/master
for commit in <candidates>; do
  git -C <scratch> cherry-pick --no-commit $commit >/dev/null 2>&1 \
    && echo "OK   $commit" || { echo "FAIL $commit"; git -C <scratch> cherry-pick --abort; }
  git -C <scratch> reset --hard origin/master >/dev/null
done
```

A commit that conflicts is telling you it depends on the branch. Believe it.
Reading the diff and deciding a commit "looks independent" is how a splice turns
into an afternoon of conflict resolution.

## Step 2 - Pick the shape: beside master, or stacked on it

**Beside master.** Everything spliced applies to master cleanly, so the new PR
targets `master` and merges on its own. This is the shape to want.

**Stacked.** The candidates do not stand alone, so the new PR targets master and
the ones after it target the one below: `PR-A → master`, `PR-B → PR-A`, the
original PR → `PR-B`. Every PR in a stack says at the top which PR it sits on and
that that one merges first.

A branch whose commits are one feature end to end usually only splices by
stacking - the cut is a point along the branch, not a subset of it.

## Step 3 - Write the division down, then build

Put the commit subject lines, grouped, in order, with each group's base, in the
reply - the same rule as
[atomic-commit's step 8](../atomic-commit/SKILL.md#8-write-the-subject-lines-down-then-build) -
and build the branches in the same turn. Where a PR is cut is not a
structural decision; the user approves or sends back the PRs themselves.

Decide anything that spans two groups - a docs commit touching one page per
group, a commit whose message covers both - and say in the reply which way it
went and why.

## Step 4 - Rebase on current master first

`git fetch origin && git rebase origin/master`. Master usually moved since the PR
opened, and splicing onto a stale base produces PRs that conflict the moment they
are pushed.

## Step 5 - Snapshot, then rebuild forward

```bash
git tag -f splice-snapshot HEAD
```

Build each branch by cherry-picking onto the one below it. When you are done:

```bash
git diff splice-snapshot <tip>
```

**Empty, or the splice lost something.** This is the only proof that moving
commits around changed nothing but their address.

## Step 6 - Re-read every message you moved

Reordering makes commit messages false. A message describes the tree it arrives
at, and that tree is different now.

- A commit that moved **later** may claim a change the commit now ahead of it
  already made. Cut that paragraph - it is the other commit's statement.
- A commit that moved **earlier** may describe a state that no longer precedes
  it.
- A commit split across two PRs needs its message cut to what it still does.
  Naming a file it no longer touches is a defect.

When a commit's prose is rewritten later on the branch and both halves end up in
the same PR, fold the later one in - the intermediate wording never existed in
this stack, and shipping it so a commit two later can fix it is
[a fixup in the wrong place](../dusk-developer/SKILL.md#a-fixup-belongs-in-the-commit-it-fixes---always).

## Step 7 - Check every tip builds

`cargo check --workspace --all-targets` at each PR's tip, not just the last one.
A reviewer opens the middle PR and builds it.

## Step 8 - Push and open the PRs bottom-up

Push the new branches, force-push the rewritten original, then open the PRs from
the bottom of the stack up, so each one has a base to target.

**Only the PR that finishes the issue carries `Closes #N`.** Merging any of the
others leaves the issue unfinished, and a closing keyword in one of their bodies
would close it anyway - the bottom of a stack targets master, so GitHub acts on
it. Their bodies say **`Part of #N`** instead, and name the PR that closes it.

**The original PR is now a different PR.** It carries a fraction of what it
carried, so its title and body no longer describe it. Rewrite both: what these
commits do, which PR it is stacked on, and which of the known-problems and
review findings actually belong to the part that stayed. Findings about the code
that left go with it.

`gh pr edit` fails on this repository with a GraphQL error about Projects
(classic). Retarget and retitle over REST instead:

```bash
gh api -X PATCH repos/<owner>/<repo>/pulls/<N> \
  -f base=<branch> -f title="<title>" -F body=@<file>
```

## Step 9 - Report the stack

Give the user the PR numbers in merge order, the commit count in each, and every
message you rewrote in step 6, quoted. Branch names and PR titles are
[names you invented](../dusk-developer/SKILL.md#naming-routine---your-names-are-placeholders) -
list them and ask.

## Checklist

- ☐ Rebase on current `origin/master`
- ☐ Cherry-pick every candidate onto a scratch worktree; record OK or FAIL
- ☐ Choose beside-master or stacked from that result, not from reading
- ☐ Keep each group's tests and docs with the code they belong to
- ☐ Put the grouped subject lines with their bases in the reply, and build without waiting
- ☐ `git tag -f splice-snapshot HEAD` before touching anything
- ☐ Rebuild each branch by cherry-pick
- ☐ `git diff splice-snapshot <tip>` is empty
- ☐ Re-read and fix every message that moved, split, or changed neighbours
- ☐ `cargo check --workspace --all-targets` at every tip
- ☐ Push, open PRs bottom-up, each naming the PR it is stacked on
- ☐ Rewrite the original PR's title and body for what it still carries
- ☐ Report the stack, the rewritten messages, and the names you invented

## I do not write comments

Not one - not `//`, not `///`, not `#` in a schema. The user writes every comment
in this codebase. My explanations go in the commit message.
See [dusk-developer](../dusk-developer/SKILL.md#i-do-not-write-comments).
