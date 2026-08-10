#ifndef OBS_RS_CONFIG_H
#define OBS_RS_CONFIG_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OBS_RS_CONFIG_MAX_DOCUMENT_BYTES (64U * 1024U)
#define OBS_RS_CONFIG_MAX_VALUE_BYTES 4096U

typedef struct obs_rs_config obs_rs_config;

typedef enum obs_rs_config_error {
    OBS_RS_CONFIG_OK = 0,
    OBS_RS_CONFIG_NULL_INPUT = 1,
    OBS_RS_CONFIG_INVALID_UTF8 = 2,
    OBS_RS_CONFIG_INVALID_LINE = 3,
    OBS_RS_CONFIG_INVALID_KEY = 4,
    OBS_RS_CONFIG_DUPLICATE_KEY = 5,
    OBS_RS_CONFIG_INVALID_VALUE = 6,
    OBS_RS_CONFIG_VALUE_TOO_LONG = 7,
    OBS_RS_CONFIG_NULL_CONFIG = 8,
    OBS_RS_CONFIG_BUFFER_TOO_SMALL = 9,
    OBS_RS_CONFIG_KEY_NOT_FOUND = 10,
    OBS_RS_CONFIG_INPUT_TOO_LARGE = 11,
    OBS_RS_CONFIG_INTERNAL_FAILURE = 12,
} obs_rs_config_error;

/* Parses an explicit-length UTF-8 document and transfers ownership of a new handle. */
obs_rs_config *obs_rs_config_create(const uint8_t *input,
                                    size_t length,
                                    obs_rs_config_error *error);

/* Accepts null or a live handle returned by obs_rs_config_create exactly once. */
void obs_rs_config_destroy(obs_rs_config *config);

/* Inserts or replaces a UTF-8 key/value pair on an exclusively owned handle. */
bool obs_rs_config_set(obs_rs_config *config,
                       const uint8_t *key,
                       size_t key_length,
                       const uint8_t *value,
                       size_t value_length,
                       obs_rs_config_error *error);

/*
 * Copies one value without a NUL terminator. `required` receives the exact byte
 * count. A null output with zero capacity can query the size and returns false for
 * non-empty values with OBS_RS_CONFIG_BUFFER_TOO_SMALL.
 */
bool obs_rs_config_get(const obs_rs_config *config,
                       const uint8_t *key,
                       size_t key_length,
                       uint8_t *output,
                       size_t capacity,
                       size_t *required,
                       obs_rs_config_error *error);

/* Serializes sorted key=value\n bytes without a NUL terminator. */
bool obs_rs_config_serialize(const obs_rs_config *config,
                             uint8_t *output,
                             size_t capacity,
                             size_t *required,
                             obs_rs_config_error *error);

#ifdef __cplusplus
}
#endif

#endif /* OBS_RS_CONFIG_H */
