This folder holds example deliverable artifacts built with the Dusk framework.

Copy them as templates or use them directly, add your own programs, swap the underlying impl, based on your needs.

- `dusk_node`
  See [Node artifacts](../docs/docs/embedding/node-artifacts.md) for which one to
  pick and how to build it.
- `dusk_cli`: builds the `dusk` CLI binary (from `dusk/src/dusk_cli`) - an
  interactive shell, or a one-shot `dusk <addr> "<command>"`.
- `dusk_py`: builds a Python extension (from `dusk/src/dusk_py`) exposing Dusk's
  Python API.
