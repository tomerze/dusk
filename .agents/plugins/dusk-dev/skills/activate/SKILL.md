---
name: activate
description: Put the dusk working rules in force on whatever branch is checked out — every decision through the dilemma-triage agent as it is made, one commit while working and the atomic-commit agent's split just before the push, a fix reported in three lines, the review agents alongside the human, the hand-off that ends "Ready for review.", and merging only on explicit approval. Use it at the start of any work in this repository that is not an issue drive: a branch the user is already on, a fix they described, a change they asked for by hand. drive-issue invokes it and adds the issue mechanics on top.
---

# Activate the dusk rules

The working sequence for any change in this repository, on the branch that is
checked out right now. Nothing here is about how to write the code — `AGENTS.md`
and the working agreements own that. This is how decisions are made, how commits
are made, how the branch is handed over, and what the harness checks while that
happens.

It takes no arguments and creates nothing: no worktree, no branch, no issue.
`drive-issue` is this skill plus the issue.

**The user's approval is the only gate you may not skip.** Merging without it is
the one unrecoverable step.

## Every turn is marked

The user reviews what I hand them, and the agents review it beside them; the
merge waits for both. That only works if every turn says which state the work is
in, so from the first commit onward a turn ends one of exactly three ways:

- **A hand-off** ends with **`Ready for review.`** on its own line, with nothing
  after it. It means the work is pushed, the pull request carries the tip, and
  there is something to read.
- **A dilemma** opens with a **`DILEMMA:`** line, naming what is being decided,
  and asks the one question that blocks the work, with a question mark. The
  `dilemma-screen` agent reads it before the turn is allowed to end: a question
  I could have decided and recorded comes straight back, because stopping the
  session costs the user hours and the answer costs them one line in review.
  What survives the screen is a question where proceeding on a guess would be
  unsafe or would throw the work away. A decision `dilemma-triage` already
  marked `ask-human` is never screened.
- **An answer** replies to something the user asked, and ends with the line that
  says nothing else moved: **`Still ready for review.`** if a hand-off is
  standing, **`Still waiting for dilemma verdict.`** if a question is. The
  answer comes first and the line is last, so the user reads what they asked for
  and then where that leaves things.
- **A wait** opens with **`WAITING ON SUBAGENTS:`** and names the agents whose
  answers the work needs. It is only for that: the harness refuses it unless an
  agent this session dispatched is still running, and when they report the work
  carries on rather than waiting again.

Nothing else ends a turn, and the harness checks it on every turn once the
session has committed or pushed. A status report that is none of the four leaves
the user with nothing to do and no idea whether it is their move.

The status line carries the same state, so it is visible without reading back:
*working*, *waiting on <agents>*, *handed over, awaiting review*, or *awaiting a
verdict*.

**An answer does not change what is standing**, which is why it restates it
rather than claiming it again. A hand-off stays standing until the work moves
past it; a question stays standing until the user answers it. Getting the line
wrong is worse than leaving it out: it tells the user the ball is in the other
court.

## Nothing personal goes in the repository

**No name, no email address, no home directory path, no account handle — not in
code, not in a comment, not in a commit message, not in a manifest, not in a
skill, not in a pull request.** The repository is public and permanent; a person
who has to be named there cannot take it back. This holds for the user, for
anyone they work with, and for me.

Where one is unavoidable, it is derived at run time and never written down: the
repository's owner comes from `git remote get-url origin`, the author of a
commit comes from git's own configuration, and a path comes from `$HOME` or `~`.
If something genuinely needs a person's name in it, I say so in the reply and let
the user write it.

The pre-edit hook enforces this against the git user's own name and address and
their home directory, read fresh each time, so nothing personal is stored in the
harness either.

## What is in force

The hooks check the working agreements as each action is taken:

- No comment line is written by me, in any file.
- Nothing personal is written anywhere: no name, no email address, no home
  directory, no account handle.
- A commit subject has no conventional-commit prefix, does not start with Say,
  and carries no issue number; no message carries a watermark trailer.
- `git commit` runs in the foreground with `timeout: 600000`; nothing is staged
  with `git add -A` or `git add .`.
- No polling loop over a command I started; no copying of a build directory; no
  bare `git stash`; `cargo test` and `cargo nextest` only after asking.
- A program under `base/` is edited only with `authoring-a-program` loaded; the
  `Driver` trait only with `adding-a-driver-method` loaded.
- `git push` never targets master, and every commit on the branch passes the
  subject checks. Pushing does not wait for the split.
- `Ready for review.` waits for the split: the `atomic-commit` agent has to have
  signed off the tip being handed over.
- One pull request per branch, carrying the decisions under `## Decisions`;
  merging always asks.

Work goes through three phases. **Writing:** the rules above, and
`dilemma-triage` on every decision. **Getting ready to review:** the
`atomic-commit` agent splits the branch, which is what gates the hand-off —
pushing is free, and pushing often is how the work stays recoverable. **Ready
for review:** the hand-off goes out and the review agents run alongside the
human, never ahead of them.

## Building and measuring

- `cargo check` is cheap and always allowed. **Do not run `cargo test` or
  `cargo nextest` unless the user asked** — tests are a separate workstream.
  Work that asks for tests is asking; running them is then part of
  delivering, and shipping a test you never executed is not.
- `cargo build --release --bin dusk` is the real build. It is a plain Rust
  release build; the only unusual cost is compiling the vendored Cap'n Proto
  compiler the first time.
- If the work wants a before/after comparison, **build and measure the
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

## Driving the node by hand

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

## Reporting a fix

Every bug I fix is reported in the same three lines — in the reply, and in the
pull request body. Not a paragraph, not the story of how I found it:

```
THE PROBLEM WAS:
I FIXED IT BY:
MY FIX IS NOT GOOD ENOUGH BECAUSE:
```

**The first line is the mechanism, not the symptom.** The user saw the symptom;
they are the one who reported it, and repeating it back tells them nothing. What
they cannot see is which line of which function did the wrong thing, and why
that produced what they got. Name it: the arm that answered before the work
started, the call that returned early, the field never set, the capability that
died with its connection, the assumption that stopped holding and where. **A
first line I could have written without reading the code is not a report, it is
an echo.**

Wrong — the symptom, which they already have:

> `sh` with no command printed "running in server mode" instead of opening a
> shell.

Right — the mechanism, which they do not:

> `Portal::output`'s server-mode arm wrote one value and answered
> `daemonize = false` immediately, so the interpreter killed and reaped the
> process as soon as it had started it. Nothing stayed running for a client to
> attach a view to, and `output` returning is the only thing the caller waits
> on.

The second line is the change, in one sentence, in the same terms.

**The third line is the one that matters, and the one I will want to skip.** A
fix that covers the path in the report and not its siblings, that leaves the
same class of bug reachable another way, that I could not drive and only read —
it says so there. `null` is allowed and it is a claim: it means I went looking
for the case that breaks this fix and did not find one. Writing `null` because
nothing came to mind is a lie in three characters.

## A skill that misled me is fixed in this session

When this skill, `drive-issue`, `authoring-a-program`, `atomic-commit` or
`AGENTS.md` turns out
to describe something that is not true — a step that breaks, a number that is
wrong, a mechanism that has moved — **I fix it now, in the session that found
it.** I do not ask whether to. I do not offer to do it later. I do not write
"worth fixing when you next touch the skill" and move on: that sentence costs
the user a decision and leaves the next reader following an instruction I
already know is false.

It lands as its own commit, at the end with the other text commits, and it rides
along on whatever branch I am on. If that branch is unrelated to it, the commit
still rides — I say so in the reply and in the pull request body, once, and the
user drops it if they would rather have it separately. Deferring the fix is not
one of the choices.

Past failure: I reused a parked `target/` at a different worktree path, watched
the capnp build script fail in a way that named neither, worked out why, fixed
the build — and then told the user I would correct the skill "when you next want
the skill touched". The instruction that had just cost me a build stayed wrong,
and they had to come back and tell me to fix it.

## Every decision goes through triage, as it is made

A decision is anything the user could have decided differently: a shape, a
name that is more than a placeholder, a dependency, an omission, a mechanism
chosen over another. Each one is put to the `dusk-dev:dilemma-triage`
agent the moment it is made, with what is being decided, the alternatives and
what reversing it would cost. Its answer says `decide-alone` or `ask-human`, and
the hook that reads it appends the decision to `review/<branch>/decisions.md`.

`ask-human` means stop and ask, with the question mark, before building on the
decision; the answer is recorded under the decision as `- answer: …`. The
harness does not let a turn end while an `ask-human` decision has neither been
asked nor answered.

## One commit while you work, split it just before review

**Everything goes into a single commit until the work is finished.** Amend it as
you go. Do not split it, do not absorb anything into it, do not reach for
`--fixup` and an autosquash rebase while there is still work coming: each of
those rewrites history that the next change rewrites again, and that churn costs
far more than the whole split costs once.

The split happens **once**, at the end, when the tree is final and just before
the push: dispatch the `atomic-commit` agent (`Agent`, `subagent_type`
`dusk-dev:atomic-commit`) over that one commit. It tags the tree, cuts it
into the commits the work actually is, and the harness lets it finish only when
`git diff harness-snapshot HEAD` is empty and every subject passes; the push is
gated on that sign-off.

Review works the same way. A round of comments that changes anything substantial
is not a handful of fixups aimed at five different commits — squash the affected
commits, or the whole branch, back into one, make the changes there, and split
again. Threading a change into commits you are about to rewrite anyway is the
work done twice.

**Do not put the issue number in the commit subject.** A subject is one
imperative sentence about what the commit does; `(#N)` is metadata about why the
work was scheduled, not part of that sentence, and it is already carried by the
`Closes #N` a drive puts in the PR body. Trailing issue numbers in `git log --oneline` also
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

## Getting ready to review

**One thing stands between the work and the human: the split.** A person cannot
read a branch that is one WIP blob, so the `dusk-dev:atomic-commit` agent cuts it
into the commits the work is, and the harness refuses the hand-off until it has
signed off the tip. Pushing is not held up by it: push whenever, as often as you
like, and split before you ask anyone to read it.

**`dusk-dev:comment-review` has already run** on every commit whose diff added a
comment line, dispatched by the harness the moment the commit landed, and
anything it found was moved into the commit message then, not now. A commit
that added none was never sent to it, and `comments.md` says so rather than
saying the reviewer found nothing.

## Push and open the PR over the MCP

**Say what you are about to open before you open it.** A pull request appears in
the user's repository under their name; they should read that it is coming in
your message, not discover it as a link in the same breath that announces it is
already there.

**A branch produces exactly one pull request.** Whatever else the work turns
up — a stale skill, a broken config, a fix to something adjacent — becomes
another commit on the same branch, never a second PR. Handing over a branch is
handing over one thing to review and one thing to merge; two PRs make the user
do the bookkeeping this skill exists to do for them.

So `create_pull_request` is called at most once per session. After the PR is
open, more work means: commit onto the same branch, push, and
`update_pull_request` the body to cover it. If a second PR has already been
opened, fold its commits onto the one branch and close it as superseded.

**Re-check `origin/master` before you push, and re-run the build after you
rebase.** Dusk work takes hours and the user merges their own PRs in the
meantime. Master does not only change code — it can delete the build
infrastructure the work was set up around, so treat a rebase as invalidating the
whole environment, not just the numbers.

```
git push -u origin <branch>
mcp__github__create_pull_request(owner=…, repo=…, head=…, base="master", title=…, body=…)
```

`gh` 2.46.0 is installed on this machine and the MCP works too; either opens
the PR, and the harness checks the body the same way through both. If the MCP
returns `403 Resource not accessible by personal access token`, the token lacks
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

- **What the branch does**, one section per thing a reviewer has to see
  working. Measured numbers in a table, naming the host, the method, and the run
  count.
- **Every incompatibility, workaround and deliberate omission.**
- **Every decision the reviewer might have made differently** — with the
  alternative and what switching would cost. Dusk's working agreements say design
  decisions are the user's; the PR body is where you hand back the ones you had
  to make to keep moving.
- Anything **broken but out of scope** that you tripped over.
- **The decisions**, under `## Decisions`. The hook adds them from
  `review/<branch>/decisions.md` when the body lacks them, and refuses a body
  while that file is missing. `decision-ranker` runs after the PR is open and
  the ranking replaces the list then.

**The review folder goes in the body, not on the branch.** It is ignored by git
and never committed, so the pull request is the only place a reviewer can read
it. Each file lands under its own heading — `## Decisions`, `## Terminology`,
`## Strings`, `## Comments` — added by `update_pull_request` as each agent
finishes. A heading whose agent found nothing says so in one line; it is not
left out, because a missing heading reads as an agent that never ran.

## Give the branch back, ask for review, then stop

**If the work happened in a worktree, remove it before you ask** — `drive-issue`
says how. Git lets one worktree hold a branch at a time, so while yours holds it
the user cannot check it out to review it.

**Check the pull request is still open, and that it has what you pushed**, before
handing it over:

```
mcp__github__pull_request_read(method="get", owner=…, repo=…, pullNumber=N)
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
paste — `git checkout <branch>` — and ask them to review. The user reads the diff
and runs the node from their own checkout, and they should not have to open the
PR to learn the branch name. Then wait.

**End that message with `Ready for review.` on its own line, and write nothing
after it.** It is the one sentence the user is looking for, and it means every
check above has actually passed — not that the work is nearly there, not that it
is pushed and something is still running. Nothing follows it: no summary, no
caveat, no offer, no question. The line is the end of the message.

**If something is outstanding, that is not a third way to end a turn — it is
work I have not finished.** Either it is mine to finish, and I keep going, or it
is the user's to answer, and the turn is a `DILEMMA:` question. A message that
reports the state of play and stops is the thing this skill exists to prevent:
it reads as a hand-off, it is not one, and the user has no way to tell which.

The harness checks the line: it must be last, the worktree gone, the pull
request open at the pushed tip with the decisions in its body. A message that
claims it while any of that is false is sent back with the check that failed.

**The hand-off goes out once per branch.** From the moment it does, the human is
reading; nothing I do afterwards sends the branch to review again. A review
finding, my own or an agent's, is fixed, committed, pushed and said on the pull
request — and that turn ends `Still ready for review.`, because the hand-off it
restates is the one that is already standing. There is no second
`Ready for review.`, no second split to wait on, no second round of review
agents. The harness refuses the line on a branch that has had it.

This is the loop it exists to stop: the agents report, I fix, the fix changes
the tip, every sign-off is stale again, the agents run again — and the human,
who was ready to read an hour ago, is still waiting for a branch that will not
sit still.

**The hand-off is not the end of the session. It is the point where two reviews
start at once** — see the next section.

Do not merge, do not tidy the branch, and do not read silence or a question as
approval. When comments arrive, verify a claim before implementing it and say so
if you think it is mistaken.

## The agents review while the human does

The hand-off went out, and the human is reading the pull
request now. **The review agents run from here, alongside them.** They gate
nothing and they never come first: a branch whose agents have not run is still a
branch a person can read, and holding the hand-off back to finish a machine's
opinion of it wastes the only reviewer whose approval is the gate.

Dispatch them together and let them land as they finish:

- **`dusk-dev:self-review`** reads every commit against the working agreements
  and the work's definition of done, ending `VERDICT: pass` or
  `VERDICT: findings`.
- **`dusk-dev:race-screen`** reads the diff for anything that can interleave on
  the cooperative executor and dispatches `dusk-dev:race-inspector` when it finds
  some.
- **`dusk-dev:terminology-review`** writes `review/<branch>/terminology.md`;
  copy it into the body under `## Terminology`.
- **`dusk-dev:string-review`** writes `review/<branch>/strings.md`; copy it into
  the body under `## Strings`.
- **`dusk-dev:decision-ranker`** heads `review/<branch>/decisions.md` with the
  ranking; update the body's `## Decisions` to the ranked list.

Each sign-off names the commit it was given, so anything that changes the branch
means running that agent again. As each lands, say on the pull request what it
found — the human is reading in parallel, so a finding is worth more the sooner
it is there. A real finding — a race, a verdict of
`findings` — is fixed the usual way: squash, fix, re-split, force-push, and say
so on the PR.

The review folder — `decisions.md`, `comments.md`, `terminology.md`,
`strings.md` and the `report.html` the hooks render from them — is local and
ignored by git. **It is never committed; it goes in the pull request body**, one
heading per file, updated as each agent lands. A reviewer reads the pull
request, not my working directory, and a folder on the branch is four files they
have to go and find. Give the user the path to `report.html` as well: it opens in
a browser, and every `file:line` in it is a link that opens that line in VS
Code.

The harness will not let the session end while a review agent has not signed off
the pushed tip. That is a nag aimed at me, never at the human: it fires after the
hand-off has already gone out.

## Merge on explicit approval

```
mcp__github__merge_pull_request(owner=…, repo=…, pullNumber=…, merge_method=…)
```

Ask which merge method if it is not obvious from the repository's history.
Afterwards report the merge commit.

---

## Keeping this skill true

This file describes an environment that changes under it. When a step here turns
out to be wrong — a submodule that no longer exists, a build cost that no longer
applies, a gotcha that was fixed — **say so and fix the skill**, in its own
commit, separate from the work that uncovered it. A skill that misdirects the
next agent is a defect exactly like a wrong comment.

---

## Checklist

☐ Every decision put to `dilemma-triage` as it was made; `ask-human` ones asked and answered
☐ `cargo build --release --bin dusk` green; no tests run unless asked
☐ Worked in one commit; split once by the `atomic-commit` agent just before pushing
☐ Committed in the foreground with a long timeout
☐ No issue number in any commit subject
☐ `origin/master` re-checked before pushing; rebased, then the build re-run
☐ Pushed; **one** PR opened over the MCP
☐ PR body written in the first person, addressed to no one, carrying `## Decisions`
☐ The review folder copied into the PR body under `## Decisions`, `## Terminology`, `## Strings`, `## Comments` — never committed
☐ Anything found later: another commit on the same branch, body updated — never a second PR
☐ PR confirmed still open and carrying the pushed tip, not already merged
☐ Branch named in the hand-back, as `git checkout <branch>`
☐ Nothing personal written anywhere: no name, no email address, no home directory, no handle
☐ Every turn since the first commit ended as a hand-off, a `DILEMMA:` question, a `WAITING ON SUBAGENTS:` wait, or an answer restating what still stands
☐ Review requested, the message ending `Ready for review.` and nothing after it
☐ Hand-off sent **before** the review agents ran, not after
☐ `self-review`, `race-screen`, `terminology-review`, `string-review` and `decision-ranker` run after it, each landing on the PR as it finishes
☐ The user has the `report.html` path
☐ **Waited**
☐ Merged only after explicit approval

## I do not write comments

Not one — not `//`, not `///`, not `#` in a schema. The user writes every comment
in this codebase. When something genuinely needs saying in one, I say it to the
user in my reply and let them decide; my explanations go in the commit message.
See [AGENTS.md](../../../AGENTS.md#i-do-not-write-comments).

