use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=obs-rs-gui.manifest");
    embed_windows_manifest();
    // The Slint compiler walks a large UI tree recursively. Windows gives a
    // build-script process a smaller default stack than this project needs,
    // so keep the compilation off the tiny process-main stack in clean CI.
    std::thread::Builder::new()
        .name("obs-rs-slint-build".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(compile_ui)
        .expect("Slint compiler thread should start")
        .join()
        .expect("Slint compiler thread should finish");
}

fn embed_windows_manifest() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(
        env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR to the GUI build script"),
    );
    let manifest_name = "obs-rs-gui.manifest";
    let manifest = PathBuf::from(manifest_name);
    let staged_manifest = out_dir.join(manifest_name);
    fs::copy(&manifest, &staged_manifest).expect("stage the Windows GUI manifest");

    // Keep the resource script generated in OUT_DIR. The GUI stays a normal
    // Rust binary while the Windows entry point receives the same DPI and
    // supported-OS declaration as the native capture helper.
    let resource = out_dir.join("obs-rs-gui.rc");
    fs::write(
        &resource,
        format!("#define RT_MANIFEST 24\n1 RT_MANIFEST \"{manifest_name}\"\n"),
    )
    .expect("write the generated Windows GUI resource script");

    embed_resource::compile(&resource, embed_resource::NONE)
        .manifest_required()
        .expect("embed the Windows GUI manifest");
}

fn compile_ui() {
    // The chrome is a fixed dark palette, so the bundled widgets have to use
    // the dark style or every LineEdit and Button lands as a light-grey block.
    // Debug element metadata lets the headless UI test click real menu targets;
    // release builds omit it so production item trees stay compact.
    let emit_debug_info = std::env::var("PROFILE")
        .is_ok_and(|profile| !matches!(profile.as_str(), "release" | "dev-fast-gui"));
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".to_owned())
        .with_debug_info(emit_debug_info);
    slint_build::compile_with_config("ui/main.slint", config).expect("Slint UI should compile");
}
