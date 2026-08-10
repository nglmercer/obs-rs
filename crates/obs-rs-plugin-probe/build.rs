use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=OBS_LIBOBS_API_VER");

    let version = env::var("OBS_LIBOBS_API_VER").unwrap_or_else(|_| "0".to_owned());
    let version = version
        .parse::<u32>()
        .expect("OBS_LIBOBS_API_VER must be an unsigned 32-bit integer");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));

    fs::write(
        output.join("api_version.rs"),
        format!("const OBS_MODULE_API_VERSION: u32 = {version};\n"),
    )
    .expect("unable to write the generated API-version binding");
}
