# Dusk node artifacts

You can use these artifcats to embed a Dusk Node into (almost) any application.

## How to build

There is one node artifact, and which impl it links is a cargo feature. Follow
the `build` docs section of the impl relevant for you

For *Nix platforms (Linux, Dawrin, FreeBSD) use artifacts/dusk_node/lib (lib) or artifacts/dusk_node/bin (An executable ELF), with `impl_nix`

For Windows use the same two, with `impl_windows` (lib and .exe)

For other platforms which support the Rust standard library (ESP-IDF for example) use artifcats/dusk_node/lib (lib), with `impl_std`


| Operating system | Impl feature |
|---|---|
| Linux (glibc, musl, uClibc, Yocto) | impl_nix |
| Cygwin | impl_nix |
| Android | impl_nix |
| iOS (incl. Mac Catalyst) | impl_nix |
| macOS | impl_nix |
| OpenHarmony | impl_nix |
| FreeBSD | impl_nix |
| watchOS | impl_nix |
| tvOS | impl_nix |
| Solaris | impl_nix |
| illumos | impl_nix |
| OpenBSD | impl_nix |
| NetBSD | impl_nix |
| Fuchsia | impl_nix |
| visionOS | impl_nix |
| Haiku | impl_nix |
| DragonFly BSD | impl_nix |
| Redox | impl_nix |
| GNU/Hurd | impl_nix |
| Unikraft | impl_nix |
| Windows (MSVC, MinGW, gnullvm, UWP, Win7) | impl_windows |
| WebAssembly (WASI, Emscripten, no-OS) | impl_std |
| ESP-IDF | impl_std |
| QNX | impl_std |
| VxWorks | impl_std |
| Trusty | impl_std |
| Intel SGX | impl_std |
| NuttX | impl_std |
| L4Re | impl_std |
| SOLID | impl_std |
| Hermit | impl_std |
| HelenOS | impl_std |
| Motor OS | impl_std |
| PlayStation Vita | impl_std |
| VEXos | impl_std |

## How to link

Link against `libdusk_node.a` and make sure to include `dusk.h`

For example
```bash
gcc main.c -I../dusk/artifacts/dusk_node/lib/include -L../dusk/target/release/ -ldusk_node
```

## How to run

### Library

In your code:

Include the Dusk C API
```c
#include "dusk.h"
```

Run the Dusk Node (recommended to do this early on application startup)
```c
dusk_node_run(NULL); // This function blocks, make sure to run it on a dedicated thread.
```

#### Executable

Some Dusk node executables require some trivial command-line arguments to run. 

Run Dusk node executables just like any other executable on your platform.

artifacts/dusk_node/bin produces the `dusk_node` executable: the node artifact
wrapped in a `main` that passes its one optional argument through. An executable
ELF file on *Nix, a PE file on Windows.

## Nix

A Dusk node artifcat for *Nix platforms (Linux, Dawrin, FreeBSD)

Builds as a C static library.

### Build

```bash
cargo build --profile prod --target <target> -p dusk_node
```

### Possible targets

| Target |
|---|
| **nix Tier 1: build and tests gate merges** |
| aarch64-apple-darwin |
| aarch64-unknown-linux-gnu |
| arm-unknown-linux-gnueabi |
| armv7-unknown-linux-gnueabihf |
| i686-unknown-freebsd |
| i686-unknown-linux-gnu |
| i686-unknown-linux-musl |
| mips-unknown-linux-gnu |
| mips64-unknown-linux-gnuabi64 |
| mips64el-unknown-linux-gnuabi64 |
| mipsel-unknown-linux-gnu |
| powerpc64le-unknown-linux-gnu |
| x86_64-unknown-freebsd |
| x86_64-unknown-linux-gnu |
| x86_64-unknown-linux-musl |
| **nix Tier 2: build gates merges, tests optional** |
| aarch64-apple-ios |
| aarch64-linux-android |
| aarch64-unknown-linux-ohos |
| arm-linux-androideabi |
| arm-unknown-linux-musleabi |
| armv7-linux-androideabi |
| armv7-unknown-linux-ohos |
| i686-linux-android |
| loongarch64-unknown-linux-gnu |
| s390x-unknown-linux-gnu |
| x86_64-linux-android |
| x86_64-unknown-illumos |
| x86_64-unknown-linux-ohos |
| x86_64-unknown-netbsd |
| **nix Tier 3: built in CI, may be dropped if it blocks work** |
| armv7-unknown-linux-uclibceabihf |
| i686-unknown-hurd-gnu |
| powerpc64-unknown-linux-gnu |
| x86_64-unknown-dragonfly |
| x86_64-unknown-fuchsia |
| x86_64-unknown-haiku |
| x86_64-unknown-linux-gnux32 |
| x86_64-unknown-openbsd |
| x86_64-unknown-redox |

## Windows

A Dusk node artifcat for Windows

Builds as a C static library.

### Build

```bash
cargo build --profile prod --target <target> -p dusk_node \
  --no-default-features --features impl_windows
```

### Possible targets

| Target |
|---|
| aarch64-pc-windows-msvc |
| i686-pc-windows-msvc |
| x86_64-pc-windows-msvc |
| arm64ec-pc-windows-msvc |
| i686-pc-windows-gnu |
| x86_64-pc-windows-gnu |
| aarch64-pc-windows-gnullvm |
| i686-pc-windows-gnullvm |
| x86_64-pc-windows-gnullvm |
| aarch64-uwp-windows-msvc |
| i686-uwp-windows-msvc |
| x86_64-uwp-windows-msvc |
| i686-uwp-windows-gnu |
| x86_64-uwp-windows-gnu |
| i686-win7-windows-msvc |
| x86_64-win7-windows-msvc |
| i686-win7-windows-gnu |
| x86_64-win7-windows-gnu |

## Std

A Dusk node artifcat for every platform that has support for the Rust standard library and threads.

Builds as a C static library.

### Build

```bash
rustup component add rust-src

cargo build --profile prod -Z build-std=std,panic_abort \
  --target <target> -p dusk_node --no-default-features --features impl_std
```

Xtensa needs the esp-rs forked compiler (`espup install`) and `cargo +esp`,
otherwise the same command with `--target xtensa-esp32-espidf`.

# Possible targets

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

