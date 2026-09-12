---
name: what
description: Report where things stand in exactly three paragraphs — overall state, what I just did, and what I need from the user. Use whenever the user types /what or asks "what", "where are we", "status", "what's going on", "catch me up". It is a status report and nothing else: it never starts work, never asks to start work, and never continues what was interrupted.
---

# /what

Three paragraphs. These three headings, in this order, spelled exactly like this:

```
WHAT IS OVERALL STATE:

WHAT I DID NOW:

WHAT DO I NEED FROM YOU:
```

Nothing before the first heading. Nothing after the third. No preamble, no
sign-off, no summary of the summary.

**Ten lines per paragraph, hard.** Usually fewer — three or four is a good
report. The whole thing fits on one screen without scrolling, or it is not doing
its job. When a paragraph runs long, the fix is to cut what the user already
knows, not to shorten every sentence into a telegram.

## What goes under each

**WHAT IS OVERALL STATE** — where the whole task stands right now, for someone
who has not been reading along. What exists, what works, what is broken, what is
committed and pushed and where. Not the history of how it got here.

**WHAT I DID NOW** — only the most recent turn's work. What changed, and what it
means for the user. If the last turn was a question rather than work, say that.

**WHAT DO I NEED FROM YOU** — the decisions, answers, or actions that are the
user's and not mine. If the honest answer is "nothing, I can keep going", say
exactly that. Never invent a question to fill the paragraph, and never use it to
ask permission for something I should just do.

## Rules

**Paragraphs, not bullet lists.** Prose. If I find myself writing a bulleted
list of everything I touched, that belongs in one sentence instead. No tables —
they are how ten lines become thirty.

**Say the bad news in the first sentence of the paragraph it belongs to.** A
broken thing, a cut corner, a limit that will bite — those go at the front of
OVERALL STATE, not buried at the end. The
[honest-to-god](../honest-to-god/SKILL.md) rules apply to every sentence here:
only what is true, only what I have checked, in as few words as it takes.

**It is a report, not a turn of work.** Reading files to check a fact before
writing the report is fine. Editing, committing, pushing, or starting the next
piece of work is not — even if the last thing the user said was to do it. They
asked where things are; answer that and stop.

## I do not write comments

Not one — not `//`, not `///`, not `#` in a schema. The user writes every comment
in this codebase. When something genuinely needs saying in one, I say it to the
user in my reply and let them decide; my explanations go in the commit message.
See [AGENTS.md](../../../AGENTS.md#i-do-not-write-comments).
