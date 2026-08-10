#ifndef OBS_MODULE_ABI_H
#define OBS_MODULE_ABI_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque type matching obs_module_t in libobs/obs.h. */
typedef struct obs_module obs_module_t;

/*
 * The actual OBS build must provide its generated LIBOBS_API_VER here. The probe
 * defaults to zero so it cannot be mistaken for a production-compatible plugin.
 */
#ifndef OBS_RS_LIBOBS_API_VER
#define OBS_RS_LIBOBS_API_VER 0U
#endif

typedef bool (*obs_module_load_fn)(void);
typedef void (*obs_module_set_pointer_fn)(obs_module_t *module);
typedef uint32_t (*obs_module_ver_fn)(void);
typedef const char *(*obs_module_string_fn)(void);

/* Required exports from libobs/obs-module.h. */
bool obs_module_load(void);
void obs_module_set_pointer(obs_module_t *module);
uint32_t obs_module_ver(void);

/* Optional exports implemented by the probe. */
const char *obs_module_name(void);
const char *obs_module_description(void);
const char *obs_module_author(void);

#ifdef __cplusplus
}
#endif

#endif /* OBS_MODULE_ABI_H */
