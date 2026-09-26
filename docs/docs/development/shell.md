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
| `base/sh` | both | the `sh` program. `client/` is client-side; `interpreter/` is server-side; `capnp/sh.capnp` is the wire contract and `capnp/bytecode.capnp` the bytecode a script compiles into |
| `base/sh/compiler` | client | `dusk_program_sh_compiler` - the tokenizer, the nom grammar, the `Ast` and the syntax error; `sh` depends on it only under its `client` feature, so a node never links it |
| `base/sh/proc` | client (`std`) | `#[sh_entry]` |
| `base/sh/src/client/prompt/` | client (`std`) | reedline UI, builtins, draws output |
| `base/sh/src/client/shell/` | client (`std`) | `Shell` - drives the `sh` process a client was handed |
| `dusk_connection` | client (`std`) | `Connection` - the TCP/RPC link |

## Representations at a glance

Three stages, each named for what does the work, with what it produces beside it:

```
Source (Text)   ->   Compiler (Bytecode)   ->   Codegen (Instructions)
```

In full, with every intermediate form a line of shell passes through:

```
raw text            "ps && date  # comment"           reedline buffer (client/prompt)
   │ strip_comments
stripped text       "ps && date  "                    quote-aware comment removal
   │ nom (compiler/src/tokenize.rs)
Ast                 Ast{ [Expr(And(Command "ps",       transient, client-side only
                                   Command "date"))] }
   │ compile_into - each command resolved to ProgramArgs
capnp Bytecode      Bytecode{ statements:[…] }         the wire format (bytecode.capnp)
   │  ──────────────────────────────────────────────   CLIENT / SERVER BOUNDARY
   │ codegen::generate
instructions        0000: program_args                 Vec<Inst>
                    0001: jump_if_error 0003
                    0002: program_args
   │ Interpreter::exec_inner
execution           Dusk.process + Dusk.run per Inst   spawns real processes
```

The first four rows live entirely in the client. The `Bytecode` is the only thing
that crosses the network. Code generation and execution happen server-side, inside
the `sh` process.

## Two entry paths

There are two distinct ways a script reaches the interpreter, and they parse at
different moments.

**Interactive prompt.** `dusk <address>` with no command runs one command:
`sh --prompt`. `open_prompt` builds those args itself rather than through the
`sh` entry, and calls `Dusk.process` then `process.run()`. The node calls the args'
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

Each accepted line goes `Prompt::execute_command` → `Shell::sh`, which asks the
server which functions are defined, **compiles the text on the client** into bytecode -
resolving each command to its `ProgramArgs` and each word naming a function to a
`call` - and ships it via `ShPortal.sh(script, output, stop)`. One line = one
`sh` RPC carrying freshly compiled bytecode. The shell server is left running when the
client goes: it keeps its functions and is there for the next client.

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
parsed at args-build time (`ShProgramArgsBuilder::build`,
`base/sh/src/client/cli.rs`) into bytecode baked into the program's args as
`ShMode::Script` / `DetachedScript`.
The script then runs when the caller drives the process's `OutputPortal.output`
(or, for a detached script, immediately in `Process::main` against a discard
stream).

A detached script that is one program - one statement, a `programArgs`
expression (`folds`, `base/sh/src/lib.rs`) - leaves the `sh` process out of it.
`Process::main` spawns the exec task, sends `ready` and returns, so the `sh`
process exits at once and whoever ran it reaps it: `init` with `waitpid`, the
interpreter through the portal refusal described under errors below. The exec
task runs the program the way any statement runs - `output` into the discard
stream, then kill and `waitpid` - so `ps` shows the program alone, and it is
reaped when it ends. `main` records the decision in `State.folded`, and `output`
answers `daemonize = !folded`, so a caller that reaches `output` of the exited
`sh` still reaps it. Any other detached script keeps its `sh` process, which
answers `daemonize` and stops the script when it is terminated.

Either way the server side is identical: a `Bytecode` reader handed to
`Interpreter::exec`.

## Syntax

The grammar is a nom parser in `base/sh/compiler/`. It is deliberately tiny.

Source is parsed to bytecode before it goes anywhere else. `ShMode::Script`
and `ShMode::DetachedScript` carry bytecode, never source, so every caller runs
`compile` first - the CLI, the `sh` entry, the prompt's `Shell::sh`
through `compile_into` straight into the outgoing message - and there is
one description of what a script compiles to.

Compiling needs the `SH_ENTRIES` table and the command's clap parser to build that
program's args, so it happens wherever the source is read. That is the client at a
prompt, and it is the build for a script known in advance:
`dusk_program_sh_compiler_proc::compile_sh!` resolves a command through
`compile_to_words` and hands back the bytes while the calling crate compiles, which
is how the node artifact gets its init script. It takes the source as a string
literal, or as `env!("NAME")` to read it from an environment variable at that
moment - the node artifact reads `DUSK_NODE_INIT_SCRIPT` - and a change to the
variable recompiles the calling crate. Bytes cannot carry a capability, so
the `Dusk` client the entry builder is handed is disconnected, and each command's
args `Server` and `created` are left out: a program in such a script that asks for
its `Server` fails with `Message contains null capability pointer`. Every other caller
gets its script from `compile` as a `BytecodeMessage` (a `capnp_rpc::ImbuedMessageBuilder`), which keeps
each command's capabilities in a table beside the message, so `ShMode::Script` and
`ShMode::DetachedScript` hand them to the node. `tests/common` picks its port at run
time, so it calls `compile_to_words` itself rather than going through the macro.

`compile_to_words` compiles with `compile`, then copies the bytecode into a plain
message command by command, leaving each command's `Server` and `created` behind.
An `sh` command's args data is itself a script - `sh`'s entry compiles `ps` in
`compile_sh!("sh -d ps")` with `compile` - so its script or detached script is
copied the same way, and `sh <command>` and `sh -d <command>` compile at build
time. Any other program whose args data itself holds a capability cannot go into
bytes: the compile fails with an error naming the program, and `compile_sh!`
turns it into a compile error where the macro was called.

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

**A command** is a run of space-separated words; the grammar stores the raw source
slice (`recognize`) in the `Ast`, it does **not** split into argv here - that
happens in resolution, where `command_words` splits the slice and the program's
entry builds its args. A **word** is single-quoted, double-quoted, or bare. Quotes
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

`parser::parse` runs the nom `ast` parser and rejects the input
as `"syntax error"` if anything fails, and as ``"syntax error at `…`"``, naming
the line it stopped on, if non-whitespace remains unconsumed. `compile_into` strips
the comments before it, resolves every command the `Ast` holds, and then walks
the `Ast` into the capnp `Bytecode` builder.

## The wire format

`bytecode.capnp` mirrors the `Ast`, with each command already resolved:

```capnp
struct Bytecode {
  struct Statement {
    struct Expr {
      union {
        programArgs @0 :Dusk.ProgramArgs(AnyPointer, AnyPointer);
        and @1 :ExprPair;
        or @2 :ExprPair;
        call @3 :Text;
      }
    }
    union { expr @0 :Expr;  functionDefinition @1 :FunctionDefinition; }
  }
  statements @0 :List(Statement);
}
```

A command is a built `Dusk.ProgramArgs`, not a text slice for the server to
resolve, and a word that names a function is the `call` variant - which word is
which is settled while the bytecode is built. Other schemas import this one as
`using Bytecode = import "/capnp/bytecode.capnp";` and spell the field type
`Bytecode.Bytecode`: `ShArgs.Data`'s `script` and `detachedScript`, `ShPortal.sh`'s
`script`, `InitArgs.Data`'s `initScript`. The generated Rust module is
`bytecode_capnp`, re-exported by `base/sh/src/lib.rs` as `bytecode`, so Rust says
`bytecode::Reader`.

## Code generation

`codegen::generate` (`base/sh/src/interpreter/codegen.rs`) generates `Instructions`
from a `Bytecode` reader - a flat `Vec<Inst>` walked by a program counter. It runs
**every time a script executes** (`Interpreter::exec`), plus per-function via
`generate_function` (see [Functions](#functions)). Both dump the result at `debug`,
under `script instruction disassembly` and `function instruction disassembly`.

The instruction set (`instructions.rs`):

| Inst | Meaning |
|------|---------|
| `ProgramArgs(Rc<ProgramArgs>)` | run one external program |
| `Call(symbol)` / `TailCall(symbol)` | invoke a shell function |
| `DefineFunction{ symbol, body }` | define / redefine / (empty body) undefine |
| `JumpIfOk(target)` / `JumpIfError(target)` | conditional jump on the result register |

**`&&` and `||` become jumps.** For `a && b` codegen emits `a`, a
`JumpIfError` placeholder, then `b`, and back-patches the jump target to just
past `b` - so if `a` leaves an error in the result register, `b` is skipped.
`||` is the mirror image with `JumpIfOk`. The "exit status" of a command is
simply whether its `Inst` left `Ok` or `Err` in the register.

**Nothing is resolved here.** This is the part worth internalising. Codegen
reads the union tag and nothing else: a `programArgs` becomes `Inst::ProgramArgs`
around the `ProgramArgs` the bytecode already carries, a `call` becomes `Inst::Call`
on that symbol. Which of the two a word is was decided where the source was
parsed, against the `SH_ENTRIES` table and the functions the server reported, so
generating instructions is a purely local server operation that asks the client nothing. What
the `ProgramArgs` wrap is still client-side - the capabilities inside them are
hosted wherever they were built - so *running* a program needs that host, even
though generating its instructions does not.

**Tail-call optimisation.** If the last instruction is `Call`, it is
rewritten to `TailCall`, which the interpreter executes by swapping the running
instructions rather than recursing - so `foo() { foo }` loops forever without growing
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
- **`Call`** resolves the function's instructions and runs them as a nested
  `exec_inner`; **`TailCall`** swaps those instructions in and resets `pc` to 0.
- **`DefineFunction`** mutates the function table (see below).
- **Jumps** set `pc` from the result register.

`exec` writes into the caller's stream and never closes it - one line of shell
runs many programs into the same one. `ShPortal.sh` closes it once the line is
finished, error or not (a failure to close is logged at `warn`).

## Functions

Functions are the one piece of shell state that outlives a single line.

**Storage.** The `function_table` (`Arc<Mutex<HashMap<String, Rc<RefCell<BytecodeMessage>>>>>`)
belongs to the `sh` `Process`, made when the process is. Each `sh` has its own:
a function defined at a prompt lives in the shell server that prompt is attached
to, where the next client attaching to it finds it, and a `sh <command>` or
`sh -d` elsewhere on the node - its own process - does not have it.

**Definition.** `name() { … }` becomes `DefineFunction`, which at runtime
inserts the body (an owned capnp message) into the table, drops any stale
instructions, and generates them again. Redefining overwrites (logged at `info`).
Defining with an **empty body** removes the function - that is how you undefine
one.

**Generation & caching.** A function body's instructions are generated by
`generate_function` into the `generated_functions` cache, lazily on first `Call`
and eagerly on definition (which also chases the dependencies a body calls).
Recursion is handled by a set of the symbols being generated, local to one
`generate_function` call, so a self-reference short-circuits instead of looping.
Nothing enters the cache until its instructions are complete, and instructions
generated from a body that was redefined meanwhile are not cached at all - every
script the `sh` process runs shares the cache, so a half-built entry would be run
by whichever script called the function next.

**No arguments.** A function call is a bare word. Passing arguments to a function
(`greet foo`) fails while the bytecode is built, with ``function `greet` cannot take
arguments`` - functions take no parameters.

**Listing.** `ShPortal.functions()` returns the table's keys. The prompt calls
this *before every prompt render* to feed the syntax highlighter (and the
`functions` builtin uses it too). Worth knowing: that per-prompt call is a real
RPC to the server on the hot path, and is the first thing to fail (silently, at
`warn`) if the connection has dropped between commands.

## When the client connection is needed

| Operation | Needs the live client connection? |
|-----------|-----------------------------------|
| **Compiling** text into bytecode | Client-side, but **yes** at the prompt - `Shell::sh` asks `ShPortal.functions` first, to tell a function call from a program |
| Submitting a line / awaiting its output | Yes - `ShPortal.sh`, then await `done` |
| **Generating** instructions from bytecode | No - the union tag says what each expression is; nothing is resolved server-side |
| Spawning / killing the resulting process | No network - uses the server-local `dusk_core::local_client` |
| Running a program whose args the client built | Yes for any capability inside those args - they are hosted wherever the args were built |
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

- **Syntax errors** surface client-side from `parser::parse` as `"syntax
  error"`. On the interactive path `Shell::sh` returns the error and
  `execute_command` logs it at `error` - the prompt survives.
- **Resolution errors** are client-side too, raised while the bytecode is built: an
  unknown program (`no sh entry found for …`), a malformed command (`invalid
  command …`) and a function given arguments. They fail the call that was
  building the message, so nothing is sent at all. A function is known from its
  definition on, so a word used before it is defined runs the program of that
  name. Inside a function body, a bare word that names neither a program nor a
  known function compiles to a call and is looked up when the function runs.
- **Codegen errors** - bytecode codegen cannot read, or a `Call` whose function
  has no body (`call to unknown symbol: …`) - fail
  `codegen::generate`, which fails `exec`, which the `sh_exec_task` reports through its
  completion signal; `ShPortal.sh` then returns a capnp error that the client
  `await`s and logs.
- **Execution errors** are split into `ExecutionError::Runtime` (infrastructure:
  a failed RPC, a missing portal - logged at `error` server-side) and
  `ExecutionError::Program` (the spawned program itself reported failure via its
  portal or non-zero `waitpid`). Both land in the result register, where `&&` /
  `||` read them as exit status.
- **A detached script's result** has no caller to return to, so the detached
  `sh`'s `Process::main` keeps the completion `spawn_sh_exec_task` returns and
  watches it beside its signal channel: a failed script is logged at `error` with
  the error's whole chain, a finished one at `info`. The process keeps running
  until `Terminate` either way. A detached script that is one program has no
  `sh` left to watch it, so its exec task is spawned with `logs_result` and logs
  its own result the same way (`log_detached_result`, `base/sh/src/exec.rs`).
- **A portal refused because the process is gone** - it exited before anyone
  asked for it, as a detached `sh` whose script is one program always has - is
  logged at `debug`, not `error`, and the process is killed and reaped the same
  way; `waitpid` gives the statement its result.
- **Eager dependency compilation** deliberately swallows errors - a missing or
  broken function body is left for the runtime `Call` to surface, so defining a
  function that references a not-yet-defined one is not itself an error.
- **`daemonize`** is not an error at all: it is a program saying it means to
  keep running, and tells the interpreter to skip the kill/reap.
