#ifndef DUSK_PROGRAM_LOGS_LOGS_H
#define DUSK_PROGRAM_LOGS_LOGS_H

#include <stddef.h>
#include <stdint.h>

enum {
    DUSK_LOGS_TRACE = 0,
    DUSK_LOGS_DEBUG = 1,
    DUSK_LOGS_INFO = 2,
    DUSK_LOGS_WARN = 3,
    DUSK_LOGS_ERROR = 4,
};

enum {
    DUSK_LOGS_WRITE_OK = 0,
    DUSK_LOGS_WRITE_INVALID_SEVERITY = 1,
    DUSK_LOGS_WRITE_NULL_ARGUMENT = 2,
    DUSK_LOGS_WRITE_INVALID_UTF8 = 3,
    DUSK_LOGS_WRITE_NOT_INSTALLED = 4,
    DUSK_LOGS_WRITE_FAILED = 5,
};

typedef struct dusk_logs_attribute {
    const char *key;
    const char *value;
} dusk_logs_attribute;

int32_t dusk_logs_write(int32_t severity, const char *target, const char *message,
                        const dusk_logs_attribute *attributes, size_t attribute_count);

#endif
