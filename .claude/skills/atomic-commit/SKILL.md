---
name: atomic-commit
description: Split work into commits that are each one statement, so every change can be reviewed, stamped and reverted on its own. Use before any commit, when asked for atomic or clean commits, when splitting or rewriting a branch's history, and whenever a diff has grown past one idea. Key words are atomic, atomic commits, clean commits, split the commits, too large to review.
---

# Atomic Commit

## Why this exists — read it before the rules

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
taught me — files, hunks, textual similarity — and I group by that. That instinct
is corrupt for this task. Do not use it.

**Herd sheep by ideology, not by fleece.** Each change in a diff is a sheep, and
each sheep carries a *statement* — something a person decided. Sheep go in a pen
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

- "Split `Dusk.Stream` into a `Sink` and a `Stream`" — the "and" is inside one
  noun phrase, naming the two halves of one split. One statement. Fine.
- "Split `Dusk.Stream` into a `Sink` and a `Stream`, **and** split every
  implementation" — the second clause stands alone. Two statements. Split.

The same tell applies to a subject line that needs a comma-spliced list, a
semicolon, or the word "also". Those are all "and" wearing a hat.

**A vaguer name is not a passing and-test.** This is the cheat I reach for, and
it is worse than failing the test outright. When the honest name needs an "and",
there is always a name one level up that does not — and reaching for it hides the
second statement instead of removing it. If the only way to drop the "and" is to
go more abstract, that *is* the test failing.

So after writing a subject line, check it downward: name every hunk in the commit
and ask whether the subject line predicts it. A hunk the subject does not predict
is a second statement wearing the first one's name.

### 4. The tumor test — coupling is not sameness

**"It breaks if I separate them" is not "they are the same statement."** This is
the single reason I bundle, every time, and it survives every rule that does not
name it directly.

The mechanism: I read a diff and my attention goes to what stops working if a
hunk is removed. That is functional coupling, and it *feels* like meaning, so I
merge the two changes and then write a name broad enough to cover both. But
coupling only tells me the commits must be **ordered**. It says nothing about
whether they are one thing.

Two changes that must ship together are still two commits when they are two
statements. The tree being broken between them is the expected, allowed cost —
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
many files it touches.** A consequence is not a separate statement — it is what
the statement *means* in code.

Change one line of a schema and eleven programs stop compiling: those eleven
programs are the consequence, they are not eleven decisions, and they go in the
one commit. A commit touching thirty files is fine when the thirty files are one
statement. Size is not the test. "One statement" is the test.

Ask of each hunk: *would this hunk exist if the decision had not been made?* No →
it is a consequence, it belongs with the decision. Yes → it is its own sheep.

### 6. Prose is always its own commit

Documentation, `CLAUDE.md`, and the skills under `.claude/skills/` get their own
commits, even when the words changed *because* of a code decision. They are read
by different people, reviewed differently, and reverted independently.

Split prose further when it addresses different readers — the published docs site
and the repository's own working agreements are two commits, not one.

### 7. Write the message, and put the explanation in it

Everything I wanted to say in a comment goes here instead: why this exists, what
it replaced, what was rejected, what it is a step toward. See the
[no-comments rule](../../CLAUDE.md#dont-add-comments-everywhere) — a comment
explaining the change is a commit message that leaked into the source and will
outlive the reader it was written for.

If I find myself writing a comment while making a change, that is a signal I have
not written the commit message yet. Write it there.

No conventional-commit prefixes. No `feat:`, `fix:`, `chore:`, `docs:`,
`refactor:`. One imperative sentence, in the style of `git log --oneline -20`.

### 8. Present the subject lines and wait

Show the user **the list of subject lines, in order** — not the file groupings,
not a script. The subject lines are what they are approving, because the subject
lines are the statements. Wait for the nod before staging anything.

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

## The two ways I actually get this wrong

**Failure — one glob.** PR #32 shipped as a single commit: 32 files, a schema
split, twelve program conversions, a helper deleted, a rename, the shell's
close relocated, two tests, five doc pages, `CLAUDE.md` and a skill. Every
sentence of it was true and none of it could be reviewed. The user could not say
yes to the rename without also saying yes to the schema. That is not a review,
it is a hostage situation.

**Failure — the overcorrection.** Told to split it, I proposed *one commit per
program*: twelve commits doing the same thing for the same reason. That is
herding by fleece — I saw twelve files and made twelve pens. All twelve programs
changed because one line of `sh.capnp` changed. One decision, one pen.

**Failure — the tumor.** Splitting it on the second attempt, I put the twelve
program conversions in one commit and slipped `sh`'s own change — moving the
close of the client's stream out of `Interpreter::exec` and into `ShPortal.sh` —
into the middle of them. It had to land at the same time or the shell would stop
closing anything, so I called it a consequence. It is not: "who closes the
client's stream" is a different statement from "what a program says in `output`",
and it is revertible on its own. I then named the commit "Have a program say its
outcome instead of implying it", which is abstract enough that the relocation
hides inside it — the vaguer-name cheat, in the very next split I made after
writing this file.

Between those failures is the actual skill, and neither file count nor diff size
nor what-breaks-if-I-split will find it. Only the statement will.

## Mechanics

**Before rewriting any history, snapshot the final tree.** `git add -A && git
commit --no-verify -m "WIP snapshot"` and write down the SHA. Everything after
that is recoverable, and `git diff <snapshot> HEAD` at the end must come back
empty — that is the proof the rewrite lost nothing.

**Splitting an existing commit.** Reset to the base, then rebuild forward: for
each statement, bring in the files it wholly owns with `git checkout <snapshot>
-- <paths>`, and hand-construct the intermediate content for any file that spans
more than one statement. A file appearing in three commits is normal and is not a
sign the split is wrong.

**Never `git add .` or `git add -A`** when building a real commit — only the
snapshot may be taken that way. Stage the paths the statement owns.

**Never put a timeout on a commit.** Pre-commit hooks stash the unstaged changes
before they run; killing the commit mid-hook leaves that stash unrestored and the
user's uncommitted work gone. Give it the maximum timeout, or run it in the
background. If one does get killed, the work is in the patch file named in the
hook's `[INFO] Stashing unstaged files to <path>` line: `git apply <path>`.

**A rebase in progress is a normal working state, not a blocker.** `git rebase -i`
with `edit` is exactly how a commit gets split; the working tree handed to me is
usually the leftover of the commit being split. Do not raise it, do not warn, do
not ask. Commit onto the detached HEAD as usual; `git rebase --continue` is the
user's move.

**A fixup belongs in the commit it fixes.** Discovery order must never show up in
the history — see
[the working agreement](../../CLAUDE.md#a-fixup-belongs-in-the-commit-it-fixes--always).
Force-push the rewritten branch; a pushed branch is not a reason to append.

## Checklist

- ☐ Read the whole diff and list the **decisions**, ignoring file layout
- ☐ Write a one-sentence subject line for each, before staging anything
- ☐ Run the and-test on every subject line; split what fails, and never
      escape it by choosing a vaguer name
- ☐ Name every hunk and ask whether the subject line predicts it
- ☐ Run the tumor test: for anything that merely has to land together,
      order it next to the commit instead of inside it
- ☐ Run the consequence test on every hunk; fold consequences into their decision
- ☐ Pull prose out into its own commits, split by reader
- ☐ Show the user the subject lines in order, and wait
- ☐ Snapshot, then rebuild forward, staging named paths only
- ☐ `git diff <snapshot> HEAD` is empty

## I do not write comments

Not one — not `//`, not `///`, not `#` in a schema. The user writes every comment
in this codebase. When something genuinely needs saying in one, I say it to the
user in my reply and let them decide; my explanations go in the commit message.
See [CLAUDE.md](../../CLAUDE.md#i-do-not-write-comments).
