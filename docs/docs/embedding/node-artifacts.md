# Dusk node artifacts

Embed a Dusk node in (almost) any application, as a C static library linked
into your program or as a standalone binary (executable/firmware).

Getting a node running takes four steps:

1. [Pick the impl](#1-pick-the-impl) for your platform.
2. [Build](#2-build) the library (or the standalone binary).
3. [Link](#3-link) the library into your application (standalone binaries don't need this step)
4. [Run](#4-run) the node as part of your application

## 1. Pick the impl

Each node artifact links one Dusk impl, selected by a cargo feature. Your platform
decides which:

| Impl | Cargo feature | Platforms | Artifacts |
|---|---|---|---|
| `nix` | `impl_nix` | Unix-like systems: Linux, Android, macOS, iOS, the BSDs and more | library, executable |
| `windows` | `impl_windows` | Windows | library, executable (`.exe`) |
| `std` | `impl_std` | Other platforms with the Rust standard library and threads: ESP-IDF, WebAssembly, QNX, VxWorks and more | library |

Every supported operating system and its impl is in the
[platform list](#platforms).

## 2. Build

`artifacts/dusk_node` is a CMake project that runs `cargo` for you. It needs
`cargo` with the Rust target installed, and CMake 3.23 or newer. To call
`cargo` yourself instead, see [Building with cargo](#building-with-cargo).

Dusk cannot be built on a windows host natively. To build dusk on a Windows host, use WSL.

To build the windows target `cargo-xwin` is needed.
```
cargo install --locked cargo-xwin
```

### Standalone

```sh
cd artifacts/dusk_node
cmake --list-presets                 # show available presets
cmake --preset nix-x64-linux         # configure, once per build directory
make -C build/nix-x64-linux dusk_node dusk_node_bin
```

Output:

```
build/nix-x64-linux/lib/libdusk_node.a
build/nix-x64-linux/bin/dusk_node
```

Clean:

```sh
make -C build/nix-x64-linux clean    # or: rm -rf build/nix-x64-linux
```

### Inside a CMake project, with presets

Needs CMake 3.23 or newer.

`CMakePresets.json` in your project (the `include` path is relative to this
file, and your preset names must differ from the node's):

```json
{
  "version": 4,
  "include": ["path/to/dusk/artifacts/dusk_node/CMakePresets.json"],
  "configurePresets": [
    { "name": "my-app-linux", "inherits": "nix-x64-linux" }
  ],
  "buildPresets": [
    { "name": "my-app-linux", "configurePreset": "my-app-linux" }
  ]
}
```

`CMakeLists.txt`:

```cmake
add_subdirectory(path/to/dusk/artifacts/dusk_node dusk_node)

# Brings the include path for dusk.h and the system libraries Rust's std needs.
# Native libraries pulled in by an impl's own dependencies are not included.
target_link_libraries(my_app PRIVATE dusk::node)
```

### Variables

The presets set these. To override one, or to build for a target that has no
preset, pass them with `-D`:

```sh
# Override one value of a preset
cmake --preset nix-x64-linux -DDUSK_NODE_CARGO_PROFILE=dev

# Build for a target that has no preset
cmake -S . -B build/aarch64-linux \
  -DDUSK_NODE_IMPL=nix -DDUSK_NODE_CARGO_TARGET=aarch64-unknown-linux-gnu
make -C build/aarch64-linux dusk_node
```

#### DUSK_NODE_IMPL

**Required.** The impl the node links, without the `impl_` prefix: `nix`,
`windows` or `std`. The valid values are the `impl_*` features in
`lib/Cargo.toml`, and configuring with any other value lists them.

#### DUSK_NODE_CARGO_TARGET

**Required**, even when building for the host (`cargo -vV` prints the host's
triple as `host:`). The Rust target triple to build for. It selects that
platform's library naming and system libraries, and it puts the output under
`cargo/<triple>/` in the node's build directory.

#### DUSK_NODE_CARGO_PROFILE

The cargo profile, empty to follow `CMAKE_BUILD_TYPE`: `Debug` takes cargo's
`dev`, `MinSizeRel` takes `prod`, and anything else takes `release`. You can
set it to name a profile directly.

#### DUSK_NODE_RUSTFLAGS

Extra flags for `rustc`, separated by spaces as in cargo's `RUSTFLAGS`, e.g.
`-DDUSK_NODE_RUSTFLAGS="-C target-cpu=native"`. Empty by default. A flag that
contains a space can be quoted: CMake splits the value by Windows command-line
rules on a Windows host, and by Unix shell rules elsewhere, except that a
backslash escapes the next character inside single quotes too.

They are passed after the flags in the repository's `.cargo/config.toml`, and
only to the crates built for `DUSK_NODE_CARGO_TARGET` - not to build scripts or
proc macros.

Cargo takes its flags from the first source it finds, so either of these
replaces both this variable and `.cargo/config.toml`:

- `CARGO_ENCODED_RUSTFLAGS` or `RUSTFLAGS` in the environment the build runs in.
- `target.<triple>.rustflags` or `target.<cfg>.rustflags` in any cargo config
  file, such as `~/.cargo/config.toml`.

#### DUSK_NODE_INIT_SCRIPT

The shell script the node runs when it starts, e.g.
`-DDUSK_NODE_INIT_SCRIPT="nightfall -l 127.0.0.1:9091"`. The presets set it to
`nightfall -l 9090`, which listens on port 9090 on every address. Empty leaves
it to cargo, as in [Building with cargo](#building-with-cargo).

The script is compiled into the node while it is built: a script that does not
compile fails the build, and changing the variable rebuilds the node. A script of
several commands separates them with `;`.

> **Side note:** if you set these in your CMakeLists.txt instead of a preset,
> make them cache variables, e.g. `set(DUSK_NODE_IMPL nix CACHE STRING "")`.
> A plain `set()` can get quietly dropped on the very first configure, then
> mysteriously work on the second run.

### Building with cargo

From anywhere in the repository, for the library:

```sh
# nix
cargo build --profile prod --target <target> -p dusk_node \
  --no-default-features --features impl_nix

# windows
cargo xwin build --profile prod --target <target> -p dusk_node \
  --no-default-features --features impl_windows

# std
rustup component add rust-src
cargo build --profile prod -Z build-std=std --target <target> -p dusk_node \
  --no-default-features --features impl_std
```

The library lands in `target/<target>/prod/`. For the executable (nix and
windows), use `-p dusk_node_bin` instead of `-p dusk_node`.

- `-Z build-std` needs a nightly toolchain.
- Targets that `rustup target add` can't install need `-Z build-std=std` with
  any impl, the same way as the std command.
- Xtensa targets need the esp-rs fork of the compiler (`espup install`) and
  `cargo +esp`; the command is otherwise the same.

The init script is the `DUSK_NODE_INIT_SCRIPT` environment variable. The
repository's `.cargo/config.toml` sets it to `nightfall -l 9090`, and a value in
the environment the build runs in takes its place. Cargo reads that file only for
a build run inside the repository, so a build from another workspace - a Rust
application that depends on `dusk_node` - sets the variable itself, in its
environment or in the `[env]` table of its own `.cargo/config.toml`:

```sh
DUSK_NODE_INIT_SCRIPT="nightfall -l 127.0.0.1:9091" cargo build --profile prod \
  --target <target> -p dusk_node --no-default-features --features impl_nix
```

A build without a `.git` directory - a Docker build whose context leaves it
out - cannot ask git for the revision it is built from, which every program
records. Pass it in `DUSK_GIT_REV`, at least 16 hex digits, taken from a
checkout that has one. In the stage of the `Dockerfile` that runs the build,
take it as a build argument and set the variable from it:

```dockerfile
ARG GIT_REV
ENV DUSK_GIT_REV=$GIT_REV
RUN cargo build --profile prod --target <target> -p dusk_node_bin
```

and pass the argument from the checkout:

```sh
docker build --build-arg GIT_REV=$(git rev-parse HEAD) .
```

When `DUSK_GIT_REV` is set the build does not run git at all; when it is not set
and git cannot answer, the build stops with an error that names `DUSK_GIT_REV`.

### The fleet token

Every node carries a **fleet token**: a secret compiled into `dusk_core`, the
same in every node of one build. It is how a node proves it was built by whoever
runs the fleet the first time it enrolls in one. A program on the node reads it
with `dusk_core::fleet_token::fleet_token()`, and a client holding the node's
`Dusk` capability with
[`Dusk.fleetToken`](../sdk-reference/capnp-schemas.md#duskcapnp). Nothing in
the node reads it on its own.

Set it as `DUSK_FLEET_TOKEN` in the environment the build runs in:

```sh
DUSK_FLEET_TOKEN="$(cat fleet-token)" cargo build --profile prod \
  --target <target> -p dusk_node_bin
```

A CMake build hands its environment to cargo, so the variable in the
environment of `cmake --build` reaches it the same way. It is not a CMake
variable, because a cache variable would leave the secret in `CMakeCache.txt`.

A Docker build passes it as a build secret, never a build argument: the image's
history keeps the build arguments a `RUN` used. Mount the secret as the
variable in the `RUN` that builds the node:

```dockerfile
RUN --mount=type=secret,id=fleet-token,env=DUSK_FLEET_TOKEN,required=true \
    cargo build --profile prod --target <target> -p dusk_node_bin
```

and pass it from a file on the build host:

```sh
docker build --secret id=fleet-token,src=fleet-token .
```

The `env` option of a secret mount needs Dockerfile syntax 1.10.0 or newer -
start the `Dockerfile` with `# syntax=docker/dockerfile:1.10` on a BuildKit
whose built-in frontend is older. `required=true` fails the build when the
secret is missing, where it would otherwise build nodes with a made-up token.

Changing `DUSK_FLEET_TOKEN` rebuilds `dusk_core` and everything that links it.
An empty value, or one that is not UTF-8, fails the build. The build never
prints the token.

When `DUSK_FLEET_TOKEN` is not set, the build makes one up: 32 random bytes,
written as 64 hex digits, kept as `random_fleet_token` in `dusk_core`'s build
output directory, `<target dir>/<target>/<profile directory>/build/dusk_core-<hash>/out/`.
The target directory is `target/` for cargo and `<CMake build directory>/cargo/`
for CMake; the profile directory is the profile's name, except `debug` for
`dev`; `<target>/` is left out of a cargo build without `--target`. Every later
build that cargo puts in that directory reuses the token, and setting the
variable and later unsetting it brings the same one back. A build that cargo
puts in another directory makes another token: after `cargo clean`, in another
target directory, for another profile, target or set of `dusk_core` features,
and after a change of toolchain or of a crate `dusk_core` depends on. Set
`DUSK_FLEET_TOKEN` for any fleet whose nodes are built more than once.

**One secret in every node.** Every node of a build holds the same token, and
it is easy to take from any of them: it is in the binary, and every client that
connects to a node can ask for it with `Dusk.fleetToken`. Whoever extracts it
once can enroll as many nodes as they like, as if they were yours. Keep a node's
port off networks you do not trust. Building with a new token gives the nodes
built from then on a new one; it does not stop the leaked token working, and the
nodes already shipped still carry it. Only whoever accepts enrollments can stop
accepting it.

## 3. Link

If you link `dusk::node` from CMake, skip this step: the target brings the
include path and the system libraries along.

Otherwise, pass the include directory, the library, and the system libraries
Rust's standard library needs. On Linux with glibc:

```sh
gcc main.c \
  -D DUSK_PTHREAD \
  -I path/to/dusk/dusk/include \
  -L path/to/lib/dir \
  -ldusk_node -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc
```

`path/to/lib/dir` is the directory holding `libdusk_node.a`:
`artifacts/dusk_node/build/<preset>/cargo/<target>/<profile>/` after a CMake
build, `target/<target>/prod/` after a cargo build.

For other targets, cargo prints the system libraries to use. Take your cargo
build command, make it `cargo rustc --lib`, and append
`-- --print native-static-libs`:

```sh
cargo rustc --lib --profile prod --target <target> -p dusk_node \
  --no-default-features --features impl_nix -- --print native-static-libs
```

The list is on the `native-static-libs:` line of the output.

## 4. Run

### Library

`dusk_spawn` starts a namespace on a thread of its own, so call it early in your
application's startup:

```c
#include "dusk.h"

int main(void)
{
    uint64_t handle = dusk_spawn(NULL, NULL);

    /* ... the rest of your application ... */
}
```

To run it on a thread you choose, call `dusk_run(dusk_new(), NULL)` there
instead. `dusk.h` documents every call.

### Executable

Available with the nix and windows impls. It takes no arguments and, built with
the default init script, listens on port 9090, on every address:

```sh
./dusk_node
```

The address is built in: the init script, `nightfall -l 9090` by default, is
compiled into the artifact by `compile_sh!` while it is built. To listen somewhere
else, set [`DUSK_NODE_INIT_SCRIPT`](#dusk_node_init_script) and rebuild. The macro
resolves the script's commands against the sh entries linked into
`dusk_program_sh_compiler_proc`, which are the
Base programs from `dusk_base`: to name a program of your own there, add its crate
to `base/sh/compiler/proc/Cargo.toml` and reference its `sh_entry` beside the
`dusk_base::link_anchors()` call in `base/sh/compiler/proc/src/lib.rs`. On Linux, two nodes whose init scripts
listen on the same port cannot run on one machine unless each listens on an
address of its own rather than on every address - the second fails to bind.

## Reference

### Platforms

| Operating system | Impl |
|---|---|
| Linux (glibc, musl, uClibc, Yocto) | `nix` |
| Android | `nix` |
| OpenHarmony | `nix` |
| macOS | `nix` |
| iOS (incl. Mac Catalyst) | `nix` |
| watchOS | `nix` |
| tvOS | `nix` |
| visionOS | `nix` |
| FreeBSD | `nix` |
| OpenBSD | `nix` |
| NetBSD | `nix` |
| DragonFly BSD | `nix` |
| Solaris | `nix` |
| illumos | `nix` |
| Fuchsia | `nix` |
| Haiku | `nix` |
| Redox | `nix` |
| GNU/Hurd | `nix` |
| Cygwin | `nix` |
| Unikraft | `nix` |
| Windows (MSVC, MinGW, gnullvm, UWP, Win7) | `windows` |
| WebAssembly (WASI, Emscripten, no-OS) | `std` |
| ESP-IDF | `std` |
| QNX | `std` |
| VxWorks | `std` |
| Trusty | `std` |
| Intel SGX | `std` |
| NuttX | `std` |
| L4Re | `std` |
| SOLID | `std` |
| Hermit | `std` |
| HelenOS | `std` |
| Motor OS | `std` |
| PlayStation Vita | `std` |
| VEXos | `std` |

### nix targets

| Target |
|---|
| **Linux** |
| aarch64-unknown-linux-gnu |
| arm-unknown-linux-gnueabi |
| arm-unknown-linux-musleabi |
| armv7-unknown-linux-gnueabihf |
| armv7-unknown-linux-uclibceabihf |
| i686-unknown-linux-gnu |
| i686-unknown-linux-musl |
| loongarch64-unknown-linux-gnu |
| mips-unknown-linux-gnu |
| mips64-unknown-linux-gnuabi64 |
| mips64el-unknown-linux-gnuabi64 |
| mipsel-unknown-linux-gnu |
| powerpc64-unknown-linux-gnu |
| powerpc64le-unknown-linux-gnu |
| s390x-unknown-linux-gnu |
| x86_64-unknown-linux-gnu |
| x86_64-unknown-linux-gnux32 |
| x86_64-unknown-linux-musl |
| **Android** |
| aarch64-linux-android |
| arm-linux-androideabi |
| armv7-linux-androideabi |
| i686-linux-android |
| x86_64-linux-android |
| **OpenHarmony** |
| aarch64-unknown-linux-ohos |
| armv7-unknown-linux-ohos |
| x86_64-unknown-linux-ohos |
| **Apple** |
| aarch64-apple-darwin |
| aarch64-apple-ios |
| **BSD** |
| i686-unknown-freebsd |
| x86_64-unknown-freebsd |
| x86_64-unknown-netbsd |
| x86_64-unknown-openbsd |
| x86_64-unknown-dragonfly |
| **illumos, Fuchsia, Haiku, Redox, GNU/Hurd** |
| x86_64-unknown-illumos |
| x86_64-unknown-fuchsia |
| x86_64-unknown-haiku |
| x86_64-unknown-redox |
| i686-unknown-hurd-gnu |

### windows targets

| Target |
|---|
| **MSVC** |
| aarch64-pc-windows-msvc |
| i686-pc-windows-msvc |
| x86_64-pc-windows-msvc |
| arm64ec-pc-windows-msvc |
| **MinGW** |
| i686-pc-windows-gnu |
| x86_64-pc-windows-gnu |
| **gnullvm** |
| aarch64-pc-windows-gnullvm |
| i686-pc-windows-gnullvm |
| x86_64-pc-windows-gnullvm |
| **UWP** |
| aarch64-uwp-windows-msvc |
| i686-uwp-windows-msvc |
| x86_64-uwp-windows-msvc |
| i686-uwp-windows-gnu |
| x86_64-uwp-windows-gnu |
| **Windows 7** |
| i686-win7-windows-msvc |
| x86_64-win7-windows-msvc |
| i686-win7-windows-gnu |
| x86_64-win7-windows-gnu |

### std targets

| Target |
|---|
| **WebAssembly** |
| wasm32-unknown-emscripten |
| wasm32-unknown-unknown |
| wasm32-wasip1 |
| wasm32-wasip1-threads |
| wasm32-wasip2 |
| wasm32-wasip3 |
| **ESP-IDF** |
| riscv32imac-esp-espidf |
| riscv32imafc-esp-espidf |
| riscv32imc-esp-espidf |
| xtensa-esp32-espidf |
| xtensa-esp32s2-espidf |
| xtensa-esp32s3-espidf |
| **QNX** |
| aarch64-unknown-nto-qnx710 |
| aarch64-unknown-nto-qnx710_iosock |
| aarch64-unknown-qnx |
| x86_64-pc-nto-qnx710 |
| x86_64-pc-nto-qnx710_iosock |
| x86_64-pc-qnx |
| **VxWorks** |
| aarch64-wrs-vxworks |
| armv7-wrs-vxworks-eabihf |
| i686-wrs-vxworks |
| powerpc-wrs-vxworks |
| powerpc-wrs-vxworks-spe |
| powerpc64-wrs-vxworks |
| riscv32-wrs-vxworks |
| riscv64-wrs-vxworks |
| x86_64-wrs-vxworks |
| **Trusty** |
| aarch64-unknown-trusty |
| armv7-unknown-trusty |
| x86_64-unknown-trusty |
| **Intel SGX** |
| x86_64-fortanix-unknown-sgx |
| **NuttX** |
| aarch64-unknown-nuttx |
| armv7a-nuttx-eabi |
| armv7a-nuttx-eabihf |
| riscv32imac-unknown-nuttx-elf |
| riscv32imafc-unknown-nuttx-elf |
| riscv32imc-unknown-nuttx-elf |
| riscv64gc-unknown-nuttx-elf |
| riscv64imac-unknown-nuttx-elf |
| thumbv6m-nuttx-eabi |
| thumbv7a-nuttx-eabi |
| thumbv7a-nuttx-eabihf |
| thumbv7em-nuttx-eabi |
| thumbv7em-nuttx-eabihf |
| thumbv7m-nuttx-eabi |
| thumbv8m.base-nuttx-eabi |
| thumbv8m.main-nuttx-eabi |
| thumbv8m.main-nuttx-eabihf |
| **L4Re** |
| aarch64-unknown-l4re-uclibc |
| x86_64-unknown-l4re-uclibc |
| **SOLID** |
| aarch64-kmc-solid_asp3 |
| armv7a-kmc-solid_asp3-eabi |
| armv7a-kmc-solid_asp3-eabihf |
| **Hermit** |
| aarch64-unknown-hermit |
| aarch64_be-unknown-hermit |
| riscv64gc-unknown-hermit |
| x86_64-unknown-hermit |
| **HelenOS** |
| aarch64-unknown-helenos |
| i686-unknown-helenos |
| powerpc-unknown-helenos |
| sparc64-unknown-helenos |
| x86_64-unknown-helenos |
| **Motor OS** |
| x86_64-unknown-motor |
| **PlayStation Vita** |
| armv7-sony-vita-newlibeabihf |
| **VEXos** |
| thumbv7a-vex-v5 |

### Cross-compiling to Windows

Build the Windows artifacts from a Unix host. 

Linking needs an MSVC linker and the Microsoft CRT and Windows SDK import
libraries, which a standard Unix host does not have. So install cargo xwin

