#ifndef DUSK_NODE_DUSK_H
#define DUSK_NODE_DUSK_H

#include <stdint.h>

// Runs a Dusk node until it shuts down, and returns its exit code.
//
// `user` is whatever the program running the node gives it at run time.
int32_t dusk_node_run(void *user);

#endif // DUSK_NODE_DUSK_H
