This folder holds example deliverable artifacts built with the Dusk framework.
Copy them as templates: add your own programs, swap the underlying impl, and ship.

- `dusk_node`: the Dusk **server**. Built from `dusk_base` (the programs) plus the
  `impls/nix` impl, it runs a node listening on port `9090`. Its crate type is
  `rlib` + `staticlib` + `cdylib`, so it can be used
  three ways: as a C library exposing `int32_t dusk_node_run(void)` (see
  `dusk_node/include/dusk.h`), as a Rust rlib (`dusk_node::dusk_node_run()`), or
  as the `dusk_node` binary via `dusk_node_bin`.
- `dusk_node_bin`: wraps `dusk_node` as the runnable `dusk_node` executable.
- `dusk_cli`: builds the `dusk` CLI binary (from `dusk/src/dusk_cli`) — an
  interactive shell, or a one-shot `dusk <addr> "<command>"`.
- `dusk_py`: builds a Python extension (from `dusk/src/dusk_py`) exposing Dusk's
  Python API.
