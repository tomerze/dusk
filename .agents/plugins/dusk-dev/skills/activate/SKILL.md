---
name: activate
description: Load what there is to know about working in the dusk repository - the dusk-developer orientation map and working agreements, what the hooks refuse in every session, how the tree is built and measured, and how a node is driven by hand. Use it at the start of any work in this repository.
---

# What there is to know about dusk

Load the `dusk-developer` skill now. It is the orientation map of the codebase
and the working agreements every change is held to. The sections below are about
what surrounds the code: what the hooks refuse, how the tree is built and
measured, how a node is driven by hand.

## Nothing personal goes in the repository

**No name, no email address, no home directory path, no account handle - not in
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
plugin either.

## What the hooks refuse in every session

These are checked as each action is taken:

- No comment line is written by me, in any file.
- Nothing personal is written anywhere: no name, no email address, no home
  directory, no account handle.
- A commit subject has no conventional-commit prefix, does not start with Say,
  and carries no issue number; no commit message and no pull request body
  carries a watermark trailer.
- `git commit` runs in the foreground with `timeout: 600000`; nothing is staged
  with `git add -A` or `git add .`.
- No polling loop over a command I started; no copying of a build directory; no
  bare `git stash`; `cargo test` and `cargo nextest` only after asking.
- A program under `base/` is edited only with `authoring-a-program` loaded; the
  `Driver` trait only with `adding-a-driver-method` loaded.
- `git push` never targets master, and every commit on the branch passes the
  subject checks.
- Merging always asks.

## Building and measuring

- `cargo check` is cheap and always allowed. **Do not run `cargo test` or
  `cargo nextest` unless the user asked** - tests are a separate workstream.
  Work that asks for tests is asking; running them is then part of
  delivering, and shipping a test you never executed is not.
- `cargo build --release --bin dusk` is the real build. It is a plain Rust
  release build; the only unusual cost is compiling the vendored Cap'n Proto
  compiler the first time.
- If the work wants a before/after comparison, **build and measure the
  unmodified tree first and keep the numbers** - once the target directory is
  overwritten, recovering the baseline costs another full build. A second
  worktree at `origin/master` is the cheapest way to get it back.
- Run benchmarks **sequentially on an idle machine**. A benchmark sharing the
  machine with a compile reports numbers that are off by 8×. **Check whether the
  machine is actually idle first** - other worktrees under `.claude/worktrees/`
  may have their own sessions compiling right now (`ps aux | grep rustc` shows
  whose target directory each one is writing to).
- **Never state a number you did not measure**, and never explain a slow build
  before timing it. Measure, then decide.
- **A full disk reports itself as a compiler bug.** When the volume fills, rustc
  and the linker fail with `No space left on device` mixed in among
  `rustc-LLVM ERROR` and `ld terminated with signal 7 [Bus error]`, which read
  like a broken toolchain. Run `df -h` before believing any of it. Every
  worktree carries its own `target`, so measure them with `du -sh` rather than
  assuming a size - they differ by more than an order of magnitude. Reclaiming
  space means deleting the user's build caches, so **ask first**, and name the
  measured sizes and which branches are already merged so the choice is theirs.

## Driving the node by hand

Running the thing beats reasoning about it, and it is not the test suite, so it
is available even when tests are not. Build with
`cargo build --bin dusk_node --bin dusk`, start the node, and drive it with
one-shot CLI commands (`./target/debug/dusk 127.0.0.1:9090 "ps"`).

- **The `dusk_node` binary takes no arguments and, built by default, listens on
  9090**, on every address - its init script is `DUSK_NODE_INIT_SCRIPT`, which
  `.cargo/config.toml` sets to `nightfall -l 9090`, compiled into the artifact
  while it is built, and `artifacts/dusk_node/bin/src/main.rs` passes a null
  pointer and reads no argv. Check `pgrep -af dusk_node` and
  `ss -ltn | grep 9090` before starting one. If another session's node holds
  9090, yours fails to bind and exits, and a CLI pointed at 9090 silently talks
  to *theirs*: build yours on another port instead, with
  `DUSK_NODE_INIT_SCRIPT="nightfall -l 127.0.0.1:9091" cargo build --bin dusk_node`,
  and point the CLI at that. That build replaces `target/debug/dusk_node` until
  the next build without the variable, and `tests/prompt` fails meanwhile: with
  `something already listens on 127.0.0.1:9090` while another node holds 9090,
  otherwise with `nothing came up on 127.0.0.1:9090`, leaving the 9091 node it
  started running. Do not kill a node you did not start.
- **The node writes nothing to its own stdout**, so redirecting it to a file
  gets you an empty file rather than its logs. Read them through the node:
  `./target/debug/dusk <address> "logs dump --replay-only"` returns the buffered
  history and exits. `logs view` is the interactive view and refuses
  `--replay-only`.
- A node built from a different revision answers with the wrong schema, and the
  error names neither the port nor the process: a changed capnp union surfaces as
  `Enum value or union discriminant <n> was not present in the schema`, which
  reads like a schema bug in the branch you just wrote.

---

## Keeping this skill true

This file describes an environment that changes under it. When a step here turns
out to be wrong - a submodule that no longer exists, a build cost that no longer
applies, a gotcha that was fixed - **say so and fix the skill**, in its own
commit, separate from the work that uncovered it. A skill that misdirects the
next agent is a defect exactly like a wrong comment.

## I do not write comments

Not one - not `//`, not `///`, not `#` in a schema. The user writes every comment
in this codebase. When something genuinely needs saying in one, I say it to the
user in my reply and let them decide; my explanations go in the commit message.
See [dusk-developer](../dusk-developer/SKILL.md#i-do-not-write-comments).
