# Key-value store

Every Dusk node holds a small in-memory **key-value store**, shared by every
program running on it. Dusk uses it to record facts about the node - which
version it runs, which shell functions are defined, how its log buffer is
doing - and your programs can use it for the same kind of thing: state that
several processes need to see, or a number you want to read back later. The
`kvs` program is how you reach it from the [shell](shell.md); it's a
[Base program](../getting-started/concepts/base.md).

The store lives in memory and starts empty every time the node starts, except
for its [persistent keys](#persistent-keys): a node built with a persistent file
keeps those in it, and they are back when the node starts again.

## Commands

```sh
kvs get <key>            # print every key matching <key>, with its value; fails if there is none
kvs set <key> <value>    # store <value> under <key>, replacing what was there
kvs delete <key>         # remove <key>; prints whether it was there
kvs exists <key>         # print whether <key> is there
kvs scan                 # list every key, with its flags
```

A value typed at the prompt is stored as a string. Values written by programs
can be any Dusk value - numbers, lists, records - and `kvs get` prints them the
way the shell prints any other output.

## Keys are ids

A key is a name like `dusk.version` when you type it, but the node never sees
the name: the client hashes it to a 64-bit id and sends only that. So the store
is a map from id to value, and nothing on the node can say what a key was
called.

A program registers the names of the keys it writes - every key in the
[What Dusk records](#what-dusk-records) table is registered by the program that
sets it - so a client that has an id in hand can show it under the name it was
hashed from. The node cannot: it sends the ids back and the client names them.
The keys in [What the impl records](#what-the-impl-records) are written by the
impl, not by a program, and `init` registers their names, so `kvs scan` shows
them by name too.

`kvs scan` is how you see them. Each row is a key's name, the id it hashes
to and the key's [flags](#flags), and a key no program registered a name for
shows as its id:

```console
> kvs scan
 Key                 ID                  Flags
 dusk.namespace_id   0x356cac24ff2e7205  sticky
 dusk.git_rev        0x73f97df9dc6dba02  sticky
 dusk.version        0x7520055bd5de6ac4  sticky
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

## Flags

Every key carries a set of flags beside its value, and `kvs scan` names them in
its `Flags` column. A key with none shows an empty cell. A set replaces the
key's flags along with its value.

### Sticky keys

A **sticky** key is one only Dusk sets: every key in
[What Dusk records](#what-dusk-records) and
[What the impl records](#what-the-impl-records) is sticky, so a node's
`dusk.namespace_id` or `dusk.hostname` is what Dusk wrote there, or nothing
until Dusk writes it, unless someone overrode it with `--forbidden-unstick`.
Stickiness belongs to the key's name, not to a value stored under it: the
programs that own Dusk's keys - `init` for the `dusk.*` names, `logs` for its
counters - claim their names as the node starts, so a name is sticky before Dusk
has written to it, like `logs.written` before any `logs` command has run or
`dusk.hostname` on a node whose driver could not read the hostname. `kvs set`
and `kvs delete` refuse a sticky key, naming its id -
`kvs set dusk.hostname web-1` fails with:

```text
key 0xa4cf005db444b151 is sticky: only Dusk sets it. `kvs set --forbidden-unstick` sets it anyway
```

`--forbidden-unstick` overrides the refusal. The key then holds your value until
Dusk writes it again - `init` writes its keys when the node starts, `logs` its
counters whenever a `logs` command runs - and its name stays sticky, so changing
it again takes `--forbidden-unstick` too:

```sh
kvs set --forbidden-unstick dusk.hostname web-1
kvs delete --forbidden-unstick logs.written
```

The node logs every override that sets or removes a key at `warn`, with the
key's id in a `key` field: `forbidden-unstick overwrote a sticky key` or
`forbidden-unstick removed a sticky key`.

A client driving a `kvs bind` process over its portal meets the same refusal:
`KvsPortal.set` and `KvsPortal.delete` refuse a sticky key unless the call sets
its `forbiddenUnstick` parameter, the portal's `--forbidden-unstick`. A server
for a protocol in which stickiness means nothing, such as the Redis server of
PR #83, would pass it on every call.

A program on the node writing through `dusk_program_kvs_internal::Kvs::set` sets a
sticky key like any other, which is how Dusk writes the keys that are sticky.
`Kvs::set_unless_sticky` and `Kvs::delete_unless_sticky` are the ones that
refuse, the way `kvs set`, `kvs delete` and the portal do. A program makes names
its own with `dusk_program_kvs_internal::own_keys`, passing its thread's
`dusk_core::driver::tid()` and a `static` array of their ids - from its
launcher's `new`, as `init` and `logs` do, so the names are sticky before any
client connects - gives them back with `disown_keys` and the same `static` when
the launcher is dropped, and lists the same names with `known_keys!` so a client
can show them by name. Ownership belongs to the node on that thread: a node on
another thread of the same process, built with other programs, does not see it.
A value set with `FLAG_STICKY` in its flags is sticky too, under a name nobody
owns, until a set without the flag replaces it.

### Sensitive keys

A **sensitive** key holds a secret - a token, a password, a key. Mark one when
you set it:

```sh
kvs set --sensitive deploy.token 9f2c41d07be3
```

Neither `kvs` nor the store under it writes a sensitive value into the node's
[logs](logs.md) or a trace: they log the key, never the value. `kvs get` still
answers the value to whoever asks for it, and `kvs scan` names the flag
`sensitive`. The mark
belongs to the set that made it: a later `kvs set` without `--sensitive`
replaces the value and clears the mark.

Sensitive keeps the value out of the node's own records; it does not hide it
from a client. Every client that connects to the node can read it with `kvs
get`.

A program on the node marks a key the same way, passing `FLAG_SENSITIVE` in
the flags of `dusk_program_kvs_internal::Kvs::set`, and a client driving a
`kvs bind` process passes it in the `flags` of `KvsPortal.set`. A program that
reads a key with `Kvs::get_with_flags`, and a client that reads one with
`KvsPortal.get`, gets its flags with its value, and keeps a sensitive one out of
its own logs too.

`sensitive` and `persistent` are the flags a client sets. `kvs set` and
`KvsPortal.set` refuse any other - `sticky` included, which only a program on the
node sets - with
`flags 0x01 hold a flag a client cannot set; a client may set sensitive, persistent`.

### Persistent keys

A **persistent** key survives the node restarting, crashing or losing power:

```sh
kvs set --persistent deploy.region eu-west-1
```

The node keeps it in its persistent file as well as in memory, and reads the
file back when it starts, so the key is there again, with its value and its
flags. `kvs set --persistent` answers once the record is on disk: the node syncs
the file before it does. A later `kvs set` without `--persistent` takes the key
out of the file, and `kvs delete` removes it from both. `kvs scan` names the flag
`persistent`, and a key can carry it with `sensitive`:
`kvs set --persistent --sensitive deploy.token 9f2c41d07be3`.

A node keeps persistent keys only when it was built with a file: the `persistent`
field of the kvs launcher's `KvsConfig`, which the node artifact sets from
[`DUSK_NODE_KVS_PERSISTENT`](../embedding/node-artifacts.md#dusk_node_kvs_persistent)
when it is built. On any other node `kvs set --persistent` fails with
`this node keeps no persistent kvs keys: its kvs launcher was built without a file`
and stores nothing. An impl without a file system leaves the field unset.

The kvs launcher registers its file for the thread it is built on, and every
namespace's kvs asked for on that thread while the launcher holds the file binds
to it - a node has one namespace - so a node's launcher set is built on the
thread that runs the node - as the impls' `run` does, calling the builder there -
or the node keeps no persistent keys.

A program on the node sets a persistent key by passing `FLAG_PERSISTENT` to
`dusk_program_kvs_internal::Kvs::set`, reads it like any other key, and asks
`Kvs::keeps_persistent_keys` whether the node has a file that opened. A set or
delete that cannot reach the file fails and leaves the key as it was in memory;
when the record reached the disk before the failure - the write went through and
the sync failed - it can still come back when the node restarts.

#### The file

The file is an append-only log. Every set and delete of a persistent key appends
one record, encrypted and authenticated with XChaCha20-Poly1305 under a key
derived with HKDF-SHA256 from the node's
[fleet token](../embedding/node-artifacts.md#the-fleet-token), salted with the
device's [`dusk.device.id`](#what-the-impl-records), with the info
`dusk-kvs-persistent-v1`. A device without an id derives the key from the fleet
token alone, and the node logs that at `info`. So a file opens only on the device
that wrote it, under a node built with the same fleet token: a node rebuilt with
another token - including a new random one, see the fleet token's section -
cannot read the keys its earlier build kept.

The encryption keeps the file's contents from anyone who copies the file off the
device and has neither the fleet token nor the device's id. It does not keep them
from anyone else: the fleet token is in the node's binary, which usually sits on
the same disk as the file, and every client that connects to a node of the same
build can ask for it with `Dusk.fleetToken`; the device's id is a kvs key every
client can read. Mark a secret `--sensitive` as well, to keep it out of the
node's logs.

When the node starts it replays the file, and logs what it found at `info`:

- A last record that runs past the end of the file or fails to authenticate - the
  node lost power while writing it, or it is damaged - is dropped, the file cut
  back to the record before it, and the node logs that at `warn` with the bytes it
  dropped. A damaged length in the middle of the file looks the same, and drops
  every record after it.
- A damaged record with records after it: the node keeps the records before it,
  drops the rest, and logs that at `error`.
- A file whose first record does not open under the node's key - a file from
  another device or another build, or one whose first record is damaged - or one
  holding a record a newer Dusk wrote, is left as it is, and logged at `error`.
  The node then keeps no persistent keys: `kvs set --persistent` fails naming the
  reason, until the file is moved away and the node restarts.

A key the file holds is restored with the flags it was kept with, unless Dusk
has already written it in this run - the impl writes `dusk.impl`, `dusk.os.*` and
`dusk.device.*` before the file opens. Then Dusk's value stays, and the node
drops the file's copy and logs the key at `warn`. A key Dusk writes after the
file opened - `init`'s keys, `logs`' counters - replaces the restored value, and
the node drops the file's copy and logs the key at `warn` the same way. So an
override kept with `kvs set --persistent --forbidden-unstick` lasts until Dusk
writes the key again.

The node syncs the file's directory when it creates the file. When the file has
grown past twice the size of its live records plus 64 KiB, the node compacts it:
it writes the live records to `<file>.tmp`, syncs that, renames it over the file
and syncs the directory again. A crash before the rename leaves the old file
whole. Where the directory cannot be opened to sync it - opening a directory as a
file fails on Windows - the node logs that at `warn`, and until the file system
flushes the directory on its own a power loss can lose a new file, or bring back
the file from before a compaction along with every key written since.

A file belongs to one node. A second node in the same process built with the
same path fails to start: its kvs launcher refuses the file, with
``another node of this process keeps its persistent kvs keys in `<path>` ``. When
a node's launcher set is dropped, or its run returns, its kvs closes the file
and refuses persistent sets from then on, and another node may take the file.
Nothing stops two node processes from writing one file - the file system driver
has no lock - and two that do overwrite each other's records: give every node on
a device its own file.

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
column - `kvs scan` with `ID` and `Flags`, `kvs get` with `ID` and `Value` - and the
node logs a warning that it did.

A `kvs get` compiled into an init script cannot ask the node for its keys
either, so it reads the one key you name, by its whole name or its id.

## What Dusk records

These keys are written by Dusk itself, and they are [sticky](#sticky-keys):
`kvs set` and `kvs delete` refuse them. Overwriting one with
`--forbidden-unstick` only lasts until its owner writes it again.

| Key | Written by | Value |
|-----|------------|-------|
| `dusk.version` | `init`, at startup | The node's Dusk version, e.g. `0.1.0`. |
| `dusk.git_rev` | `init`, at startup | The git revision the node's `init` program was built from. |
| `dusk.namespace_id` | `init`, at startup | The node's namespace id, a random 64-bit number chosen at startup. |
| `dusk.tid` | `init`, at startup | The id of the thread the node runs on, as the impl's driver reports it - the operating system's thread id on Linux, Android, Apple systems and Windows. |
| `dusk.hostname` | `init`, at startup | The node's hostname, as the impl's driver reports it. Left unset, with a warning in the node's logs, when the driver cannot read it. |
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

`init` logs `dusk.target.arch`, `dusk.target.os` and `dusk.target.bits` once, at `info`, as
`dusk target` with `arch`, `os` and `bits` fields, and then the impl the node was
built with as `dusk impl` with a `name` field - the value of
[`dusk.impl`](#what-the-impl-records). A node whose impl did not write
`dusk.impl` logs that at `warn` instead.

## What the impl records

The [impl](../getting-started/concepts/drivers-and-impls.md) writes these keys
once, as the node starts, before `init` runs, and they are
[sticky](#sticky-keys). The store only lives while something
holds it, so the impl moves its `Arc<Kvs>` into the launcher-set closure it hands
`dusk_core::init::init`, which keeps that closure for as long as the namespace's
launcher set is registered; an impl that drops the store instead loses these keys
before `init` runs. Which of them a node has depends
on its impl and on the platform it was built for; the table names the impls in
Dusk's repository that write each one. The impl logs them at `info`: each source
of `dusk.os.*` keys once, as `dusk os`, with a `source` field naming it -
`process`, `time zone`, `locale`, `uname`, `credentials`, `resource limits`,
`os-release`, `boot id`, `pid 1`, `glibc`, `Android system properties`, `sysctl`,
`windows version`, `windows emulation`, `computer name`, `windows session` - and
a `values` field listing the keys and values it wrote; and the `dusk.device.*`
keys together once, as `dusk device`, with a `values` field. A source that fails
writes no keys, and the node logs why at `warn`. One that this system simply
does not have - no os-release file, no device id, process 1 hidden from the
node, a device that reports no vendor, model or CPU, a Windows or Android too
old to carry a value - writes no keys either, and is logged at `info`.
[Collected information](../telemetry/collected-information.md) lists what these
keys hold, platform by platform.

| Key | Written by | Value |
|-----|------------|-------|
| `dusk.impl` | `nix`, `std`, `windows` | The name of the impl the node was built with, e.g. `nix`, `std` or `windows`. |
| `dusk.os.process.pid` | `nix`, `std`, `windows` | The process id the operating system gave the node. |
| `dusk.os.process.executable` | `nix`, `std`, `windows` | The path of the executable the node runs from. |
| `dusk.os.process.working_directory` | `nix`, `std`, `windows` | The directory the node was started in. |
| `dusk.os.process.parent_pid` | `nix`; `std` on Unix; `windows` | The process id of the process that started the node - e.g. `1` when init or systemd runs it. |
| `dusk.os.time_zone` | `nix`; `std` except on Windows; `windows` | The system's time zone: its IANA name, e.g. `Europe/Berlin`, or on Windows the Windows time zone name, e.g. `Pacific Standard Time`. |
| `dusk.os.locale` | `nix`, `std`, `windows` | The locale the node runs with. With `std`, and with `nix` everywhere but macOS and iOS, the value of the first of the environment variables `LC_ALL`, `LC_MESSAGES` and `LANG` that is set and not empty, e.g. `en_US.UTF-8`; a node that has none of them has no key. With `nix` on macOS and iOS, the identifier of the user's current locale, e.g. `en_US`. With `windows`, the user's default locale name, e.g. `en-US`. |
| `dusk.os.nix.uname.sysname` | `nix` | The `sysname` field of `uname(2)`, e.g. `Linux`. |
| `dusk.os.nix.uname.nodename` | `nix` | The `nodename` field of `uname(2)`: the name of the device on the network. |
| `dusk.os.nix.uname.release` | `nix` | The `release` field of `uname(2)`: the kernel release. |
| `dusk.os.nix.uname.version` | `nix` | The `version` field of `uname(2)`: the kernel version. |
| `dusk.os.nix.uname.machine` | `nix` | The `machine` field of `uname(2)`: the hardware the kernel reports. |
| `dusk.os.nix.uname.domainname` | `nix`, on Linux and Android | The `domainname` field of `uname(2)`. |
| `dusk.os.nix.uid` | `nix` | The real UID the node runs as, from `getuid(2)`; `0` is root. |
| `dusk.os.nix.euid` | `nix` | The effective UID the node runs as, from `geteuid(2)`: the one its permissions are checked against. |
| `dusk.os.nix.limits.open_files.soft` | `nix` | How many files the node may have open at once (`RLIMIT_NOFILE`), or `"unlimited"`. A node past it fails to accept connections with "too many open files". |
| `dusk.os.nix.limits.open_files.hard` | `nix` | The most the soft limit could be raised to, or `"unlimited"`. |
| `dusk.os.nix.limits.core_file_size.soft` | `nix` | The largest core dump a crash of the node leaves, in bytes (`RLIMIT_CORE`), or `"unlimited"`. `0` means a crash leaves none. |
| `dusk.os.nix.limits.core_file_size.hard` | `nix` | The most the soft limit could be raised to, or `"unlimited"`. |
| `dusk.os.linux.os_release.<key>` | `nix`, on Linux | Seven entries of [os-release](https://www.freedesktop.org/software/systemd/man/latest/os-release.html) - `name`, `pretty_name`, `id`, `id_like`, `version`, `version_id` and `version_codename` - each `<key>` the entry's name in lower case and the value without its quotes, e.g. `dusk.os.linux.os_release.id` is `ubuntu`, `dusk.os.linux.os_release.version_id` is `24.04`. The file is `/etc/os-release`, or `/usr/lib/os-release` when that does not exist. |
| `dusk.os.linux.boot_id` | `nix`, on Linux | `/proc/sys/kernel/random/boot_id`: a random id the kernel picks at each boot, so it changes when the device reboots and not when the node restarts. |
| `dusk.os.linux.pid1` | `nix`, on Linux | The name of process 1 - `systemd`, `init`, or in a container whatever the container runs first, e.g. `tini`. |
| `dusk.os.linux.glibc_version` | `nix`, on Linux with glibc | The version of the GNU C library the node runs with, e.g. `2.39`. A node built for musl has none. |
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
| `dusk.os.macos.translated` | `nix`, on macOS | Whether the node runs under Rosetta - an `x86_64` build on Apple silicon - from the `sysctl.proc_translated` sysctl; `false` on Intel Macs, which have no Rosetta. |
| `dusk.os.ios.product_version` | `nix`, on iOS | The `kern.osproductversion` sysctl. |
| `dusk.os.ios.build_version` | `nix`, on iOS | The `kern.osversion` sysctl. |
| `dusk.os.windows.major_version` | `windows` | The Windows major version, as a number, e.g. `10`. |
| `dusk.os.windows.minor_version` | `windows` | The Windows minor version, as a number, e.g. `0`. |
| `dusk.os.windows.build_number` | `windows` | The Windows build number, as a number, e.g. `22631`. |
| `dusk.os.windows.revision` | `windows` | The update build revision (`UBR`), as a number: the `4169` in `22631.4169`, which says which cumulative update is installed. |
| `dusk.os.windows.edition` | `windows` | The `EditionID`, e.g. `Professional`, `Core`, `ServerStandard`. |
| `dusk.os.windows.display_version` | `windows` | The `DisplayVersion`, e.g. `23H2`. Windows before 20H2 has none. |
| `dusk.os.windows.native_arch` | `windows` | The device's own architecture, in Rust's names - `x86`, `x86_64`, `arm`, `aarch64` - so it compares with `dusk.target.arch`. |
| `dusk.os.windows.emulated` | `windows` | Whether the node runs emulated: `true` when `dusk.target.arch` is not the device's own architecture, e.g. an `x86_64` build on an ARM64 device or an `x86` build on an `x86_64` one. |
| `dusk.os.windows.computer_name` | `windows` | The computer's name, as Windows reports it (its NetBIOS name). |
| `dusk.os.windows.session_id` | `windows` | The Windows session the node runs in, as a number: `0` when it runs as a service. |
| `dusk.os.windows.elevated` | `windows` | Whether the node runs elevated, with an administrator's rights. |
| `dusk.device.cores` | `nix`, `std`, `windows` | How many CPUs the node may use: the device's logical cores, or fewer when an affinity mask or a container's CPU quota limits it. |
| `dusk.device.memory_bytes` | `nix` on Linux, Android, macOS and iOS; `windows` | The device's total physical memory, in bytes. |
| `dusk.device.swap_bytes` | `nix` on Linux, Android and macOS | The device's total swap space, in bytes. |
| `dusk.device.boot_time_ms` | `nix` on Linux, Android, macOS and iOS; `windows` | When the device last booted, in milliseconds since 1970 (UTC). |
| `dusk.device.vendor` | `nix` on Linux and Android; `windows` | Who made the device, e.g. `Dell Inc.`, `QEMU`, `Amazon EC2`, `Google` - from the firmware (DMI) on Linux, `ro.product.manufacturer` on Android, the BIOS registry key on Windows. |
| `dusk.device.model` | `nix` on Linux, Android, macOS and iOS; `windows` | The device's model, e.g. `MacBookPro18,3`, `iPhone14,2`, `Standard PC (Q35 + ICH9, 2009)` - from the firmware (DMI) or the device tree on Linux, `ro.product.model` on Android, `hw.model` on macOS, `hw.machine` on iOS, the BIOS registry key on Windows. |
| `dusk.device.cpu` | `nix` on Linux, Android and macOS; `windows` | The processor's name, e.g. `AMD Ryzen 7 9700X 8-Core Processor` - from `/proc/cpuinfo` on Linux, `ro.soc.model` on Android 12 and later, `machdep.cpu.brand_string` on macOS, the registry on Windows. |
| `dusk.device.id` | `nix` on Linux, macOS, the BSDs and illumos; `std` on those and Windows; `windows` | An id for the device that stays the same across restarts and reboots: `/var/lib/dbus/machine-id`, or `/etc/machine-id`, on Linux; the hardware UUID on macOS; the `MachineGuid` on Windows; `/etc/hostid` on the BSDs, or where it is missing the SMBIOS system UUID on FreeBSD and DragonFly; the host id on illumos. An empty id is left out, and logged at `warn`. Android and iOS offer native code no such id, so nodes there have none. |

Every client that connects to the node can read `dusk.device.id`.
