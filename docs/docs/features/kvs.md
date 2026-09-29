# Key-value store

Every Dusk node holds a small in-memory **key-value store**, shared by every
program running on it. Dusk uses it to record facts about the node - which
version it runs, which shell functions are defined, how its log buffer is
doing - and your programs can use it for the same kind of thing: state that
several processes need to see, or a number you want to read back later. The
`kvs` program is how you reach it from the [shell](shell.md); it's a
[Base program](../getting-started/concepts/base.md).

The store lives in memory and starts empty every time the node starts. Nothing
in it survives a restart.

## Commands

```sh
kvs get <key>            # print every key starting with <key>, with its value; fails if there is none
kvs set <key> <value>    # store <value> under <key>, replacing what was there
kvs delete <key>         # remove <key>; prints whether it was there
kvs exists <key>         # print whether <key> is there
kvs scan                 # list every key
```

A value typed at the prompt is stored as a string. Values written by programs
can be any Dusk value - numbers, lists, records - and `kvs get` prints them the
way the shell prints any other output.

## Keys are ids

A key is a name like `dusk.version` when you type it, but the node never sees
the name: the client hashes it to a 64-bit id and sends only that. So the store
is a map from id to value, and nothing on the node can say what a key was
called.

A program registers the names of the keys it writes - every key in the table
below is registered by the program that sets it - so a client that has an id in
hand can show it under the name it was hashed from. The node cannot: it sends
the ids back and the client names them.

`kvs scan` is how you see them. Each row is a key's name and the id it hashes
to, and a key no program registered a name for shows as its id:

```console
> kvs scan
 Key                 ID
 dusk.namespace_id   0x356cac24ff2e7205
 dusk.git_rev        0x73f97df9dc6dba02
 dusk.version        0x7520055bd5de6ac4
 0xbc316f8a9c3bae10  0xbc316f8a9c3bae10
```

Either column is a key you can type back: `kvs get 0x7520055bd5de6ac4` reads
the same entry as `kvs get dusk.version`, because that id is what
`dusk.version` hashes to.

The list is the store at one instant - no key is repeated, and none that was
there is missed - and it arrives as a stream, a page of rows at a time, so a
store with many keys never has to answer in one reply.

Two different names can hash to the same id. It is unlikely, and if it happens
the two names silently share one entry.

## Reading keys by name

`kvs get` reads every key whose name starts with what you type, and answers a
table of `Key` and `Value`, one row per key, in name order:

```sh
kvs get dusk            # dusk.git_rev, dusk.namespace_id and dusk.version
kvs get dusk.version    # dusk.version alone
```

The client finds them the way `kvs scan` lists them: it asks the node for every
id it holds, keeps the ones whose name starts with `<key>`, and reads those. So
only a registered name matches part of what you typed. A key you set yourself,
like `deploy.stage`, is found by its whole name or by its id, and its row shows
the id.

The client asks when it reads the line, not when the line runs. A `kvs get` in a
shell function reads the keys that matched when the function was defined, and
one in a detached script the keys that matched when it was sent; a key written
after that is left out.

`kvs get` fails when no key matches, and names what you typed:
``no key matches `logs` ``.

## What Dusk records

These keys are written by Dusk itself. Read them; overwriting them only lasts
until their owner writes again.

| Key | Written by | Value |
|-----|------------|-------|
| `dusk.version` | `init`, at startup | The node's Dusk version, e.g. `0.1.0`. |
| `dusk.git_rev` | `init`, at startup | The git revision the node's `init` program was built from. |
| `dusk.namespace_id` | `init`, at startup | The node's namespace id, a random 64-bit number chosen at startup. |
| `logs.written` | `logs`, whenever a `logs` command starts and finishes | How many log records have been stored in the node's buffer since it started. Records dropped on the way in (the three `logs.dropped_*` counters) are not counted; records the buffer later overwrote are. |
| `logs.dropped_no_lane` | `logs`, same | Records dropped because their level is routed to no lane of the buffer. |
| `logs.dropped_oversize` | `logs`, same | Records dropped because they are bigger than their lane's whole arena. |
| `logs.write_failures` | `logs`, same | Records that failed to serialize on their way into the buffer. |
| `logs.overwritten` | `logs`, same | A list with one number per lane: how many stored records that lane has destroyed by overwriting its oldest. |

The `logs.*` counters are a snapshot taken by the `logs` program, so they are
as fresh as the last time a `logs` command ran on the node. Run `logs dump
--replay-only` to refresh them.
