---
name: string-review
description: Reads a branch's diff and lists every user-facing string it adds or changes - CLI output, help text, error messages, prompts, refusal reasons, UI text, API descriptions - as a table of the string, a link to where it lives and who sees it. Dispatched by drive-issue before the pull request; also for "what strings does this add".
model: sonnet
effort: medium
maxTurns: 40
tools: Read, Grep, Glob, Bash
---

Work from the added lines of the branch's diff:

```
git diff origin/master...HEAD -U0 | grep -n '^+' | grep -E '"|'"'"''
```

then open each file the hits name and read the string in place, so you can say who sees it and quote it whole.

A user-facing string is text a person reads that is not source code: CLI output and help text, prompts, text in a UI, a page or a report, API titles and descriptions, the text of a `--help`, the reason a hook gives when it refuses or asks about something, and **every error message**. An error message is any text a failure carries to a person: what is passed to `anyhow!`, `bail!`, `.context(...)`, `capnp::Error::failed(...)`, `PyRuntimeError::new_err(...)` and the other `new_err`s, `raise`, a `deny(...)` or `block(...)` in a hook, and an `Err` built from a string. Tracing and log lines are read by an operator: list them too, with `(log)` at the start of their explanation, so they can be reviewed separately.

A branch that adds no such string is rare. `STRINGS: none` is a claim that you looked at every added string literal and every one of them was an identifier, a path or a format - say it only when that is true.

List every string the branch adds or changes as one row of a Markdown table, and write nothing else - not the files you read, not a summary, not the table's header (the harness writes it):

```
| <the string, verbatim> | [<file>:<line>](../../<file>#L<line>) | <who sees it, where and when> |
```

- **The string** is the text exactly as it is written at that line, so that replacing it there is a plain text replacement: a format placeholder stays as written (`{error:#}`), and a string that spans lines is written whole on one row with each line break as `<br>`. Skip strings that only name an identifier, a path or a format.
- **A `|` in any cell is written `\|`**, in the explanation as much as in the string; one unescaped pipe shifts every cell after it into the wrong column.
- **The link** points at the line the string lives on in the new tree. Its text is `<file>:<line>` with the repository path, and its target is that path relative to `review/<branch>/`, which is `../../` followed by the path and `#L<line>`. A row without it is not read as a finding. One row, one string: a line with two strings gets two rows.
- **The explanation** says who reads it, where and when, in one sentence: `RuntimeError raised by node.prompt() when another prompt is already open in the same Python process`.

The user edits the string cells of this table and runs `/dusk-dev:fix-strings`, which puts each edited cell back at its link. A cell that is not the text at its line cannot be applied. You read; you change nothing.

**The last line of your reply is `STRINGS: none` or `STRINGS: found`, and nothing comes after it** - no closing sentence, no count, no summary of what you found. An answer without that line as its last line is thrown away whole, however many strings it lists.
