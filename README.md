# Dusk

## Take it for a spin

Configure, build, and run a standalone Dusk node for linux.
```bash
cmake -S artifacts/dusk_node --preset nix-x64-linux # Configure
make -C artifacts/dusk_node/build/nix-x64-linux dusk_node_bin # Build
./artifacts/dusk_node/build/nix-x64-linux/bin/dusk_node # Run
```

Build a Dusk CLI client for that node, and use it to connect to the running node.
```bash
cargo build --release -p dusk_cli # Build
./target/release/dusk 127.0.0.1:9090 # Connect
```