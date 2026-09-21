# The Shell

The dusk shell is small in surface - a handful of operators, no variables, no
pipes - but the machinery behind a single line is spread across two processes
and five representations. This page traces a line of shell from the keystroke to
the spawned process: what it turns into at each stage, where the compiler runs,
when the client connection is load-bearing and when it isn't, and how errors
travel back.

## The crates

| Crate | Side | Role |
|-------|------|------|
| `base/sh` | both | the `sh` program. `client/` is client-side; `interpreter/` is server-side; `capnp/sh.capnp` is the wire contract |
| `base/sh/bytecode` | both | `dusk_program_sh_bytecode` - `capnp/bytecode.capnp` always; the grammar, the AST and the `Parser` that lowers a script into a `Script` behind its `parser` feature, so a node compiles only the schema |
| `base/sh/src/client/prompt/` | client (`std`) | reedline UI, builtins, draws output |
| `base/sh/src/client/shell/` | client (`std`) | `Shell` - drives the `sh` process a client was handed |
| `dusk_connection` | client (`std`) | `Connection` - the TCP/RPC link |

## Representations at a glance

```
raw text            "ps && date  # comment"           reedline buffer (client/prompt)
   │ strip_comments
stripped text       "ps && date  "                    quote-aware comment removal
   │ nom (bytecode/src/tokenize.rs)
nom AST             Ast{ [Expr(And(Command "ps",       transient, client-side only
                                   Command "date"))] }
   │ Parser::parse  ─────────────────────────────────  CLIENT / SERVER BOUNDARY
capnp Script        Script{ statements:[…] }           the wire format (script.capnp)
   │ compiler::compile  (server-side)
Frame (bytecode)    0000: program_args                 Vec<Inst>
                    0001: jump_if_error 0003
                    0002: program_args
   │ Interpreter::exec_inner
execution           Dusk.process + Dusk.run per Inst   spawns real processes
```

The first three rows live entirely in the client. The `Script` is the only thing
that crosses the network. Compilation and execution happen server-side, inside
the `sh` process.

## Two entry paths

There are two distinct ways a script reaches the interpreter, and they parse at
different moments.

**Interactive prompt.** `dusk <address>` with no command runs one command:
`sh --prompt`. It builds the args through the `sh` entry, exactly as a script
would, and calls `Dusk.process` then `process.run()`. The node calls the args'
`created` callback with the prompt process, and everything else is the
callback's: `Created` (`base/sh/src/client/mod.rs`) spawns a task that builds a
`Shell` around the node's default shell server, at `sh.capnp`'s `defaultPid` -
starting one there if nothing is running it, since a client that connects finds
the default shell server rather than making another - opens the prompt on this terminal, and
**returns at once**. It has to: the prompt
waits for the process's portal, the portal waits for the process to be run, and
the run cannot be sent until `Dusk.process` - which is waiting for this
callback - has returned. Under `DUSK_NON_INTERACTIVE` the callback opens
nothing and fails the call instead, because there is no terminal to open on.

The CLI runs the prompt process with **`process.run()`**, not `Dusk.run`, so the
call returns when that process exits - which is what `exit` at the prompt does
to it. The CLI then reaps it and leaves; the shell server it was attached
to is untouched, and the next client attaches to the same one. A connection that drops
takes the prompt process with it, so the CLI creates and runs another - without
the callback this time, since the prompt it opened is still there - while the
`Shell`'s own `auto_reconnect` brings the shell server back at `defaultPid` and keeps
the open prompt working across the break.

Each accepted line goes `Prompt::execute_command` → `Shell::sh`, which **parses
the text on the client** into a `Script` and ships it via
`ShPortal.sh(script, output, stop, compiler)`. One line = one `sh` RPC carrying
a freshly-parsed `Script`. The shell server is left running when the client
goes: it keeps its functions and is there for the next client.

**A prompt is a process.** `sh --prompt` runs `sh` in `ShMode::Prompt`, a mode
whose process does nothing on the node: it is the view's lifetime, and the work
is all on the client. The mode carries the client's hostname - its own, or
`DUSK_CLIENT_HOSTNAME` - which the process takes as its name, so it appears in
`ps` as `sh[prompt ⟷ pc1]` and says which machine is at it. The `sh` entry builds its args with a `created` callback
carrying the pid of the shell server to attach to (`defaultPid`, or the one
`--prompt <pid>` names), so the node calls back into the client that built the
args and the prompt opens there, driving the shell server at that pid. Leaving
the prompt kills the prompt process - never the shell server - so whoever ran
`sh --prompt` is told the view is over: `dusk <address>` returns, and
`node.prompt()` returns. A
prompt-mode `output` parks until that happens, so a `sh --prompt` inside a
script waits for its view the way `logs view`'s statement waits for its pager.
The callback refuses instead - failing the `Dusk.process` call that fired it,
and killing the prompt process it was called with - when `DUSK_NON_INTERACTIVE`
is set, when the client's output is not a terminal, or when a prompt is already
open on it. A terminal has one prompt, so a `sh --prompt` typed at a prompt is
an error rather than a second view on the same screen.

**`sh --server`** starts the node's default shell server and nothing else: no
view, no callback, `output` answers `daemonize` so the statement that ran it
leaves it running for clients to attach to. Its pid is `defaultPid`, or the one
`sh --server <pid>` names - the same pid `sh --prompt <pid>` attaches to, so a
shell server outside the default one is two commands rather than a special case.

**One command.** `dusk <address> "ps"` and `dusk.sh(...)` from Python run `sh`
in `ShMode::Script`, with no callback at all, and drive it themselves:
`Dusk.process`, `Dusk.run` - a process of its own, not the caller's session -
then the portal, `OutputPortal.output` into the caller's stream, and `kill` plus
`waitpid` when `output` returns without daemonising. That is what the node's
interpreter does for every program in a script, done by the client for the one
program it runs itself.

**`sh` as a program.** When `sh <command>` (or `sh -d <command>`) runs as a
program - nested in another script, or launched directly - the command string is
parsed at args-build time (`ShArgs::new`, `base/sh/src/client/mod.rs`) into a
`Script` baked into the program's args as `ShMode::Script` / `DetachedScript`.
The script then runs when the caller drives the process's `OutputPortal.output`
(or, for a detached script, immediately in `Process::main` against a discard
stream).

Either way the server side is identical: a `Script` reader handed to
`Interpreter::exec`.

## Syntax

The grammar is a nom parser in `base/sh/bytecode/`. It is deliberately tiny.

**Comments** are stripped before parsing (`strip_comments`, quote-aware): `#`
to end of line, and only when the `#` starts a word - a `#` inside a word
(`http://host/page#section`) is just a character. Text inside `'…'` / `"…"` is
preserved verbatim.

**A script** is a list of statements separated by `;`, newline, or `\r`. A
trailing separator is allowed.

**A statement** is either a function definition or an expression.

**An expression** is a command, or a left-associative chain of commands joined by
`&&` / `||`:

```
ps && date || true
```

`&&` and `||` share one precedence level and fold left - there is no
parenthesised grouping, and the operators only join *commands*, not sub-chains.

**A command** is a run of space-separated words; the parser stores the raw source
slice (`recognize`), it does **not** split into argv here - that happens later,
during compilation. A **word** is single-quoted, double-quoted, or bare. Quotes
use `is_not` - there is no escape character; a quote runs to the next matching
quote. A bare word ends at any of `` \t\r\n;&|(){} ``.

**A function definition** is `identifier () { body }`, where `body` is a full
nested script and `identifier` is `[A-Za-z0-9_]+`:

```
greet() {
    date
    ps
}
```

`Parser::parse` strips comments, runs the nom `ast` parser, and rejects the input
as `"syntax error"` if anything fails or any non-whitespace remains unconsumed.
On success it walks the AST straight into the capnp `Script` builder.

## The wire format

`bytecode.capnp` mirrors the AST one-to-one:

```capnp
struct Script {
  struct Statement {
    struct Expr {
      union { command @0 :Text;  and @1 :ExprPair;  or @2 :ExprPair; }
    }
    union { expr @0 :Expr;  functionDefinition @1 :FunctionDefinition; }
  }
  statements @0 :List(Statement);
}
```

`command` is still the raw text slice; argv resolution is the compiler's job.

## Compilation

`compiler::compile` (`base/sh/src/interpreter/compiler.rs`) lowers a `Script`
reader into a `Frame` - a flat `Vec<Inst>` walked by a program counter. It runs
**every time a script executes** (`Interpreter::exec`), plus per-function via
`compile_function` (see [Functions](#functions)).

The instruction set (`inst.rs`):

| Inst | Meaning |
|------|---------|
| `ProgramArgs(Rc<ProgramArgs>)` | run one external program |
| `Call(symbol)` / `TailCall(symbol)` | invoke a shell function |
| `DefineFunction{ symbol, body }` | define / redefine / (empty body) undefine |
| `JumpIfOk(target)` / `JumpIfError(target)` | conditional jump on the result register |

**`&&` and `||` compile to jumps.** For `a && b` the compiler emits `a`, a
`JumpIfError` placeholder, then `b`, and back-patches the jump target to just
past `b` - so if `a` leaves an error in the result register, `b` is skipped.
`||` is the mirror image with `JumpIfOk`. The "exit status" of a command is
simply whether its `Inst` left `Ok` or `Err` in the register.

**Each command word round-trips to the client.** This is the part worth
internalising. For a command, the compiler looks at the first word. If it names a
known function it emits `Call`. Otherwise it calls
`compiler.build_program_args(text)` - an **RPC back to the client**, on the
`Compiler` capability the script arrived with - which resolves the program name
against the `SH_ENTRIES` table and returns a fully-built `ProgramArgs` capability
(itself wrapping client-side capabilities). That becomes `Inst::ProgramArgs`. So
compilation is *not* a local server operation: every external program in a script
requires the client that sent it to be connected and answering. (A line sent over
`ShPortal.sh` carries its own `Compiler`; a script that arrived in the process's
args gets one from `ShArgs.Server.compiler()`, hosted wherever the `ProgramArgs`
were created.)

**Tail-call optimisation.** If a frame's last instruction is `Call`, it is
rewritten to `TailCall`, which the interpreter executes by reusing the current
frame rather than recursing - so `foo() { foo }` loops forever without growing
the stack.

## Execution

`Interpreter::exec_inner` (`interpreter/mod.rs`) is the loop: a `pc`, a
`result_register: Result<()>`, a cooperative `yield_now` each iteration, and a
`stop` check.

- **`ProgramArgs`** → `Execution::program_args` (`execution.rs`): `Dusk.process`
  then `Dusk.run`, fetch the process portal, cast it to `OutputPortal`, and call
  `output(stream)` with the caller's own stream. Both the portal fetch and the
  output call are `select`ed against the `stop` signal - the fetch as well as the
  call, because a program that does its work before reporting itself ready (as
  `sleep` does) parks the shell on the portal for the whole command, and a
  `stop` raced only against `output` would go unobserved until the work it was
  meant to interrupt had finished. Returning from `output` is how a program says
  it is finished, and the `daemonize` it answers with is how it says whether it
  means to keep running: the process is killed (`SIGTERM`) and reaped
  (`waitpid`) unless it asked to be left alone. These RPCs go through
  `dusk_core::local_client` - an **in-process, server-local** `Dusk` client - so
  spawning/killing does not touch the network. (The launched program may still
  hold the *remote* client embedded in its args.)
- **`Call`** resolves the function's frame and runs it as a nested `exec_inner`;
  **`TailCall`** swaps the current frame and resets `pc` to 0.
- **`DefineFunction`** mutates the function table (see below).
- **Jumps** set `pc` from the result register.

`exec` writes into the caller's stream and never closes it - one line of shell
runs many programs into the same one. `ShPortal.sh` closes it once the line is
finished, error or not (a failure to close is logged at `warn`).

## Functions

Functions are the one piece of shell state that outlives a single line.

**Storage.** The `function_table` (`Arc<Mutex<HashMap<String, ScriptWrapper>>>`)
belongs to the `sh` `Process`, made when the process is. Each `sh` has its own:
a function defined at a prompt lives in the shell server that prompt is attached
to, where the next client attaching to it finds it, and a `sh <command>` or
`sh -d` elsewhere on the node - its own process - does not have it.

**Definition.** `name() { … }` compiles to `DefineFunction`, which at runtime
inserts the body (an owned capnp message) into the table, drops any stale
compiled frame, and eagerly recompiles. Redefining overwrites (logged at `info`).
Defining with an **empty body** removes the function - that is how you undefine
one.

**Compilation & caching.** Function bodies are compiled by `compile_function`
into a per-`Interpreter` `compiled_functions` cache. Compilation is lazy on first
`Call`, eager on definition (and eagerly chases the dependencies a body calls).
Recursion is handled by inserting an empty placeholder frame under the symbol
*before* compiling the body, so a self-reference short-circuits instead of
looping the compiler.

**No arguments.** A function call is a bare word. Passing arguments to a function
(`greet foo`) is a compile error - functions take no parameters.

**Listing.** `ShPortal.functions()` returns the table's keys. The prompt calls
this *before every prompt render* to feed the syntax highlighter (and the
`functions` builtin uses it too). Worth knowing: that per-prompt call is a real
RPC to the server on the hot path, and is the first thing to fail (silently, at
`warn`) if the connection has dropped between commands.

## When the client connection is needed

| Operation | Needs the live client connection? |
|-----------|-----------------------------------|
| Parsing text → `Script` | No - runs entirely client-side, before anything is sent |
| Submitting a line / awaiting its output | Yes - `ShPortal.sh`, then await `done` |
| **Compiling** each external program | **Yes** - `build_program_args` RPCs *back* to the client, on the line's `Compiler`, per program |
| Spawning / killing the resulting process | No network - uses the server-local `dusk_core::local_client` |
| Listing functions (highlighter, `functions` builtin) | Yes - `ShPortal.functions`, once per prompt |

The connection itself is resilient, and so is the shell on top of it.
`Connection` wraps its capability in `capnp_rpc::auto_reconnect`, and so does
`Shell`: the process it was handed is the first incarnation, and when a call
finds the node gone, `Shell::recreate_sh_process` makes a fresh server-mode `sh`
at `defaultPid` on the node that answers next - with no `created` callback on
those args, so no second prompt opens. Meanwhile the keepalive pings `pid()` on
an RTT-adaptive interval and the status line reads `disconnected` while it
fails. A prompt therefore outlives the node it was opened against, which is why
the CLI waits for the prompt rather than for its `process.run()` - that call
dies with the connection.

## Error handling

Errors are values, not panics, and they travel back along the same path the
script came in on.

- **Syntax errors** surface client-side from `Parser::parse` as `"syntax
  error"`. On the interactive path `Shell::sh` returns the error and
  `execute_command` logs it at `error` - the prompt survives.
- **Compile errors** - an unknown program (`no sh entry found for …`), a
  malformed command, a function given arguments, or an unknown function - abort
  `compile`, which fails `exec`, which the `sh_exec_task` reports through its
  completion signal; `ShPortal.sh` then returns a capnp error that the client
  `await`s and logs.
- **Execution errors** are split into `ExecutionError::Runtime` (infrastructure:
  a failed RPC, a missing portal - logged at `error` server-side) and
  `ExecutionError::Program` (the spawned program itself reported failure via its
  portal or non-zero `waitpid`). Both land in the result register, where `&&` /
  `||` read them as exit status.
- **Eager dependency compilation** deliberately swallows errors - a missing or
  broken function body is left for the runtime `Call` to surface, so defining a
  function that references a not-yet-defined one is not itself an error.
- **`daemonize`** is not an error at all: it is a program saying it means to
  keep running, and tells the interpreter to skip the kill/reap.
