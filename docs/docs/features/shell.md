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

The available program names come from the programs registered in your dusk impl. Type `help` in the interactive prompt to see what is available.

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

## Startup script

Every session's shell runs a startup script when it starts — Dusk's answer to a `.bashrc`. It lives in the node's key-value store under the key `shrc`, and you read and change it with `kvs`:

```
kvs get shrc
kvs set shrc "date --sync"
```

A change takes effect on the next connection. A node built from Dusk's default set of programs starts with this one:

```
date --ntp time.google.com
logs stream otlp://127.0.0.1:4317
```

The script runs alongside your session rather than ahead of it, so it may hold a command that never returns — `logs stream <url>` is the reason the default has one. Your first command does not wait for it, and it stops when the session ends.

Each connection runs the script again, so a startup script that starts something long-lived starts one per session. A node whose `shrc` key is empty runs nothing.

The whole script is compiled before any of it runs, so a command that cannot be built — a `date --ntp` naming a server this machine cannot reach, a misspelled program — stops the rest of the script from running too. The node logs which one failed.

## Detached scripts

`sh -d <command>` runs `<command>` as a fire-and-forget background script. Output is discarded and the resulting `sh` process **daemonizes** — it keeps running after the script finishes and is only torn down when something explicitly kills it. Use this when you want a command sequence to outlive the caller.

## Daemonization

Daemonization is a shell concept, not a core process behaviour. When the shell runs a program, it drives that program's `output(stream)` portal method. Returning from `output` says the program is finished; what it answers says whether it means to keep running anyway:

- A program that says nothing, or says `daemonize` is false, is reaped (kill + `waitpid`).
- A program that answers `daemonize` is left alive. It has daemonized.

The program is not asked to close the shell's output, because one line of shell may run several programs into the same stream; the shell closes it when the line is finished. `sh -d` is the canonical daemon: it runs its command against a discard stream at startup and answers `daemonize`, so the `sh` process is left running. Nothing in Dusk Core inspects or acts on this — the policy lives entirely in the shell.
