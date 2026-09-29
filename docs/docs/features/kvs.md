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
kvs get <key>            # print every key matching <key>, with its value; fails if there is none
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
kvs get dusk.target     # dusk.target.arch, dusk.target.bits and dusk.target.os
kvs get dusk.version    # dusk.version alone
```

A `*` stands for any run of characters, dots included, so a pattern can leave
out the middle of a name as well as its end. What you type is always matched
from the start of the name and left open at its end, as though it ended in `*`:

```sh
kvs get *               # every key
kvs get logs*           # the same as kvs get logs
kvs get dusk.*.uname    # dusk., then anything, then .uname
```

The client finds them the way `kvs scan` lists them: it asks the node for every
id it holds, keeps the ones whose name matches `<key>`, and reads those. So
only a registered name matches part of what you typed. A key you set yourself,
like `deploy.stage`, is found by its whole name or by its id, and its row shows
the id.

The client asks when it reads the line, not when the line runs. A `kvs get` in a
shell function reads the keys that matched when the function was defined, and
one in a detached script the keys that matched when it was sent; a key written
after that is left out.

`kvs get` fails when no key matches, and names what you typed:
``no key matches `logs` ``.

## When no client can name the keys

The node cannot name a key; the client that ran the command does. A command
whose client is gone cannot have its keys named: one in the node's init script,
which is compiled before the node runs, or one in a detached script whose client
has disconnected. `kvs scan` and `kvs get` still answer, without the `Key`
column - `kvs scan` with `ID` alone, `kvs get` with `ID` and `Value` - and the
node logs a warning that it did.

A `kvs get` compiled into an init script cannot ask the node for its keys
either, so it reads the one key you name, by its whole name or its id.

## What Dusk records

These keys are written by Dusk itself. Read them; overwriting them only lasts
until their owner writes again.

| Key | Written by | Value |
|-----|------------|-------|
| `dusk.version` | `init`, at startup | The node's Dusk version, e.g. `0.1.0`. |
| `dusk.git_rev` | `init`, at startup | The git revision the node's `init` program was built from. |
| `dusk.namespace_id` | `init`, at startup | The node's namespace id, a random 64-bit number chosen at startup. |
| `dusk.target.arch` | `init`, at startup | The CPU architecture the node was built for, as Rust names it, e.g. `x86_64`, `aarch64`. |
| `dusk.target.os` | `init`, at startup | The operating system the node was built for, as Rust names it, e.g. `linux`, `windows`, `android`, `macos`. |
| `dusk.target.bits` | `init`, at startup | The width of a pointer on that target, as a number, e.g. `64`. |
| `logs.written` | `logs`, whenever a `logs` command starts and finishes | How many log records have been stored in the node's buffer since it started. Records dropped on the way in (the three `logs.dropped_*` counters) are not counted; records the buffer later overwrote are. |
| `logs.dropped_no_lane` | `logs`, same | Records dropped because their level is routed to no lane of the buffer. |
| `logs.dropped_oversize` | `logs`, same | Records dropped because they are bigger than their lane's whole arena. |
| `logs.write_failures` | `logs`, same | Records that failed to serialize on their way into the buffer. |
| `logs.overwritten` | `logs`, same | A list with one number per lane: how many stored records that lane has destroyed by overwriting its oldest. |

The `logs.*` counters are a snapshot taken by the `logs` program, so they are
as fresh as the last time a `logs` command ran on the node. Run `logs dump
--replay-only` to refresh them.

## What the impl records

The [impl](../getting-started/concepts/drivers-and-impls.md) writes these keys
once, as the node starts, before `init` runs. Which of them a node has depends
on its impl and on the platform it was built for; the table names the impls in
Dusk's repository that write each one. The impl logs them at `info`: each source
of `dusk.os.*` keys once, as `dusk os`, with a `source` field naming it -
`process`, `time zone`, `uname`, `credentials`, `resource limits`, `os-release`,
`boot id`, `pid 1`, `glibc`, `Android system properties`, `sysctl`,
`windows version`, `windows emulation`, `computer name`, `windows session` - and
a `values` field listing the keys and values it wrote; and the `dusk.device.*`
keys together once, as `dusk device`, with a `values` field. A source that fails
writes no keys, and the node logs why at `warn`. One that this system simply
does not have - no os-release file, no device id, process 1 hidden from the
node, a device that reports no vendor, model or CPU, a Windows or Android too
old to carry a value - writes no keys either, and is logged at `info`.

| Key | Written by | Value |
|-----|------------|-------|
| `dusk.impl` | `nix`, `std`, `windows` | The name of the impl the node was built with, e.g. `nix`, `std` or `windows`. |
| `dusk.os.nix.uname.sysname` | `nix` | The `sysname` field of `uname(2)`, e.g. `Linux`. |
| `dusk.os.nix.uname.nodename` | `nix` | The `nodename` field of `uname(2)`: the name of the device on the network. |
| `dusk.os.nix.uname.release` | `nix` | The `release` field of `uname(2)`: the kernel release. |
| `dusk.os.nix.uname.version` | `nix` | The `version` field of `uname(2)`: the kernel version. |
| `dusk.os.nix.uname.machine` | `nix` | The `machine` field of `uname(2)`: the hardware the kernel reports. |
| `dusk.os.nix.uname.domainname` | `nix`, on Linux and Android | The `domainname` field of `uname(2)`. |
| `dusk.os.linux.os_release.<key>` | `nix`, on Linux | Seven entries of [os-release](https://www.freedesktop.org/software/systemd/man/latest/os-release.html) - `name`, `pretty_name`, `id`, `id_like`, `version`, `version_id` and `version_codename` - each `<key>` the entry's name in lower case and the value without its quotes, e.g. `dusk.os.linux.os_release.id` is `ubuntu`, `dusk.os.linux.os_release.version_id` is `24.04`. The file is `/etc/os-release`, or `/usr/lib/os-release` when that does not exist. |
| `dusk.os.android.release` | `nix`, on Android | The `ro.build.version.release` system property: the Android version, e.g. `14`. |
| `dusk.os.android.sdk` | `nix`, on Android | The `ro.build.version.sdk` system property, as a number: the API level, e.g. `34`. |
| `dusk.os.android.security_patch` | `nix`, on Android | The `ro.build.version.security_patch` system property, e.g. `2024-05-05`. |
| `dusk.os.android.incremental` | `nix`, on Android | The `ro.build.version.incremental` system property: the build's incremental version. |
| `dusk.os.android.model` | `nix`, on Android | The `ro.product.model` system property: the device model, e.g. `Pixel 8`. |
| `dusk.os.android.manufacturer` | `nix`, on Android | The `ro.product.manufacturer` system property, e.g. `Google`. |
| `dusk.os.android.fingerprint` | `nix`, on Android | The `ro.build.fingerprint` system property: the one string that names the exact build the device runs. |
| `dusk.os.android.brand` | `nix`, on Android | The `ro.product.brand` system property, e.g. `google`. |
| `dusk.os.android.build_type` | `nix`, on Android | The `ro.build.type` system property: which kind of build the device runs - a release build, a debuggable one, or an engineering one (`eng`). |
| `dusk.os.android.abi_list` | `nix`, on Android | The `ro.product.cpu.abilist` system property: the ABIs the device runs, e.g. `arm64-v8a,armeabi-v7a,armeabi`. |
| `dusk.os.macos.product_version` | `nix`, on macOS | The `kern.osproductversion` sysctl, e.g. `14.5`. |
| `dusk.os.macos.build_version` | `nix`, on macOS | The `kern.osversion` sysctl, e.g. `23F79`. |
| `dusk.os.ios.product_version` | `nix`, on iOS | The `kern.osproductversion` sysctl. |
| `dusk.os.ios.build_version` | `nix`, on iOS | The `kern.osversion` sysctl. |
| `dusk.os.windows.major_version` | `windows` | The Windows major version, as a number, e.g. `10`. |
| `dusk.os.windows.minor_version` | `windows` | The Windows minor version, as a number, e.g. `0`. |
| `dusk.os.windows.build_number` | `windows` | The Windows build number, as a number, e.g. `22631`. |
| `dusk.os.windows.revision` | `windows` | The update build revision (`UBR`), as a number: the `4169` in `22631.4169`, which says which cumulative update is installed. |
| `dusk.os.windows.edition` | `windows` | The `EditionID`, e.g. `Professional`, `Core`, `ServerStandard`. |
| `dusk.os.windows.display_version` | `windows` | The `DisplayVersion`, e.g. `23H2`. Windows before 20H2 has none. |
