#ifndef OBS_RS_UTIL_H
#define OBS_RS_UTIL_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OBS_RS_UTIL_MAX_IDENTIFIER_BYTES 64

typedef enum obs_rs_util_error {
    OBS_RS_UTIL_OK = 0,
    OBS_RS_UTIL_NULL_INPUT = 1,
    OBS_RS_UTIL_EMPTY_INPUT = 2,
    OBS_RS_UTIL_TOO_LONG = 3,
    OBS_RS_UTIL_INVALID_UTF8 = 4,
    OBS_RS_UTIL_INVALID_FIRST_CHARACTER = 5,
    OBS_RS_UTIL_INVALID_CHARACTER = 6,
    OBS_RS_UTIL_INTERNAL_FAILURE = 7,
} obs_rs_util_error;

/*
 * Validates `input[0..length)` and returns a newly allocated NUL-terminated copy
 * on success. The returned pointer must be released with obs_rs_util_string_free.
 * `input` and `output` may be null; null input reports OBS_RS_UTIL_NULL_INPUT and
 * null output suppresses the error result.
 */
char *obs_rs_util_identifier_copy(const uint8_t *input,
                                  size_t length,
                                  obs_rs_util_error *output);

/* Accepts null or a pointer returned by obs_rs_util_identifier_copy exactly once. */
void obs_rs_util_string_free(char *value);

#ifdef __cplusplus
}
#endif

#endif /* OBS_RS_UTIL_H */
