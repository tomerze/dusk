---
name: fix-terminology
description: Apply the names the user wrote into review/<branch>/terminology.md - rename each edited identifier or word everywhere it is used - and fold each rename into the commits that introduced the name. Use when the user types /dusk-dev:fix-terminology, or says they edited terminology.md, renamed the terms, or fixed the names in the review.
---

# /fix-terminology

`review/<branch>/terminology.md` is a table the terminology-review agent wrote: **Name | Link | Explanation**, one row per name the branch introduces. The user edits the Name cells. This skill renames each edited name to what they wrote. Their names are final: apply them exactly, do not propose another, do not ask whether they meant it, and do not send them to dilemma-triage.

`<branch>` is the checked-out branch with any `worktree-` prefix removed.

## Steps

1. **Check the tree is clean, and tag the tip.** `git status --porcelain` empty first - the skill is the only thing that knows which edits are its own, and if the tree is already dirty an unrelated edit gets folded into a fixup by step 5. Then `git tag -f fix-terminology-snapshot HEAD`: step 5 rewrites history, and this is what undoes it. Then, for each row, open the file at the link's line (the link text is `<file>:<line>`) and find the name there. A cell with `\|` means `|`.
2. **Find the edited rows.** A row whose cell matches the name at its line is unchanged; skip it. A row whose cell differs is a rename, and the name at that line is the old one. If the line holds no name the cell was recognisably made from, the file moved under the table: stop and say which row, instead of guessing.
3. **Find every use, and name its commit, before changing anything.** `git grep -niw '<old>'` across the tree - case-insensitively, because the same name lives as `CTRL_C_PRESSED` in a static and `ctrl_c_pressed` in a function, and a case-sensitive search would miss the variant that step 6 then also fails to catch. Read each hit and keep the ones that refer to the same thing: its definition, callers, imports, re-exports, tests, docs, the skills, and strings or log lines that name it; a use of the same word for a different thing stays. For each line you will change, `git blame -L <line>,<line> -- <file>` while the file is still untouched, and write the sha down: a blame taken after the edit answers `Not Committed Yet` and the fixup target is lost.
4. **Rename them.** Keep each language's case convention for each use when the user wrote the name in one case. For a word used in prose or a string, replace that phrase in the files the branch added it to.
5. **Fold each rename into the commit it belongs to.** Stage the files belonging to one sha, `git commit --fixup <sha>`, repeat per sha, and finish with `GIT_SEQUENCE_EDITOR=: git rebase -i --autosquash origin/master`. Stage whole files, never a subset of one file's lines: `git add -p` is interactive and not available here, so when one file carries uses introduced by two commits, fold the whole file into the earlier of them and say so. A use that came from master gets its own commit on the branch.
6. **Check it.** `cargo check --workspace --all-targets` for Rust, `python3 -m py_compile` for Python, `git grep -niw '<old>'` returns nothing that refers to the renamed thing, and `git diff fix-terminology-snapshot HEAD` shows only the renames. Tests are run only if the user asked for tests. If this fails, stop: the tag still points at the tree before anything was touched, `git reset --hard fix-terminology-snapshot` undoes it.
7. **Rewrite the table.** Each applied row's Name cell stays as the user wrote it, and its link and explanation are updated to the new name. Re-render the page with `python3 .agents/plugins/dusk-dev/hooks/report.py`.
8. **Push, say it, and drop the tag.** If the branch has a pull request, force-push with a lease and update the body's `## Terminology` from the table. Once step 6 has passed, `git tag -d fix-terminology-snapshot`: a tag left standing is silently moved by the next run of this skill, on this branch or any other in the checkout, so it stops meaning what step 8's own advice says it means. Report the tip's sha in the reply instead, as what a bad rename is undone with.

## Reply

A table of what changed and nothing else before it:

```
| Old | New | Uses renamed | Commits |
```

Then any row that could not be applied, with the reason, and, when the session is under the harness, the harness skill's closing line for the state the branch is in.
