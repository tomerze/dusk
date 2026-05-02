# Shell

Dusk ships with a shell program, `sh`, that you use to run commands against a dusk server. It is the program behind the interactive `dusk` CLI.

## Syntax

A script is a sequence of statements separated by `;` or newlines.

A statement is either a command or a function definition:

```sh
ps                          # run a command
ps && kill 1                # run the second command only if the first succeeded
ps || kill 1                # run the second command only if the first failed
foo() { ps; kill 1 }        # define a function
foo                         # call a function
```

Commands are bare token sequences: the first word is the program name, the rest are its arguments. Single-quoted (`'…'`) and double-quoted (`"…"`) strings are supported as arguments.

The available command names come from the programs registered in your dusk impl. Type `help` in the interactive prompt to see what is available.

## Functions

Function definitions associate a name with a script body. Calling the function runs the body. Functions take no arguments — they are aliases for command sequences, not parameterised procedures.

```sh
greet() { ps && kill 1 }
greet
greet
```

Functions persist for the lifetime of the `sh` process: once you define a function, you can call it from any subsequent command in the same shell session. Definitions accumulate; redefining a name replaces the previous definition.

To remove a function, redefine it with an empty body:

```sh
greet() {}                  # undefine greet
```

After this, `greet` resolves to a regular program lookup again (and fails if no such program exists).

Each call to a function re-evaluates its body in the current shell session. This means the function body is resolved against the **current** set of available programs and the current session — not whatever state existed when the function was defined. As a practical consequence:

- A function body can reference other functions defined later, as long as those functions exist by the time the call actually fires.
- If the set of available programs changes (e.g. across reconnects), function calls pick up the change.

## Logical operators

`&&` and `||` short-circuit on the success or failure of the previous command:

```sh
ps && kill 1                # kill 1 only if ps succeeded
ps || kill 1                # kill 1 only if ps failed
ps && kill 1 || kill 2      # left-to-right; same as ((ps && kill 1) || kill 2)
```

Statement separators (`;`, newline) do not short-circuit — every statement runs regardless of the previous one's outcome.

## Detached scripts

`sh -d <command>` runs `<command>` as a fire-and-forget background script. Output is discarded and the resulting `sh` process **daemonizes** — it keeps running after the script finishes and is only torn down when something explicitly kills it. Use this when you want a command sequence to outlive the caller.

See [Daemonization](programs.md#daemonization) for how a process opts into this behaviour.
