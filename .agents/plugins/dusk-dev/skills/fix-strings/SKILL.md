---
name: fix-strings
description: Apply the wording the user wrote into review/<branch>/strings.md back into the code, docs and tests, and fold each change into the commit that introduced the string. Use when the user types /dusk-dev:fix-strings, or says they edited strings.md, fixed the strings, or reworded the strings in the review.
---

# /fix-strings

`review/<branch>/strings.md` is a table the string-review agent wrote: **String | Link | Explanation**, one row per user-facing string on the branch. The user edits the String cells. This skill puts what they wrote back where each string lives. Their wording is final: apply it exactly, do not improve it, do not ask whether they meant it, and do not send it to dilemma-triage.

`<branch>` is the checked-out branch with any `worktree-` prefix removed.

## Steps

1. **Check the tree is clean, and tag the tip.** `git status --porcelain` empty first - the skill is the only thing that knows which edits are its own, and if the tree is already dirty an unrelated edit gets folded into a fixup by step 6. Then `git tag -f fix-strings-snapshot HEAD`: step 6 rewrites history, and this is what undoes it. Then, for each row, open the file at the link's line (the link text is `<file>:<line>`) and find the string literal there. A cell with `\|` means `|`, and `<br>` means a line break.
2. **Find the edited rows.** A row whose cell matches the literal at its line is unchanged; skip it. A cell that differs only in leading or trailing whitespace is **not** an edit either - the table cannot carry it, `cells()` strips it - so leave that literal alone. A row whose cell differs otherwise is an edit, and the literal at that line is the old wording. If the line holds more than one string, the old wording is the one the cell was made from, which is the literal it shares the most text with; if two are equally close, stop and say which row. If no literal on that line is recognisably the cell's original, the file moved under the table: stop and say which row, instead of guessing.
3. **Name the commit for each edited row, before changing anything.** `git blame -L <line>,<line> -- <file>` while the file is still untouched; a blame taken after the edit answers `Not Committed Yet` and the fixup target is lost. Write the sha down per row. If blame names a commit that is an ancestor of `origin/master`, that string came from master and its change gets its own commit on the branch instead.
4. **Keep what the code needs.** A format placeholder (`{error:#}`, `{pid}`, `%s`) in the old wording that the new wording drops or renames cannot be applied as written: stop and say which row and which placeholder. Escape what the language needs (quotes, backslashes, braces in a Rust `format!`) - that is spelling the user's text in the source, not changing it.
5. **Change every copy.** `git grep -nF '<old wording>'` for the same text quoted elsewhere - a docs page that quotes the error, a test that asserts it - and change those to the new wording too, blaming each of those lines first, the same way.
6. **Fold each change into the commit it belongs to.** Stage the files belonging to one sha, `git commit --fixup <sha>`, repeat per sha, and finish with `GIT_SEQUENCE_EDITOR=: git rebase -i --autosquash origin/master`. Stage whole files, never a subset of one file's lines: `git add -p` is interactive and not available here, so when one file carries changes for two shas, fold the whole file into the earlier of them and say so.
7. **Check it.** `cargo check` for Rust, `python3 -m py_compile` for Python, and `git diff fix-strings-snapshot HEAD` shows only the rows applied and nothing else. Tests are run only if the user asked for tests. If this fails, stop: the tag still points at the tree before anything was touched, `git reset --hard fix-strings-snapshot` undoes it.
8. **Rewrite the table.** Each applied row's String cell stays as the user wrote it, and its link is updated if the line moved. Re-render the page with `python3 .agents/plugins/dusk-dev/hooks/report.py`.
9. **Push, say it, and drop the tag.** If the branch has a pull request, force-push with a lease and update the body's `## Strings` from the table. Once step 7 has passed, `git tag -d fix-strings-snapshot`: a tag left standing is silently moved by the next run of this skill, on this branch or any other in the checkout, so it stops meaning what step 9's own advice says it means. Report the tip's sha in the reply instead, as what a bad application is undone with.

## Reply

A table of what changed and nothing else before it:

```
| Old | New | Where | Commit |
```

Then any row that could not be applied, with the reason, and the activate skill's closing line for the state the branch is in.
