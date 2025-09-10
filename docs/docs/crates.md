# `dusk`

The core dusk crate, used by the `dusk impl` to create `dusk namespaces` and `dusk sessions`.

# `dusk_capnp`

Includes all generic capnp rpc definitions. Can be used by external programs to communicate with a `dusk session`.

# `dusk_cli`

A CLI wrapper around the `dusk_prompt` which connects to a dusk server at a specific `ip` + `port`

# `dusk_prompt`

A library providing a command line prompt, and a shell which connects to a dusk server and runs the `sh` program.

# `dusk_{impl name}`

An OS/Hardware specific implentation of both a `dusk server` and a `dusk driver` used to run `dusk programs`

# `dusk_program`

definitions used to write dusk programs.

# `dusk_program_{program name}`

A dusk program used by a `dusk impl`
