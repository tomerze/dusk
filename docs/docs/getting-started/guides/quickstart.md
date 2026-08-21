# Quickstart

Run a node and connect to it in under a minute — the fastest way to see Dusk's
analytics and diagnosis against a live node. This assumes you've completed
[Installation](installation.md). (To put Dusk inside an app you already ship, see
[Embed Dusk in your app](embed.md).)

## Run a node

Start a Dusk node. It listens for clients over TCP on port `9090`:

```bash
cargo run --bin dusk_node
```

## Connect with the CLI

In another terminal, point the `dusk` CLI at the node's address. This opens an
interactive shell prompt connected to the node:

```bash
cargo run --bin dusk -- 127.0.0.1:9090
```

## Run your first command

At the prompt, list the processes currently running on the node:

```sh
ps
```

Type `help` to see every program the node can run. From here:

- the [Shell](../../features/shell.md) reference covers the command language,
- [Connect a client](connect-a-client.md) shows the Python and HTTP entry points,
- [Write your first program](first-program.md) adds a new program of your own.
