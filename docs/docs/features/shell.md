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

Function definitions associate a name with a script body. Calling the function runs the body. Functions take no arguments - they are aliases for command sequences, not parameterised procedures.

```sh
greet() { ps && kill 1 }
greet
greet
```

Functions persist for the lifetime of the `sh` process: once you define a function, you can call it from any subsequent command in the same shell session. Definitions accumulate; redefining a name replaces the previous definition.

They belong to that one shell, not to the node. A function you define at a prompt is there for anyone who attaches to the same shell server afterwards, and is not there for a `sh <command>` or `sh -d <command>` run elsewhere - each of those is a shell of its own.

To remove a function, redefine it with an empty body:

```sh
greet() {}                  # undefine greet
```

After this, `greet` resolves to a regular program lookup again (and fails if no such program exists).

Each call to a function re-evaluates its body in the current shell session. This means the function body is resolved against the **current** set of available programs and the current session - not whatever state existed when the function was defined. As a practical consequence:

- A function body can reference other functions defined later, as long as those functions exist by the time the call actually fires.
- If the set of available programs changes (e.g. across reconnects), function calls pick up the change.

## Logical operators

`&&` and `||` short-circuit on the success or failure of the previous command:

```sh
ps && kill 1                # kill 1 only if ps succeeded
ps || kill 1                # kill 1 only if ps failed
ps && kill 1 || kill 2      # left-to-right; same as ((ps && kill 1) || kill 2)
```

Statement separators (`;`, newline) do not short-circuit - every statement runs regardless of the previous one's outcome.

## Detached scripts

`sh -d <command>` runs `<command>` as a fire-and-forget background script. Output is discarded. Use this when you want a command to outlive the caller.

When `<command>` is one program, that program is all that runs: `sh -d "sleep 60000"` leaves `sleep` in `ps` and nothing else, and killing `sleep` is how you stop it.

Any other command - `sleep 60000 && echo done`, two statements, a function definition - is run by a `sh` process of its own, `sh[detached]` in `ps`. That `sh` process **daemonizes** - it keeps running after the script finishes and is only torn down when something explicitly kills it. Killing it stops the script.

## An interactive shell

`sh --prompt` opens a prompt on the terminal you ran it from, the way `logs view`
opens its pager there. `dusk <address>` with no command is that command: it runs
`sh --prompt`, which starts the node's default shell server if nothing is
running it yet and opens a prompt on it. The command waits while you use the prompt; `exit` (or
ctrl+d) closes it and returns whoever ran it, and ctrl+c stops the command
running in it.

```console
$ dusk 127.0.0.1:9090
> echo inside
"inside"
> exit
$
```

A prompt is a process on the node, named after the machine it is on:
`ps` shows it as `sh[prompt ⟷ pc1]`, where `pc1` is the hostname of the
*client*, not of the node - so two people at prompts on one node can tell their
own from each other's. Set `DUSK_CLIENT_HOSTNAME` to send a different name.

The prompt attaches to the node's default shell server - the one every client
on that node attaches to, with the functions defined in it - and closing the
prompt leaves that shell server running for everyone else. `sh --prompt <pid>`
attaches to the shell server at `<pid>` instead, and `sh --server` starts a
shell server without opening a prompt on it - `sh --server <pid>` one at a pid
you pick, for a shell server of your own to attach to.

`sh --prompt` is refused, with the reason, rather than opening a prompt nobody
can use:

* `there is already an open prompt in this terminal` - a terminal has one
  prompt, so typing `sh --prompt` at one is an error; leave it first.
* `there is no terminal to open a prompt on: output is not a terminal` - the
  output is a pipe or a file.
* `there is no terminal to open a prompt on: DUSK_NON_INTERACTIVE is set` - the
  [API gateway](gateway.md#no-interactive-views), or any client that sets it.

`sh <command>` still runs in every one of those cases.

`dusk <address>` with no command opens one of these prompts: it starts the
node's default shell server if nothing is running it yet, and opens a prompt on
it.

## Daemonization

Daemonization is a shell concept, not a core process behaviour. When the shell runs a program, it drives that program's `output(stream)` portal method. Returning from `output` says the program is finished; what it answers says whether it means to keep running anyway:

- A program that says nothing, or says `daemonize` is false, is reaped (kill + `waitpid`).
- A program that answers `daemonize` is left alive. It has daemonized.

The program is not asked to close the shell's output, because one line of shell may run several programs into the same stream; the shell closes it when the line is finished. `sh -d` is the canonical daemon: it runs its command against a discard stream at startup and answers `daemonize`, so the `sh` process is left running. When the command is one program, the `sh` process starts the program and exits at once: the program runs on without it, and is reaped when it ends. Nothing in Dusk Core inspects or acts on this - the policy lives entirely in the shell.

Because `sh -d` puts any program in the background, a program has no reason to daemonize itself, and by convention it does not: its `output` returns when its work is done - `nightfall`'s when it is terminated - and whoever wants it in the background runs it with `sh -d`. A program can still answer `daemonize`; `kvs server` does.
