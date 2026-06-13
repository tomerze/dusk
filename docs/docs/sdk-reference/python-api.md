# Python API

`dusk_py` builds a Python extension named `dusk` (with maturin) that drives a node
from Python — a client, not an embedded node. Build it with `uv run maturin
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

Runs `command` through the node's shell and returns a `ShellOutput` — an iterator
over the command's output values. Iterating pulls values as they stream back;
iteration ends when the command finishes.

### `node.disconnect() -> None`

Closes the connection. The node and any processes it was running are unaffected;
this just tears down the client side.

### `Dusk.help(program_name: str = "") -> list | dict` *(static)*

Lists the programs compiled into the client and their help text — `name`,
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

## `DUSK_NON_INTERACTIVE`

Some programs are interactive: `logs view` takes over the calling terminal until
the user quits it. If the process embedding this API has no terminal to give
away — a service, a notebook kernel, a gateway — set the `DUSK_NON_INTERACTIVE`
environment variable (to any value) before running commands. Interactive
commands then refuse to run, with an error naming a non-interactive alternative
(`logs view` points at `logs stream`), instead of hanging the caller forever.

The [MCP gateway](../features/mcp.md#no-interactive-views) sets it
automatically in its own process.
