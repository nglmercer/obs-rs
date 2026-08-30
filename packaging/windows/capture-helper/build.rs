use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=obs-rs-capture-windows-helper.manifest");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(
        env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR to the helper build script"),
    );
    let manifest_name = "obs-rs-capture-windows-helper.manifest";
    let manifest = PathBuf::from(manifest_name);
    let staged_manifest = out_dir.join(manifest_name);
    fs::copy(&manifest, &staged_manifest).expect("stage the Windows helper manifest");

    // Keep the resource script generated in OUT_DIR. The repository therefore
    // contains the declarative manifest but no checked-in C/C++ resource
    // source, while embed-resource still works with both RC.EXE and LLVM-RC.
    let resource = out_dir.join("obs-rs-capture-windows-helper.rc");
    fs::write(
        &resource,
        format!("#define RT_MANIFEST 24\n1 RT_MANIFEST \"{manifest_name}\"\n"),
    )
    .expect("write the generated Windows helper resource script");

    embed_resource::compile(&resource, embed_resource::NONE)
        .manifest_required()
        .expect("embed the Windows helper manifest");
}
