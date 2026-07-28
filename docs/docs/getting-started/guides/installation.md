# Installation

Get the toolchain and dependencies in place to build Dusk and run a node.

## Prerequisites

Dusk builds a vendored Cap'n Proto compiler from source, so you need a C/C++
toolchain alongside Rust:

- **Rust** — the repository pins its toolchain. Running `rustup show` from the
  project root installs the correct version automatically.
- **`make`, `cmake`, `autotools`** — required to build the vendored Cap'n Proto
  compiler under `vendor/`.
- **A network connection for the first build** — `dusk_llm` downloads the
  ≈2.9 GB GGUF model named in `dusk/src/dusk_llm/models/models.toml` into that
  directory. It is kept there and verified against the manifest's SHA-256 on
  every build, so builds after the first need no network.
- **[`uv`](https://docs.astral.sh/uv/)** — used for the Python tooling
  (pre-commit, the `dusk_py` extension, the docs).

## Clone and build

```bash
git clone https://github.com/tomerze/dusk
cd dusk
rustup show        # installs the pinned toolchain
cargo build        # first build also compiles the vendored Cap'n Proto source
```

## Pre-commit hooks

```bash
uv run pre-commit install
```

Once installed, the hooks run on every commit. With the toolchain and build deps
in place, head to the [Quickstart](quickstart.md).
