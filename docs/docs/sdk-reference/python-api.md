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

### `Dusk(address: str, port: int, sh_server_pid: int | None = None, *, server_name=None, ca=None, certificate=None, key=None)`

Connects to a node at `address:port` and takes hold of a shell server on it: the
node's default one at `defaultPid`, or the one at `sh_server_pid` - started
there if nothing is running it yet. The constructor blocks until the connection is established and
the node answers, and raises if it can't reach the node. A node that has not
answered within 60 seconds counts as one it can't reach: the constructor raises
a `Disconnected` `RuntimeError`.

`address` is a host name, an IPv4 address or an IPv6 address (`"::1"`, without
brackets). Over plain TCP the constructor resolves a host name and tries each
address it resolves to in turn until one answers, and the object connects to
that address from then on. Over TLS the name is resolved again each time the
object connects, and each address it resolves to is tried in turn until one
takes the connection. The name lookup has 10 seconds, and so does each address:
over plain TCP, an address that takes the connection and then says nothing is
given up on and the next one tried; over TLS, the handshake has 10 seconds of
its own.

Every command this object runs goes to that shell server, so the shell server's
state is the object's state: a function defined by one `sh` call is there for the next one,
and two objects on different pids do not see each other's.

#### Over TLS

The keyword arguments connect over TLS 1.3 or 1.2 instead of plain TCP, to a
server that terminates TLS in front of a node:

| Argument | What it is |
|----------|------------|
| `ca` | A PEM file of the certificates the server's certificate must chain to. Passing it is what turns TLS on. |
| `server_name` | The name the server's certificate must carry, sent as the TLS server name (SNI). Defaults to `address`. |
| `certificate` | A PEM file holding this client's certificate chain, leaf first. |
| `key` | A PEM file holding the private key of `certificate`. |

Every TLS argument needs `ca`; without it the constructor raises `ValueError`.
`certificate` and `key` go together, and the constructor raises `RuntimeError`
when it gets one without the other. The files are read again each time the
connection is made, a reconnect included, so a certificate renewed on disk is
used without making a new object. Paths are strings or `pathlib.Path`s.

```python
node = dusk.Dusk(
    "node.example.internal", 8444,
    ca="/etc/client/ca.pem",
    certificate="/etc/client/client.pem",
    key="/etc/client/client.key",
)
```

#### Errors

The constructor, `sh` and `prompt` raise `RuntimeError`. When the failure is one
the node or the connection reported, the message starts with its Cap'n Proto
error kind - `Failed`, `Overloaded`, `Disconnected` or `Unimplemented` -
followed by `: ` and the reason:

```
Disconnected: Connection refused (os error 111)
```

A connection that can't be made - a closed port, a name that does not resolve,
a refused TLS handshake - is `Disconnected`. A failure on the client side - a
TLS file that can't be read, a certificate without its key - has no kind in
front of it.

After a `Disconnected` failure the object's next call connects again. When the
attempt before it ended less than 30 seconds earlier - whether it failed or made
a connection that has broken since - that call first waits a random time below
a limit that starts at 0.1 seconds and doubles with each such attempt in a row,
up to 30 seconds, so a node that is away is not dialled in a tight loop.

### `node.sh(command: str) -> ShellOutput`

Runs `command` in this object's shell server and returns a `ShellOutput` - an iterator
over the command's output values. Iterating pulls values as they stream back;
iteration ends when the command finishes.

### `node.prompt() -> None`

Opens an interactive prompt on this object's shell server, on the calling
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

## `DUSK_CLIENT_HOSTNAME`

A prompt shows up in the node's `ps` as `sh[prompt ⟷ <hostname>]`, naming the
machine the prompt is on. Set `DUSK_CLIENT_HOSTNAME` to send a name of your own
instead of the one the machine reports.
