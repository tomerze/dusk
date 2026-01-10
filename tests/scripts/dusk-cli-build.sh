#!/usr/bin/env bash

# This script is run by nextest before running the tests, it is configured that way in tests/.config/nextest.toml
cargo build --manifest-path artifacts/dusk_cli/Cargo.toml
