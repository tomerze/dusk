#!/bin/bash

# This script is run by nextest before running the tests, it is configured that way in tests/.config/nextest.toml
uv run maturin develop --uv -m artifacts/dusk_py/Cargo.toml
 