fn main() {
    println!("cargo:rerun-if-changed=ui");
    // The chrome is a fixed dark palette, so the bundled widgets have to use
    // the dark style or every LineEdit and Button lands as a light-grey block.
    // Debug element metadata lets the headless UI test click real menu targets;
    // release builds omit it so production item trees stay compact.
    let emit_debug_info = std::env::var("PROFILE").is_ok_and(|profile| profile != "release");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".to_owned())
        .with_debug_info(emit_debug_info);
    slint_build::compile_with_config("ui/main.slint", config).expect("Slint UI should compile");
}
