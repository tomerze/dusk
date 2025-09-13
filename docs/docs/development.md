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

### Run Dusk Nix

```bash
cargo run --bin dusk_nix
```

### Run Dusk CLI

```bash
cargo run --bin dusk -- 127.0.0.1:8080
```

## Docs

### Build

```bash
uv run mkdocs build
```

## Serve

```bash
uv run mkdocs serve
```
