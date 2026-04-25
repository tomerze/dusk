# Dusk node

Produces a C library you can use to embed a Dusk Node into (almost) any application.

## Usage

### In your code

Include the Dusk C API
```c
#include "dusk.h"
```

Run the Dusk Node (recommended to do this early on application startup)
```c
dusk_node_run(); // This function blocks, make sure to run it on a dedicated thread.
```

### When you build

Link against `libdusk_node.a` and make sure to include `dusk.h`

For example
```bash
gcc main.c -I../dusk/artifacts/dusk_node/include -L../dusk/target/release/ -ldusk_node
```
