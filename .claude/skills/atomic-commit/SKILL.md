---
name: atomic-commit
description: Atomic Commit helps create clean, atomic commits by analyzing changes, detecting mixed concerns, and ensuring each commit is a complete unit of work. Key words to detect are atomic, atomic commits, clean commits.
---
# Atomic Commit

Standalone skill for non-TDD workflows. Helps create clean, atomic commits by analyzing changes and detecting mixed concerns.

## Atomic Commit Definition
- Does exactly one thing (one feature/fix/refactor)
- Leaves codebase in working state (builds and tests pass)
- Can be reverted independently
- Doesn't mix unrelated concerns

## Workflow

### 1. Analyze Changes
```bash
git status && git diff && git diff --staged
```

**A rebase in progress is a normal working state, not a blocker.** `git rebase -i`
with `edit` is exactly how a commit gets split into atomic ones — the working tree
you are asked to commit is usually the leftover of the commit being split. Do not
raise it, do not warn about it, do not ask whether to proceed. Commit onto the
detached HEAD as usual; `git rebase --continue` is the user's move, not yours.

**Never put a timeout on a commit.** Pre-commit hooks stash the unstaged changes
before they run. Killing the commit mid-hook — a timeout, a Ctrl-C — leaves that
stash unrestored, and the user's uncommitted work is gone from the tree. Give the
commit the maximum timeout available, or run it in the background. If a commit
does get killed, the work is recoverable from the patch file named in the hook's
`[INFO] Stashing unstaged files to <path>` line: `git apply <path>`.

### 2. Detect Mixed Concerns
Look for files mixing:
- Multiple features
- Bug fixes + features
- Refactoring + new functionality
- Multiple unrelated bug fixes
- Code + documentation (unless for same feature)
- Tests for multiple features

### 3. Group Commits
Group files by shared purpose. Present the **commits you plan to make** — each
one's message and the files it covers — and wait for confirmation.

Never hand the user shell commands to run. You stage and you commit; the user
reviews the plan, not a script.

Example:
```
Commit 1: "Add user auth" → auth/login.ts, auth/session.ts, tests/auth/login.test.ts
Commit 2: "Fix password validation" → validators/password.ts, tests/validators/password.test.ts
```

### 4. Create Each Commit
For each group:
1. Stage: `git add <file1> <file2>` (NO `git add .`)
2. Review: `git diff --staged`
3. Test: Run tests, ensure pass
4. Build: Run build if applicable, ensure success
5. Commit: Write the message in the repository's own style
6. Verify: `git log -1 --oneline`

**No conventional-commit prefixes.** `feat:`, `fix:`, `chore:`, `docs:`,
`refactor:` and the rest are banned. A subject line is one imperative sentence
saying what the commit does — "Add the atomic-commit skill", "Bump sntpc to 0.11
and sntpc-net-std to 1.3" — with no type, no scope, and no colon standing in for
a verb. Read `git log --oneline -20` before writing the first message and match
what is already there.

### 5. Final Check
```bash
git log --oneline -n <N>
```

## Issue Integration
- Check user message or branch name for issue number
- IF no issue: ask if user wants to create one
  - IF user does not want to provider, leave it blank
- IF yes: help write description, offer `gh issue create` or `glab issue create`
  - Include in commits: `"Add the thing (#42)"`

## Guidelines
**DO:** One logical change, include related tests, run tests, clear messages, issue numbers
**DON'T:** Mix features/fixes/refactors, commit broken code, vague messages, debug code

## TODO Pattern
☐ Analyze: git status/diff
☐ Identify file purposes
☐ Detect mixed concerns
☐ Group into atomic commits
☐ Confirm with user
☐ For each: stage → review → test → build → commit → verify
☐ Final review

## Activation
Use when user says: "atomic", "clean commits", "break down commits", "split commits"
DO NOT use for TDD workflows (use TDG skill).

## Closing
"Created N atomic commits. Tests pass. Would you like to review history, push, or create PR?"
