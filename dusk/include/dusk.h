#ifndef DUSK_DUSK_H
#define DUSK_DUSK_H

#include <stdint.h>

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

#ifdef __cplusplus
}
#endif

#endif // DUSK_DUSK_H
