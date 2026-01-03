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

## Trying things out

Make sure you have `make` `cmake` and `autotools` installed.

### Run example Dusk impl (Dusk Nix impl) which listens on tcp port 9090

```bash
cargo run --bin dusk_impl
```

### Run example Dusk CLI

```bash
cargo run --bin dusk -- 127.0.0.1:9090
```

### Try out Dusk's Python API

```py
uv run maturin develop
uv run python -c "
import dusk
d = dusk.Dusk('127.0.0.1', 9090)
print(list(d.sh('ps')))
"
```

## Using the framework

The basic flow of using the Dusk framework is as follows:

* Write your own dusk programs and or dusk impl using the dusk crates found in `/dusk`
* Copy over the example deliverable artifacts in `/artifacts`
* Make them your own by adding your programs to them and or changing the underlying impl of `dusk_impl`
* Compile and deliver 

## Docs

### Build

```bash
uv run mkdocs build
```

## Serve

```bash
uv run mkdocs serve
```
