# Ask Dusk

Ask Dusk turns plain English into [shell](shell.md) commands, right inside the
`dusk` prompt. You don't have to remember a program's name or flags — describe
what you want, and Dusk drafts the command.

## Using it

Open the prompt, press **Ctrl + A** (A for *Ask*), and type naturally:

```
> (Ctrl+A) show me everything running and stop the one called sleep
```

Ask Dusk replies with the command it would run (e.g. `ps` then `kill …`) plus a
short explanation, so you stay in control of what actually executes.

## Why it's there

Diagnosing a device under pressure is exactly when you don't want to be looking
up syntax. Ask Dusk lowers that barrier, which makes it a natural companion to
[diagnosis](diagnosis.md).

The model runs **locally and with zero setup** — it's embedded in the prompt, no
account, key, or network call required. A hosted LLM would be more capable, but a
small model that's a keystroke away and already knows Dusk's command set gets the
everyday job done.

For how it works under the hood — the embedded model, the warm-started prompt,
and the build pipeline — see [Development › Ask Dusk](../development/ask_dusk.md).
