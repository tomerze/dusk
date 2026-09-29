#ifndef DUSK_NODE_DUSK_H
#define DUSK_NODE_DUSK_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    DUSK_RUN_OK = 0,
    DUSK_RUN_LAUNCHERS_FAILED = 1,
    DUSK_RUN_INIT_ARGS_FAILED = 2,
    DUSK_RUN_UNKNOWN_NAMESPACE = 3,
    DUSK_RUN_ALREADY_RUNNING = 4,
    DUSK_RUN_NOT_SPAWNED = 5,
};

enum {
    DUSK_STATUS_MASK = 0xff,
    DUSK_EXIT_CODE_SHIFT = 8,
};

int64_t dusk_new(void);
int32_t dusk_run(int64_t namespace_id, void *user);
int64_t dusk_spawn(void *user);
int32_t dusk_join(int64_t namespace_id);
void dusk_free(int64_t namespace_id);

#ifdef __cplusplus
}
#endif

#endif // DUSK_NODE_DUSK_H
