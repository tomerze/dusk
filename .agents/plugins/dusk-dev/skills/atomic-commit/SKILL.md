---
name: atomic-commit
description: Split work into commits that are each one statement, so every change can be reviewed, stamped and reverted on its own. Use before any commit, when asked for atomic or clean commits, when splitting or rewriting a branch's history, and whenever a diff has grown past one idea. Key words are atomic, atomic commits, clean commits, split the commits, too large to review.
---

# Atomic Commit

## Why this exists - read it before the rules

The user's own words: **"Reviewing your work is the most important part of my
work. and i cannot review your work when it's just one single glob of diff. Each
change need to be stamped. each change need to be reverted."**

That is the whole reason. Not tidiness, not convention, not craft. A commit is
the unit at which a human being says *yes, this specific thing, I have looked at
it and I agree to it.* One 32-file commit is not a decision they can make. It is
a demand that they accept everything or nothing.

So the question for every commit is never "is this diff related?" It is: **can
one person read this alone, decide yes or no about it alone, and undo it alone?**

## A commit is a statement, not a set of files

This is the part I get wrong, and I get it wrong because of how I was trained. I
was fed code. I was not fed commits. So I reach for the thing code-reading has
taught me - files, hunks, textual similarity - and I group by that. That instinct
is corrupt for this task. Do not use it.

**Herd sheep by ideology, not by fleece.** Each change in a diff is a sheep, and
each sheep carries a *statement* - something a person decided. Sheep go in a pen
with the other sheep that carry the same statement. Two sheep that look identical
but stand for different things go in different pens. Twelve sheep that look
nothing alike but all follow from one decision go in one pen.

Concretely, both of these are wrong:

- **Herding by file.** "Twelve programs changed, so twelve commits." Wrong when
  all twelve changed *because one line of schema changed*. That is one decision
  with twelve consequences, and consequences are not decisions.
- **Herding by fleece.** "These two edits are character-identical, so one
  commit." Identical text is not the criterion. Two identical-looking edits made
  for different reasons are two commits.

## The branch is the work, not the diary

**Nothing a commit adds may be deleted or rewritten by a later commit on the same
branch.** The user's own words: **"You can't commit stuff and then overwrite it
in a later commit. Commits are for review. Why would i review something you
delete later?"**

And, on why the rewrite is not optional bookkeeping: **"The history of when
changes were 'discovered' is irrelevant within a PR. Commits are not a dev blog
telling a story. They are a truth of what we want to CHANGE. That's why we
absorb. Thats why we rebase."**

That is the whole justification for absorbing and for rebasing, and it is why
both are routine here rather than something to weigh. A commit answers *what do
we want this tree to become*, never *what happened to me on the way*. The order I
learned things in is my problem, not the reviewer's, and a branch that records it
is asking them to relive my afternoon instead of reading a change.

A branch is not a record of how the work happened. It is the work as it should
have been done, in an order a person can read. Every line a reviewer reads is a
line they are being asked to approve, so a line a later commit deletes has spent
their attention for nothing - and it makes them rule twice on one thing, once on
a version that was never going to survive.

This is discovery order leaking into history, and it is the same disease as a
fixup that does not absorb. I write a page, I am told it is wrong, I write it
again, and both attempts land as commits because both attempts happened. Only the
second is true. The first belongs nowhere: not in a commit, not on the branch,
not in front of a reviewer.

So the split is built from **one diff: base against the final tree.** Never from
the commits in between. The statements are the ones that survive to the end. If a
commit's content is not in the final tree, it was a draft, not a statement -
squash it into the commit that carries the final text, or drop it entirely.

### Compute the base yourself; never trust the one you were handed

The paragraphs above were already in this file on the day I broke the rule
anyway. Asked to rebuild a branch, I took the base named in the request, declared
everything below it already-reviewed history, and never looked. The drafts were
below the line. The author found one in a single glance and stopped reading the
pull request.

So this is not something to remember. It is two commands, run **before** choosing
a base, every time:

```bash
base=$(git merge-base origin/master HEAD)

# 1. commits writing a path that does not exist in the final tree.
#    Every one of them is a draft, whatever else it also does.
for sha in $(git log --format=%H "$base"..HEAD); do
  for path in $(git show --format= --name-only "$sha"); do
    git cat-file -e "HEAD:$path" 2>/dev/null ||
      echo "$(git log -1 --format=%h "$sha") writes dead path: $path"
  done
done

# 2. files written by more than one commit - the same disease in slow motion.
git log --format= --name-only "$base"..HEAD | sort | uniq -c | sort -rn | awk '$1>1'
```

**The scope is whatever those return, not what the request said.** A narrower
base is allowed only once they come back clean above it. A request that names a
base says where somebody noticed the problem; it never says where the problem
ends.

The second command is not a failure on its own - two commits may legitimately add
to one file. It is a list of places to go and check that the later commit does
not remove or rewrite a line the earlier one added. Check each one; do not read
the list and move on.

**A file that both moves and changes: move it first.** If the change lands at the
old path and a later commit moves the file, the first commit writes a path that
does not exist in the final tree, and the move deletes the line it just added.
Ordering it the other way - move first, change at the final path - says the same
thing with no commit touching a dead path. `git log --follow` reads either way;
a reviewer does not.

The tell, and the check to run before reporting: for each commit, ask whether any
later commit touches the same lines. A later commit building on an earlier one is
the branch working; a later commit *replacing* what an earlier one wrote is the
diary, and the two must be squashed into the one statement they were always
trying to be.

This applies across the whole branch, not just the commit being split. When the
range handed over contains an earlier commit whose content has since been
rewritten, that commit is in scope even if nobody named it.

## The procedure

### 1. List the decisions, not the files

Read the whole diff and write down every point where a *person chose something*.
Ignore the file layout entirely while doing this. A decision looks like:

- a schema line changed shape
- a name was wrong and is now right
- an explanation moved from source into a commit message
- a helper died because nothing calls it any more

### 2. Name each decision as one imperative sentence

Write the subject line **before staging anything**. The subject line is the
design tool, not the packaging. If I cannot write it in one sentence, I do not
yet know what the commit is, and staging files will not teach me.

### 3. The and-test

**If the subject line needs an "and" joining two actions, it is two commits.**

Apply it mechanically: delete everything after the "and". Does the remaining
clause still describe a change that could be true or false on its own? If yes,
they are two statements and they split.

- "Split `Dusk.Stream` into a `Sink` and a `Stream`" - the "and" is inside one
  noun phrase, naming the two halves of one split. One statement. Fine.
- "Split `Dusk.Stream` into a `Sink` and a `Stream`, **and** split every
  implementation" - the second clause stands alone. Two statements. Split.

The same tell applies to a subject line that needs a comma-spliced list, a
semicolon, or the word "also". Those are all "and" wearing a hat.

**A vaguer name is not a passing and-test.** This is the cheat I reach for, and
it is worse than failing the test outright. When the honest name needs an "and",
there is always a name one level up that does not - and reaching for it hides the
second statement instead of removing it. If the only way to drop the "and" is to
go more abstract, that *is* the test failing.

So after writing a subject line, check it downward: name every hunk in the commit
and ask whether the subject line predicts it. A hunk the subject does not predict
is a second statement wearing the first one's name.

### 4. The tumor test - coupling is not sameness

**"It breaks if I separate them" is not "they are the same statement."** This is
the single reason I bundle, every time, and it survives every rule that does not
name it directly.

The mechanism: I read a diff and my attention goes to what stops working if a
hunk is removed. That is functional coupling, and it *feels* like meaning, so I
merge the two changes and then write a name broad enough to cover both. But
coupling only tells me the commits must be **ordered**. It says nothing about
whether they are one thing.

Two changes that must ship together are still two commits when they are two
statements. The tree being broken between them is the expected, allowed cost -
see [what a commit does not have to do](#what-a-commit-does-not-have-to-do).

The sheep for this one: a pen of twelve identical sheep with one *different*
animal walking in the middle of them is not thirteen sheep of one colour. It is
twelve sheep and a tumor, and the tumor is the change I slipped in because it had
to arrive at the same time.

Ask of every hunk that is not one of the twelve: *is this the same statement, or
does it merely have to land together?* If the second, it is its own commit,
ordered next to this one.

### 5. The consequence test

**Everything forced by a decision belongs in that decision's commit, however
many files it touches.** A consequence is not a separate statement - it is what
the statement *means* in code.

Change one line of a schema and eleven programs stop compiling: those eleven
programs are the consequence, they are not eleven decisions, and they go in the
one commit. A commit touching thirty files is fine when the thirty files are one
statement. Size is not the test. "One statement" is the test.

Ask of each hunk: *would this hunk exist if the decision had not been made?* No →
it is a consequence, it belongs with the decision. Yes → it is its own sheep.

### 6. Prose is always its own commit

Documentation, `AGENTS.md` and the skills under
`.agents/plugins/dusk-dev/skills/` get their own commits, even when the words changed *because* of a code decision. They are read
by different people, reviewed differently, and reverted independently.

Split prose further when it addresses different readers - the published docs site
and the repository's own working agreements are two commits, not one.

**And the branch is ordered by what a reviewer must think about.** Imports,
moves, real changes, tests, text - in that order. A commit that only changes
`use` paths, or only moves code without changing it, goes at the start, where
it can be waved through; the real changes follow; the tests come after the code
they test; and every commit that changes only what a person reads - comments,
docs, the skills - sits at the end, after the last commit that
changes what a machine runs. A reviewer reads the code before the words about
it, and a text commit in the middle of the code is a page of prose between two
hunks it does not describe yet.

### 7. Write the message, and put the explanation in it

Everything I wanted to say in a comment goes here instead: why this exists, what
it replaced, what was rejected, what it is a step toward. See the
[no-comments rule](../dusk-developer/SKILL.md#i-do-not-write-comments) - a comment
explaining the change is a commit message that leaked into the source and will
outlive the reader it was written for.

If I find myself writing a comment while making a change, that is a signal I have
not written the commit message yet. Write it there.

No conventional-commit prefixes. No `feat:`, `fix:`, `chore:`, `docs:`,
`refactor:`. One imperative sentence, in the style of `git log --oneline -20`.

**A subject never starts with "Say".** A commit changes a tree; it does not say
anything. The verb is what was done to the file - update, change, add, remove,
fix, document - and that holds for prose commits exactly as for code: "Update
docs for how a prompt opens now", never "Say how a prompt opens now in the
docs". A subject built around "say" is naming the sentence the file gained
instead of the act done to the file.

**A docs commit starts "Update docs", and a tests commit starts "Add tests".** A
commit that changes only files under `docs/` has a subject that begins
`Update docs`; one that changes only files under `tests/` begins `Add tests`, or
`Update tests` when it changes tests that already exist rather than adding any.
They are the low-stakes commits, and the reviewer skips them by their first two
words without opening them, so the marker is exact: not "Document", not "Test
the", not "Update the Python API docs". The pre-bash hook refuses a push
whose docs or tests commits do not start that way.

### 8. Write the subject lines down, then build

Put **the list of subject lines, in order** in the reply - not the file
groupings, not a script - and build the commits in the same turn. The subject
lines are the statements, and the commits are what the user approves or sends
back; they review them as commits, not as a plan. How a diff is divided is not
a structural decision, so it does not wait for a nod: a split they dislike is
redone, and that is cheaper than a stall. The user, verbatim: **"just do what
you see fit. if i dont like it i will not approve it. this is what we always do
with non structural decisions."**

## What a commit does *not* have to do

**It does not have to compile.** This one matters, because it is the belief that
most often makes me bundle two statements into one commit: I notice that
splitting them leaves the middle commit broken, and I quietly merge them to keep
the tree green.

The user, verbatim: **"i don't fucking care if it doesn't compile or if it makes
no sense checking out a random commit in the tree. ideally make it compile for
git bisect but that's ideally. NOT A MUST."**

So: a green intermediate commit is a nice-to-have, worth a little effort, and
worth **nothing** compared to a reviewable one. When the two conflict,
reviewability wins every time. Never bundle to stay green.

The same goes for tests passing at every commit. Ideal. Not a requirement.

**And never go looking.** Do not check out each commit and compile it, and do
not `cargo check` anything but the tip. Walking a seventeen-commit branch
through `cargo check --workspace` costs many minutes of the user's time and
tokens to measure the one property they have already said is not required. If
some later reader - a review agent, a hook, me on the next pass - reports a
non-compiling intermediate commit as a defect, that report is wrong; the answer
is to say so, not to rewrite the history to satisfy it.

The user, verbatim, when I was about to compile all seventeen: **"Don't ever
fucking compile seventeen commits! THE PROPERTY EVERY COMMIT MUST COMPILE
DOESN'T EVEN EXIST! IT'S A NICE TO HAVE NOT A HARD LIMIT."**

## The ways I actually get this wrong

**Failure - one glob.** PR #32 shipped as a single commit: 32 files, a schema
split, twelve program conversions, a helper deleted, a rename, the shell's
close relocated, two tests, five doc pages, `AGENTS.md` and a skill. Every
sentence of it was true and none of it could be reviewed. The user could not say
yes to the rename without also saying yes to the schema. That is not a review,
it is a hostage situation.

**Failure - the overcorrection.** Told to split it, I proposed *one commit per
program*: twelve commits doing the same thing for the same reason. That is
herding by fleece - I saw twelve files and made twelve pens. All twelve programs
changed because one line of `sh.capnp` changed. One decision, one pen.

**Failure - the tumor.** Splitting it on the second attempt, I put the twelve
program conversions in one commit and slipped `sh`'s own change - moving the
close of the client's stream out of `Interpreter::exec` and into `ShPortal.sh` -
into the middle of them. It had to land at the same time or the shell would stop
closing anything, so I called it a consequence. It is not: "who closes the
client's stream" is a different statement from "what a program says in `output`",
and it is revertible on its own. I then named the commit "Have a program say its
outcome instead of implying it", which is abstract enough that the relocation
hides inside it - the vaguer-name cheat, in the very next split I made after
writing this file.

**Failure - the diary.** On `esp32-node` I wrote the node artifacts docs page in
my own words - 194 new lines in one commit - having been asked to *move* the
text the user had written. Told what I had done, I restored their text in a later
commit, which deleted 126 of those 194 lines. Six further commits then rewrote
the same page again as they edited it. Every one of those went to review. The
user: **"Why would i review something you delete later?"** The error is not that
those commits were badly divided. It is that they existed at all: only the text
that survives to the final tree is a statement, and every version before it was a
draft that should never have reached a commit. The split is built from the base
against the final tree for exactly this reason.

**Failure - trusting the handed-in base.** Told that a branch showed a reviewer
text it later deleted, I wrote the rule above into this file - and then rebuilt
only the range the request had named, and reported the work done. The branch
still opened on a commit adding `impls/esp32` whose own message said "The crate
and struct names are placeholders", renamed wholesale to `impls/portable` further
along; five commits wrote paths absent from the final tree. The author:
**"This commit is enough for me to stop reading the PR."** Writing the rule down
did not stop me applying it to a range instead of a branch. Only running the two
commands does, which is why they are commands and not a paragraph.

Between those failures is the actual skill, and neither file count nor diff size
nor what-breaks-if-I-split will find it. Only the statement will.

## Mechanics

**Before rewriting any history, snapshot the final tree.** `git add -A && git
commit --no-verify -m "WIP snapshot"`, then `git tag -f split-snapshot/<branch> HEAD` - one tag per branch, so two splits on two worktrees never share one.
Everything after that is recoverable, and `git diff split-snapshot/<branch> HEAD` at the
end must come back empty - that is the proof the rewrite lost nothing.

**Unpick the commit, then commit forward out of the working tree.** Do not
reconstruct each statement's content from the snapshot; let git hand you the
whole change as unstaged edits and take it apart from there.

```
git reset <base>                      # the whole change is now unstaged
git add <the paths this statement owns>
git commit --no-verify -m "<subject>" -m "<body>"
…                                     # once per statement
git diff split-snapshot/<branch> HEAD # empty, or the split lost something
git tag -d split-snapshot/<branch>   # once it is empty
```

Reset against the base the split rebuilds from, not `HEAD^`. When the tree was
dirty the tip is the snapshot commit, and `HEAD^` unpicks that instead, leaving
the WIP commit in the history under everything you then build.

For a commit in the middle of the branch, `git rebase -i <base>` marking it
`edit` puts you in the same place: `git reset HEAD^`, commit forward, then
`git rebase --continue`. A rebase in progress is a normal working state.

For a file that spans two statements, stage the hunks rather than the file:
`git add -p <path>`, or write the intermediate content and stage it.

**Every commit in a split is `--no-verify`, and this is the whole of the speed.**
Pre-commit stashes the unstaged changes before it runs and restores them after,
on every commit - and during a split the working tree is full of the statements
not committed yet, so it stashes and restores the entire remaining change each
time. The checks are also being asked about trees that will never ship: an
intermediate state can have a moved file and not its caller, and fail a lint for
a reason the final tree does not have.

The final tree is the one that matters and it is byte-identical to the snapshot,
which was already verified. So verify once, at the end, not once per commit:

```
uv run pre-commit run --from-ref <base> --to-ref HEAD
```

**Never `git add .` or `git add -A`** when building a real commit - only the
snapshot may be taken that way. Stage the paths the statement owns.

**A commit that does run the hooks gets the maximum timeout.** Pre-commit stashes
the unstaged changes before it runs; killing it mid-hook leaves that stash
unrestored and the user's uncommitted work gone. If one does get killed, the work
is in the patch file named in the hook's
`[INFO] Stashing unstaged files to <path>` line: `git apply <path>`.

**A rebase in progress is a normal working state, not a blocker.** The working
tree handed to me is usually the leftover of the commit being split. Do not raise
it, do not warn, do not ask. Commit onto the detached HEAD as usual, then
`git rebase --continue`.

**A fixup belongs in the commit it fixes.** Discovery order must never show up in
the history - see
[the working agreement](../dusk-developer/SKILL.md#a-fixup-belongs-in-the-commit-it-fixes---always).
Force-push the rewritten branch; a pushed branch is not a reason to append.

## Checklist

- ☐ Build from one diff: the base against the **final tree**, never the commits
      in between
- ☐ Check that no commit writes a line a later commit deletes; squash any that do
- ☐ Read the whole diff and list the **decisions**, ignoring file layout
- ☐ Write a one-sentence subject line for each, before staging anything
- ☐ Run the and-test on every subject line; split what fails, and never
      escape it by choosing a vaguer name
- ☐ Name every hunk and ask whether the subject line predicts it
- ☐ Run the tumor test: for anything that merely has to land together,
      order it next to the commit instead of inside it
- ☐ Run the consequence test on every hunk; fold consequences into their decision
- ☐ Pull prose out into its own commits, split by reader
- ☐ Put the subject lines in the reply, in order, and build without waiting
- ☐ Snapshot and tag, `git reset HEAD^`, then commit forward, staging named paths only
- ☐ Every commit in the split is `--no-verify`; the hooks run once at the end
- ☐ `git diff split-snapshot/<branch> HEAD` is empty, and the tag deleted

## I do not write comments

Not one - not `//`, not `///`, not `#` in a schema. The user writes every comment
in this codebase. When something genuinely needs saying in one, I say it to the
user in my reply and let them decide; my explanations go in the commit message.
See [dusk-developer](../dusk-developer/SKILL.md#i-do-not-write-comments).
