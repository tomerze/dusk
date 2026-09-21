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
kvs get <key>            # print the value under <key>; fails if there is none
kvs set <key> <value>    # store <value> under <key>, replacing what was there
kvs delete <key>         # remove <key>; prints whether it was there
kvs exists <key>         # print whether <key> is there
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
hand can show it under the name it was hashed from. An id no program registered
a name for has only its number.

Two different names can hash to the same id. It is unlikely, and if it happens
the two names silently share one entry.

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
