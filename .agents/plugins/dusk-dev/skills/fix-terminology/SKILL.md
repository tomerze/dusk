---
name: fix-terminology
description: Apply the names the user wrote into review/<branch>/terminology.md - rename each edited identifier or word everywhere it is used - and fold each rename into the commits that introduced the name. Use when the user types /dusk-dev:fix-terminology, or says they edited terminology.md, renamed the terms, or fixed the names in the review.
---

# /fix-terminology

`review/<branch>/terminology.md` is a table the terminology-review agent wrote: **Name | Link | Explanation**, one row per name the branch introduces. The user edits the Name cells. This skill renames each edited name to what they wrote. Their names are final: apply them exactly, do not propose another, do not ask whether they meant it, and do not send them to dilemma-triage.

`<branch>` is the checked-out branch with any `worktree-` prefix removed.

## Steps

1. **Read the table.** For each row, open the file at the link's line (the link text is `<file>:<line>`) and find the name there. A cell with `\|` means `|`.
2. **Find the edited rows.** A row whose cell matches the name at its line is unchanged; skip it. A row whose cell differs is a rename, and the name at that line is the old one. If the line holds no name the cell was recognisably made from, the file moved under the table: stop and say which row, instead of guessing.
3. **Rename it everywhere.** For an identifier, `git grep -nw '<old>'` across the tree and rename every use that refers to the same thing: its definition, callers, imports, re-exports, tests, docs, the skills, and strings or log lines that name it. Keep each language's case convention for its uses (`CTRL_C_PRESSED` for a Rust static, `ctrl_c_pressed` for its function) when the user wrote the name in one case. For a word used in prose or a string, replace that phrase in the files the branch added it to. A use of the same word for a different thing stays.
4. **Fold each rename into the commits that introduced the name.** A rename touches every commit that added a use. For each changed line, `git blame -L <line>,<line> -- <file>` before the edit names the commit; commit the changes to that commit's lines with `git commit --fixup <sha>`, and finish with `GIT_SEQUENCE_EDITOR=: git rebase -i --autosquash origin/master`. A use that came from master gets its own commit on the branch.
5. **Check it.** `cargo check --workspace --all-targets` for Rust, `python3 -m py_compile` for Python, and `git grep -nw '<old>'` returns nothing that refers to the renamed thing. Tests are run only if the user asked for tests.
6. **Rewrite the table.** Each applied row's Name cell stays as the user wrote it, and its link and explanation are updated to the new name. Re-render the page with `python3 .agents/plugins/dusk-dev/hooks/report.py`.
7. **Push and say it.** If the branch has a pull request, force-push with a lease and update the body's `## Terminology` from the table.

## Reply

A table of what changed and nothing else before it:

```
| Old | New | Uses renamed | Commits |
```

Then any row that could not be applied, with the reason, and the activate skill's closing line for the state the branch is in.
