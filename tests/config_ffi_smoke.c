#include "obs_rs_config.h"

#include <stddef.h>
#include <stdint.h>
#include <string.h>

int main(void)
{
    static const uint8_t document[] = "zeta=2\nalpha=1\n";
    obs_rs_config_error error = OBS_RS_CONFIG_INTERNAL_FAILURE;
    obs_rs_config *config = obs_rs_config_create(document, sizeof(document) - 1, &error);

    if (config == NULL || error != OBS_RS_CONFIG_OK) {
        obs_rs_config_destroy(config);
        return 1;
    }

    size_t required = 0;
    uint8_t output[32] = {0};
    if (!obs_rs_config_serialize(config, output, sizeof(output), &required, &error) ||
        error != OBS_RS_CONFIG_OK || required != sizeof("alpha=1\nzeta=2\n") - 1 ||
        memcmp(output, "alpha=1\nzeta=2\n", required) != 0) {
        obs_rs_config_destroy(config);
        return 2;
    }

    if (!obs_rs_config_set(config, (const uint8_t *)"alpha", 5,
                           (const uint8_t *)"updated", 7, &error) ||
        error != OBS_RS_CONFIG_OK) {
        obs_rs_config_destroy(config);
        return 3;
    }

    memset(output, 0, sizeof(output));
    if (!obs_rs_config_get(config, (const uint8_t *)"alpha", 5,
                           output, sizeof(output), &required, &error) ||
        error != OBS_RS_CONFIG_OK || required != 7 ||
        memcmp(output, "updated", required) != 0) {
        obs_rs_config_destroy(config);
        return 4;
    }

    obs_rs_config_destroy(config);
    obs_rs_config_destroy(NULL);
    return 0;
}
