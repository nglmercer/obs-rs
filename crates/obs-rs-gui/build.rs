fn main() {
    println!("cargo:rerun-if-changed=ui");
    // The chrome is a fixed dark palette, so the bundled widgets have to use
    // the dark style or every LineEdit and Button lands as a light-grey block.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".to_owned());
    slint_build::compile_with_config("ui/main.slint", config).expect("Slint UI should compile");
}
