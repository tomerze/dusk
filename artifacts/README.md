This folder includes example deliverable artifacts using the Dusk framework.

- `dusk_cli`: Uses the `dusk/dusk_cli` crate to create an executable dusk CLI binary.
- `dusk_py`: Uses the `dusk/dusk_py` crate to create a python library which exposes Dusk's Python API
- `dusk_impl`: Uses the `impls/nix` crate to create an executable dusk implementation binary. 
Which listens on tcp port 9090 without encryption for incoming dusk connections. 

All of these examples artifacts are meant to be used as a template for your own dusk
artifacts. Copy their code. Add your programs and or change the underlying impl.
