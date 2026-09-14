# Python API

`dusk_py` builds a Python extension named `dusk` (with maturin) that drives a node
from Python - a client, not an embedded node. Build it with `uv run maturin
develop`, then:

```python
import dusk

node = dusk.Dusk('127.0.0.1', 9090)   # connect; blocks until the node answers
for value in node.sh('ps'):           # run a command; iterate its output
    print(value)
node.disconnect()
```

## `Dusk`

### `Dusk(address: str, port: int)`

Connects to a node at `address:port`. The constructor blocks until the connection
is established and the node answers, and raises if it can't reach the node.

### `node.sh(command: str) -> ShellOutput`

Runs `command` through the node's shell and returns a `ShellOutput` - an iterator
over the command's output values. Iterating pulls values as they stream back;
iteration ends when the command finishes.

### `node.prompt() -> None`

Opens an interactive prompt on the node's default shell server, on the calling
terminal, the
way `logs view` opens a pager there. The call blocks while you use the prompt and
returns when you leave it: `exit`, or ctrl+d or ctrl+c while typing a line. Ctrl+c
while a command is running stops that command and keeps the prompt open. Set
`DUSK_NON_INTERACTIVE` where there is no terminal to give - see below.

### `node.disconnect() -> None`

Closes the connection. The node and any processes it was running are unaffected;
this just tears down the client side.

### `Dusk.help(program_name: str = "") -> list | dict` *(static)*

Lists the programs compiled into the client and their help text - `name`,
`version`, `short_description`, `long_description`, `program_id`. With no argument
it returns them all; with a program name it returns just that one. It reads the
link-time program set, so it needs **no** connection.

```python
import dusk
print(dusk.Dusk.help())        # every program
print(dusk.Dusk.help('ps'))    # just ps
```

## `ShellOutput`

The iterator returned by `sh`. Each item is one `Dusk.Value` from the command's
output stream, converted to a native Python value. It's lazy: each step blocks
until the next value arrives, and the iterator is exhausted when the command
signals it's done.

Displaying one - at a prompt, or with `print` - runs the command to completion
and shows everything it wrote, so a bare `node.sh('ps')` is a whole command:

```python
>>> node.sh('echo hello')
['hello']
```

Under IPython and Jupyter the same expression *streams*: each value is written
the moment the node sends it, one per line, so a command that takes a while
shows its output while it runs rather than in one lump at the end.

```
In [1]: node.sh('echo first; sleep 3000; echo second')
Out[1]: 'first'
'second'
```

IPython still records the result, so `Out[1]` and `_1` work as they do for any
other value. Because Python runs between values there, ctrl+c interrupts a
command that is still producing them.

Either way the values are kept, so iterating the same `ShellOutput` afterwards
still yields them. A command that never finishes on its own (`logs stream
<url>`) blocks where it is displayed, exactly as `list()` on it would; iterate
it instead.

## `DUSK_NON_INTERACTIVE`

Some programs are interactive: `logs view`, `sh --prompt` and `node.prompt()` take over the
calling terminal until the user quits them. If the process embedding this API
has no terminal to give away - a service, a notebook kernel, a gateway - set the
`DUSK_NON_INTERACTIVE` environment variable (to any value) before running
commands. Interactive commands then refuse to run, with the reason, instead of
hanging the caller forever: `logs view` points at `logs dump`, and `sh --prompt`
says `there is no terminal to open a prompt on: DUSK_NON_INTERACTIVE is set`.
`node.sh(command)` runs unaffected.

`node.prompt()` and `sh --prompt` also refuse on their own when the process's output
is not a terminal, or when a prompt is already open on it, so a prompt is never
opened where nobody could type into it.

The [API gateway](../features/gateway.md#no-interactive-views) sets it
automatically in its own process.
