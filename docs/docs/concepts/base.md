# Base

Dusk Base, is a set of programs composing the core utilities of the dusk program ecosystem.

These programs are optional, and swapable. You could write your own out of source version of Dusk Base and compile the dusk artifacts with that.
The relation between Dusk Base and Dusk is similar to the relation between GNU Coreutils and the Linux Kernel. 

You can definetly go without it, but Dusk Base together with Dusk composes a much more complete OS-like experience at the node level.

# Programs under Base

## `sh`

The shell. Runs commands. It's what you talk to when you connect to a dusk node
with the `dusk` CLI.

## `ps`

Lists the processes currently running in the node. Shows their pid, name, etc.

## `kill`

Sends a signal to a process by pid. Use it to kill a process, or
to send any other signal to a running process.

## `sleep`

Waits for the given number of milliseconds and exits.

## `date`

Prints the node's current internal wall-clock time, or sets it to a given unix
timestamp (in milliseconds).

## `true` / `false`

Two programs that do nothing except report success (`true`) or failure
(`false`). They're building blocks for shell conditionals.
