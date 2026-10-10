# Collected information

When a Dusk node starts, it records a few facts about itself, the operating
system it runs on and the device under it, so that whoever looks after the node
can tell what it is and where it runs. It gathers them once, at startup, keeps
them in the node's [key-value store](../features/kvs.md) and writes them to the
node's [logs](../features/logs.md).

Dusk sends none of this anywhere unless it is told to. It leaves the node in
three ways: a client that connects to the node can read it, from the key-value
store or from the logs; `logs stream` sends the node's logs to a file or a
collector; and a node built to
[connect to nightfall](../getting-started/guides/connect-to-nightfall.md) sends
nightfall a report every time it enrolls - the SHA-256 of the device's id, the
device's network name, Dusk's version, the impl, and the operating system and
processor architecture the node was built for. On a machine whose TPM attests
the node's key, the report carries the TPM's endorsement key, its certificate
and the certificates of its issuers, and the public half of the node's key, and
the SHA-256 of that endorsement key takes the place of the device's id. None of it
is kept across a restart; the node gathers it again each time it starts.

Some of these facts can identify more than the node: the device's id, which
stays the same for as long as the operating system is installed; a TPM's
endorsement key and its certificate, which stay the same for as long as the TPM
is in the machine; the device's
network name, or on Windows its computer name, which on many computers includes
its owner's name; and the path of the program the node runs from and of the
directory it was started in, which can include the name of an account's home
folder. Anyone who can connect to the node can read them.

What a node records depends on the platform it runs on and on the
[impl](../getting-started/concepts/drivers-and-impls.md) it was built with - the
part of Dusk that runs it on that platform. The sections below cover the impls
in Dusk's repository: `nix` on Linux, Android, macOS, iOS, the BSDs and illumos,
`windows` on Windows, and `std` on any platform.
[Key-value store](../features/kvs.md#what-the-impl-records) lists the key each
fact is kept under.

## On all platforms

Nodes built with the `nix`, `std` and `windows` impls all record:

| Information | Where it comes from |
|-------------|---------------------|
| Dusk's version, and the revision it was built from | The node's own build |
| The processor architecture and operating system the node was built for, and whether it is a 32- or 64-bit build | The node's own build |
| Which impl the node was built with, e.g. `nix`, `std` or `windows` | The node's own build |
| The process id the operating system gave the node, the path of the program file it runs from, the directory it was started in, and the process id of the process that started it | The operating system |
| The system's time zone | The operating system |
| How many processor cores the node may use | The operating system |

## Linux

| Information | Where it comes from |
|-------------|---------------------|
| The kernel's name, release and version, the device's network name, its hardware type and its domain name | The kernel |
| Which distribution and version this is - its name, version and codename, and the distributions it is based on | The distribution's release file, `/etc/os-release`, or `/usr/lib/os-release` |
| An id that changes every time the device boots | The kernel |
| The account ids the node runs as | The operating system |
| How many files the node may keep open at once, and how large a crash dump of the node may be | The limits the operating system sets for the node |
| What process 1 is - `systemd`, `init`, or in a container whatever the container runs first | The kernel, unless it hides process 1 from the node |
| The version of the C library the node runs with, on systems with glibc | The C library |
| Who made the device and its model | The device's firmware (DMI), or for the model on boards without it, the device tree |
| The processor's name | The kernel's CPU information, `/proc/cpuinfo` |
| When the device last booted | The kernel |
| The device's total memory and swap | The kernel |
| The device's id | The system's machine id file, `/var/lib/dbus/machine-id` or `/etc/machine-id` |

## Android

| Information | Where it comes from |
|-------------|---------------------|
| The Android version, API level, security patch level and build number | The device's system properties |
| The device's model, manufacturer and brand, the fingerprint of the exact build it runs, the kind of build it is, and the processor ABIs it runs | The device's system properties |
| The processor's name, on Android 12 and later | The device's system properties |
| The kernel's name, release and version, the device's network name, its hardware type and its domain name | The kernel |
| The account ids the node runs as | The operating system |
| How many files the node may keep open at once, and how large a crash dump of the node may be | The limits the operating system sets for the node |
| When the device last booted | The kernel |
| The device's total memory and swap | The kernel |

Android gives native programs no id for the device, so an Android node records
none.

## macOS

| Information | Where it comes from |
|-------------|---------------------|
| The macOS version and build | The system |
| Whether the node runs translated by Rosetta - an Intel build on an Apple silicon Mac | The system |
| The Mac's model, e.g. `MacBookPro18,3`, and its processor's name | The system |
| When the Mac last booted | The system |
| The kernel's name, release and version, the Mac's network name and its hardware type | The kernel |
| The account ids the node runs as | The operating system |
| How many files the node may keep open at once, and how large a crash dump of the node may be | The limits the operating system sets for the node |
| The Mac's total memory and swap | The system |
| The device's id | The Mac's hardware UUID |

## iOS

| Information | Where it comes from |
|-------------|---------------------|
| The iOS version and build | The system |
| The device's model, e.g. `iPhone14,2` | The system |
| When the device last booted | The system |
| The kernel's name, release and version, the device's network name and its hardware type | The kernel |
| The account ids the node runs as | The operating system |
| How many files the node may keep open at once, and how large a crash dump of the node may be | The limits the operating system sets for the node |
| The device's total memory | The system |

iOS gives native programs no id for the device, so an iOS node records none.

## The BSDs and illumos

| Information | Where it comes from |
|-------------|---------------------|
| The kernel's name, release and version, the device's network name and its hardware type | The kernel |
| The account ids the node runs as | The operating system |
| How many files the node may keep open at once, and how large a crash dump of the node may be | The limits the operating system sets for the node |
| The device's id | `/etc/hostid` on FreeBSD, DragonFly, OpenBSD and NetBSD, or where it is missing the device's SMBIOS system UUID on FreeBSD and DragonFly; the host id on illumos |

A node on these platforms does not record the device's total memory.

## Windows

| Information | Where it comes from |
|-------------|---------------------|
| The Windows version - major and minor version, build number and update revision - and its display version, e.g. `23H2` | The system, and the Windows registry for the update revision and the display version |
| The Windows edition, e.g. `Professional` | The Windows registry |
| The computer's name | The system |
| The session the node runs in - whether it runs as a service - and whether it runs with an administrator's rights | The system |
| The device's own processor architecture, and whether the node runs emulated on it | The system |
| Who made the device, its model, and its processor's name | The Windows registry, as the firmware reports them |
| When the device last booted | The system |
| The device's total memory | The system |
| The device's id | The Windows registry, `MachineGuid` |

## Nodes built with the `std` impl

A node built with the portable `std` impl, on any platform, records what
[all platforms](#on-all-platforms) record - except, on Windows, the time zone
and the process id of the process that started it - and the device's id on
Linux, macOS, Windows, the BSDs and illumos, from the same place as above.
