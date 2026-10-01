#ifndef DUSK_DUSK_H
#define DUSK_DUSK_H

#include <stdint.h>

#if defined(DUSK_PTHREAD)
#include <pthread.h>
#include <stdlib.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

enum dusk_result_source {
    DUSK_RESULT_SOURCE_NAMESPACE = 0,
    DUSK_RESULT_SOURCE_DUSK_MAIN = 1,
    DUSK_RESULT_SOURCE_RUST_PANIC = 2,
};

enum dusk_main_failed {
    DUSK_MAIN_FAILED_INIT_ARGS = 1,
    DUSK_MAIN_FAILED_UNKNOWN_HANDLE = 2,
    DUSK_MAIN_FAILED_BOUND_HANDLE = 3,
    DUSK_MAIN_FAILED_SPAWN = 4,
};

union dusk_result {
    int64_t raw;
    struct {
#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
        int64_t value : 48;
        uint64_t source : 16;
#else
        uint64_t source : 16;
        int64_t value : 48;
#endif
    };
};

uint64_t dusk_new(void);

int64_t dusk_run(uint64_t handle, void *user);

#if defined(DUSK_PTHREAD)
struct dusk_spawn_args_ {
    uint64_t handle;
    void *user;
    void (*finalize)(void *user, int64_t result);
};

static inline int dusk_spawn_trampoline_0_(struct dusk_spawn_args_ *args);

static inline uint64_t dusk_spawn(void *user,
                                  void (*finalize)(void *user, int64_t result)) {
    struct dusk_spawn_args_ *args = (struct dusk_spawn_args_ *)malloc(sizeof *args);
    if (args) {
        uint64_t handle = dusk_new();
        args->handle = handle;
        args->user = user;
        args->finalize = finalize;
        if (dusk_spawn_trampoline_0_(args)) {
            return handle;
        }
        free(args);
    }
    if (finalize) {
        finalize(user, (DUSK_MAIN_FAILED_SPAWN << 16) | DUSK_RESULT_SOURCE_DUSK_MAIN);
    }
    return 0;
}

static inline void dusk_spawn_trampoline_2_(void *arg) {
    struct dusk_spawn_args_ args = *(struct dusk_spawn_args_ *)arg;
    free(arg);
    int64_t result = dusk_run(args.handle, args.user);
    if (args.finalize) {
        args.finalize(args.user, result);
    }
}

static inline void *dusk_spawn_trampoline_1_(void *arg) {
    dusk_spawn_trampoline_2_(arg);
    return NULL;
}

static inline int dusk_spawn_trampoline_0_(struct dusk_spawn_args_ *args) {
    pthread_t thread;
    if (pthread_create(&thread, NULL, dusk_spawn_trampoline_1_, args) != 0) {
        return 0;
    }
    pthread_detach(thread);
    return 1;
}
#endif

#ifdef __cplusplus
}
#endif

#endif // DUSK_DUSK_H
