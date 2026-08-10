#include "obs_module_abi.h"

#include <dlfcn.h>
#include <stdint.h>
#include <string.h>

int main(void)
{
    void *library = dlopen("target/release/libobs_rs_plugin_probe.so", RTLD_NOW | RTLD_LOCAL);
    if (library == NULL) {
        return 1;
    }

    obs_module_load_fn load = (obs_module_load_fn)dlsym(library, "obs_module_load");
    obs_module_set_pointer_fn set_pointer =
        (obs_module_set_pointer_fn)dlsym(library, "obs_module_set_pointer");
    obs_module_ver_fn version = (obs_module_ver_fn)dlsym(library, "obs_module_ver");
    obs_module_string_fn name = (obs_module_string_fn)dlsym(library, "obs_module_name");

    if (load == NULL || set_pointer == NULL || version == NULL || name == NULL) {
        dlclose(library);
        return 2;
    }

    set_pointer((obs_module_t *)(uintptr_t)1);
    if (!load() || version() != OBS_RS_LIBOBS_API_VER ||
        strcmp(name(), "obs-rs-plugin-probe") != 0) {
        dlclose(library);
        return 3;
    }

    dlclose(library);
    return 0;
}
