#ifndef DUSK_NODE_DUSK_H
#define DUSK_NODE_DUSK_H

#include <stdint.h>

// Runs a Dusk node until it shuts down, and returns its exit code.
//
// `user` is whatever the program running the node gives it at run time.
// dusk_node is a template, and the node you build from it decides what `user`
// means: this one reads it as a NUL-terminated "ip:port" to listen on, and
// listens on 0.0.0.0:9090 when it is NULL.
int32_t dusk_node_run(void *user);

#endif // DUSK_NODE_DUSK_H
