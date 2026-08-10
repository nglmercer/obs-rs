#include "obs_rs_util.h"

#include <stddef.h>
#include <stdint.h>
#include <string.h>

int main(void)
{
    static const uint8_t valid[] = "obs_source";
    obs_rs_util_error error = OBS_RS_UTIL_INTERNAL_FAILURE;
    char *copy = obs_rs_util_identifier_copy(valid, sizeof(valid) - 1, &error);

    if (copy == NULL || error != OBS_RS_UTIL_OK || strcmp(copy, "obs_source") != 0) {
        obs_rs_util_string_free(copy);
        return 1;
    }

    obs_rs_util_string_free(copy);

    error = OBS_RS_UTIL_INTERNAL_FAILURE;
    if (obs_rs_util_identifier_copy(NULL, 0, &error) != NULL ||
        error != OBS_RS_UTIL_NULL_INPUT) {
        return 2;
    }

    obs_rs_util_string_free(NULL);
    return 0;
}
