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
cargo build --release -p dusk_cli_bin # Build
./target/release/dusk 127.0.0.1:9090 # Connect
```

![A Dusk CLI client connected to a Dusk node](docs/docs/assets/dusk_showcase.png)

## License

Copyright (C) 2023-2026 Tomer Zeitune

Dusk is free software: you can redistribute it and/or modify it under the
terms of the GNU Affero General Public License as published by the Free
Software Foundation, version 3 of the License only.

Dusk is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR
A PARTICULAR PURPOSE. See the GNU Affero General Public License in
[LICENSE](LICENSE) for more details.

Third-party material keeps its own license: the Cap'n Proto compiler under
`vendor/capnproto` (MIT) and the Swagger UI under
`dusk/src/dusk_py/python/dusk/gw/static` (Apache-2.0).
