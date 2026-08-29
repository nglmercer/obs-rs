fn main() {
    println!("cargo:rerun-if-changed=ui");
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
