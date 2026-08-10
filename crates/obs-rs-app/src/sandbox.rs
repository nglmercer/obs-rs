use std::{error::Error, path::PathBuf};

use obs_rs_plugin_api::PluginManifest;
use obs_rs_sandbox::SandboxedPluginManifest;
use obs_rs_util::Identifier;

pub(crate) fn sandbox_manifest() -> Result<SandboxedPluginManifest, Box<dyn Error>> {
    let manifest =
        PluginManifest::new("obs_rs_sandbox_demo", "OBS-RS sandbox demo source", "0.1.0")?;
    Ok(SandboxedPluginManifest::new(
        manifest,
        [Identifier::new("sandbox_pattern")?],
    )?)
}

pub(crate) fn sandbox_source_command() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .map_or_else(
            || PathBuf::from("obs-rs-sandbox-source"),
            |directory| directory.join("obs-rs-sandbox-source"),
        )
}
