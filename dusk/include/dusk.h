#ifndef DUSK_DUSK_H
#define DUSK_DUSK_H

#include <stdint.h>
#if defined(DUSK_PTHREAD) || defined(DUSK_WIN32)
#include <stdlib.h>
#endif

#if defined(DUSK_PTHREAD)
#include <pthread.h>
#elif defined(DUSK_WIN32)
#include <windows.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

/**
 * @file
 * @brief Run Dusk namespaces from C.
 *
 * dusk_new returns a handle: 1, 2, 3 and up, never reused. Pass it to dusk_run
 * to run a namespace under it on the calling thread. When that namespace stops,
 * the handle may be run again.
 *
 * dusk_spawn calls dusk_new and dusk_run for you on a new thread, and passes
 * the result to finalize when the namespace terminates and or failure.
 *
 * Every function may be called from any thread.
 */

/**
 * @brief The source of a result: what produced its value.
 */
enum dusk_result_source {
    /** The namespace exited. The value is its exit code. */
    DUSK_RESULT_SOURCE_NAMESPACE = 0,
    /** Dusk could not run the namespace. The value is a `dusk_main_failed` saying why.
     */
    DUSK_RESULT_SOURCE_DUSK_MAIN = 1,
    /** There was a rust panic. The value is 0. */
    DUSK_RESULT_SOURCE_RUST_PANIC = 2,
};

/** @brief Reasons why we failed before the Dusk namespace even started.
 *  later versions may add more. */
enum dusk_main_failed {
    DUSK_MAIN_FAILED_INIT_ARGS = 1,      /**< Its init arguments could not be built. */
    DUSK_MAIN_FAILED_UNKNOWN_HANDLE = 2, /**< No such handle. */
    DUSK_MAIN_FAILED_BOUND_HANDLE = 3, /**< A namespace is running under the handle. */
    DUSK_MAIN_FAILED_SPAWN = 4,        /**< dusk_spawn could not start a thread. */
};

union dusk_result {
    int64_t raw;
    struct {
#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
        int64_t value : 48; // `meaning as described by the docs of the specific source`
        uint64_t source : 16; // `enum dusk_result_source`
#else
        uint64_t source : 16; // `enum dusk_result_source`
        int64_t value : 48; // `meaning as described by the docs of the specific source`
#endif
    };
};

/** @brief Registers a new namespace. @return its `handle`. */
uint64_t dusk_new(void);

/** @brief Runs the namespace of `handle` on this thread. @return its result. */
int64_t dusk_run(uint64_t handle, void *user);

#if defined(DUSK_PTHREAD) || defined(DUSK_WIN32)
struct dusk_spawn_args_ {
    uint64_t handle;
    void *user;
    void (*finalize)(void *user, int64_t result);
};

static inline int dusk_spawn_trampoline_0_(struct dusk_spawn_args_ *args);

/**
 * @brief Runs dusk_run(dusk_new(), user) on a new thread, which takes `user`.
 *
 * Calls `finalize(user, result)` exactly once, when the namespace has
 * stopped, with its result - or, when the thread cannot start, with a result
 * whose value is DUSK_MAIN_FAILED_SPAWN. `finalize` may be NULL.
 *
 * @return its `handle`, or 0 if the thread cannot start.
 */
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

#if defined(DUSK_PTHREAD)
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
#elif defined(DUSK_WIN32)
static inline DWORD WINAPI dusk_spawn_trampoline_1_(LPVOID arg) {
    dusk_spawn_trampoline_2_(arg);
    return 0;
}

static inline int dusk_spawn_trampoline_0_(struct dusk_spawn_args_ *args) {
    HANDLE thread = CreateThread(NULL, 0, dusk_spawn_trampoline_1_, args, 0, NULL);
    if (thread == NULL) {
        return 0;
    }
    CloseHandle(thread);
    return 1;
}
#endif
#endif

#ifdef __cplusplus
}
#endif

#endif // DUSK_DUSK_H
