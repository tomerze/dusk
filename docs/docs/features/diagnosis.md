# Diagnosis

A Dusk node lets you see and control the work **Dusk itself is running** on a
device, live and over the wire — the same way `docker ps` / `docker logs` /
`docker exec` show and drive containers rather than the host. This is the Dusk
layer you embedded, not the host operating system's process table.

## What you can do today

- **See Dusk's processes** — `ps` lists the
  [processes](../getting-started/concepts/processes.md) running *inside the node*
  (the Dusk programs that have been started), with each one's pid, name, and
  version. Like `docker ps`, it shows Dusk's own process table, not the host's.
- **Read logs** — `logs` opens the node's log buffer in an interactive viewer, or
  streams it to a file, otel collector, or just an http server. See [Logs](logs.md).
- **Run commands** — open the [shell](shell.md) and run Dusk programs against the
  node interactively.
- **Stop or restart work** — `kill` signals a Dusk process by pid; with the shell
  that's enough to recover a stuck workload.
- **Drive it programmatically** — the
  [Python API](../getting-started/guides/connect-a-client.md) and the
  [MCP gateway](mcp.md) do all of the above from a script or an AI agent.

What you can see and do is exactly what the node's programs expose. The
[Base programs](../getting-started/concepts/base.md) cover the Dusk layer itself
(see that page for each program's full usage); to surface something host-specific
(a sensor, a hardware register, an app-internal metric), that's a
[program you write](../getting-started/guides/first-program.md).

## The interfaces

Diagnosis happens through whatever client is handy: the interactive
[shell](shell.md) for a person, the Python API for a script, or the
[MCP gateway](mcp.md) for an agent. They all drive the same node.
