# Development

## Setup

### Install Rust

This will automatically install the correct version of rust
```bash
rustup show
```

### Install pre-commit hooks

```bash
uv run pre-commit install
```

### Install `nextest` 

```bash
cargo install cargo-nextest --locked
```

## Tests

Run the tests
```
cargo nextest run
```

## Trying things out

Make sure you have `make` `cmake` and `autotools` installed. The first build also
downloads the ≈2.9 GB [Ask Dusk](ask_dusk.md) model, so it needs a network
connection; later builds reuse the downloaded file.

### Run a node

This starts a `dusk_node` server, which listens on tcp port `9090` (it uses the
Dusk Nix impl by default).

```bash
cargo run --bin dusk_node
```

### Run example Dusk CLI

```bash
cargo run --bin dusk -- 127.0.0.1:9090
```

### Try out Dusk's Python API

```bash
uv run maturin develop
uv run python
```

```py
import dusk
d = dusk.Dusk('127.0.0.1', 9090)
# get the currently running processes using the `ps` program and print them.
print(list(d.sh('ps')))
```

## Using the framework

The basic flow of using the Dusk framework is as follows:

* Write your own dusk programs and or dusk impl using the dusk crates found in `/dusk`
* Copy over the example deliverable artifacts in `/artifacts`
* Make them your own by adding your programs to them and or changing the underlying impl that `dusk_node` links
* Compile and deliver 

## Docs

### Build

```bash
cd docs && uv run mkdocs build
```

### Serve

```bash
cd docs && uv run mkdocs serve
```
