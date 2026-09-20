---
name: fix-strings
description: Apply the wording the user wrote into review/<branch>/strings.md back into the code, docs and tests, and fold each change into the commit that introduced the string. Use when the user types /dusk-dev:fix-strings, or says they edited strings.md, fixed the strings, or reworded the strings in the review.
---

# /fix-strings

`review/<branch>/strings.md` is a table the string-review agent wrote: **String | Link | Explanation**, one row per user-facing string on the branch. The user edits the String cells. This skill puts what they wrote back where each string lives. Their wording is final: apply it exactly, do not improve it, do not ask whether they meant it, and do not send it to dilemma-triage.

`<branch>` is the checked-out branch with any `worktree-` prefix removed.

## Steps

1. **Read the table.** For each row, open the file at the link's line (the link text is `<file>:<line>`) and find the string literal there. A cell with `\|` means `|`, and `<br>` means a line break.
2. **Find the edited rows.** A row whose cell matches the literal at its line is unchanged; skip it. A row whose cell differs is an edit, and the literal at that line is the old wording. If the line holds more than one string, the old wording is the one the cell was made from, which is the literal it shares the most text with. If no literal on that line is recognisably the cell's original, the file moved under the table: stop and say which row, instead of guessing.
3. **Keep what the code needs.** A format placeholder (`{error:#}`, `{pid}`, `%s`) in the old wording that the new wording drops or renames cannot be applied as written: stop and say which row and which placeholder. Escape what the language needs (quotes, backslashes, braces in a Rust `format!`) - that is spelling the user's text in the source, not changing it.
4. **Change every copy.** `git grep -nF '<old wording>'` for the same text quoted elsewhere - a docs page that quotes the error, a test that asserts it - and change those to the new wording too.
5. **Fold each change into the commit that introduced it.** Before editing, `git blame -L <line>,<line> -- <file>` names the commit. If it is on the branch (not an ancestor of `origin/master`), commit the change with `git commit --fixup <sha>` and finish with `GIT_SEQUENCE_EDITOR=: git rebase -i --autosquash origin/master`. A copy in docs or tests goes into the commit that put that copy there, found the same way. A string that came from master gets its own commit on the branch.
6. **Check it.** `cargo check` for Rust, `python3 -m py_compile` for Python. Tests are run only if the user asked for tests.
7. **Rewrite the table.** Each applied row's String cell stays as the user wrote it, and its link is updated if the line moved. Re-render the page with `python3 .agents/plugins/dusk-dev/hooks/report.py`.
8. **Push and say it.** If the branch has a pull request, force-push with a lease and update the body's `## Strings` from the table.

## Reply

A table of what changed and nothing else before it:

```
| Old | New | Where | Commit |
```

Then any row that could not be applied, with the reason, and the activate skill's closing line for the state the branch is in.
