use std::{fmt::Write, path::Path};

use super::*;
use obs_rs_capture::encode_frame_packet;
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{Plugin, PluginManifest, VideoRequest};
use obs_rs_util::Identifier;

fn manifest() -> SandboxedPluginManifest {
    let plugin =
        PluginManifest::new("sandbox_plugin", "Sandbox plugin", "0.1.0").expect("manifest");
    SandboxedPluginManifest::new(plugin, [Identifier::new("external_pattern").expect("kind")])
        .expect("sandbox manifest")
}

#[test]
fn sandbox_manifest_round_trips_and_rejects_duplicates() {
    let manifest = manifest();
    let encoded = manifest.serialize();
    assert_eq!(SandboxedPluginManifest::parse(&encoded), Ok(manifest));
    assert!(matches!(
        SandboxedPluginManifest::parse(
            "OBSRPLUGIN1\nsandbox_plugin|Sandbox|0.1.0|1|0|external_pattern,external_pattern\n"
        ),
        Err(SandboxError::InvalidManifest { .. })
    ));
}

#[test]
fn sandbox_plugin_exposes_versioned_process_factories_without_spawning() {
    let manifest = manifest();
    let plugin = SandboxedPlugin::new(&manifest, "obs-rs-extension", vec!["--source".to_owned()])
        .expect("plugin configuration");
    assert_eq!(plugin.manifest().id().as_str(), "sandbox_plugin");
    assert_eq!(plugin.source_factories().len(), 1);
    assert_eq!(plugin.command(), Path::new("obs-rs-extension"));
    assert_eq!(plugin.arguments(), &["--source".to_owned()]);
}

#[test]
fn sandbox_plugin_rejects_unbounded_process_configuration() {
    let too_many = vec!["x".to_owned(); MAX_SANDBOX_ARGUMENTS + 1];
    assert!(matches!(
        SandboxedPlugin::new(&manifest(), "extension", too_many),
        Err(SandboxError::InvalidArguments { .. })
    ));
    assert!(matches!(
        SandboxedPluginManifest::parse("invalid"),
        Err(SandboxError::InvalidManifest { .. })
    ));
}

#[cfg(unix)]
#[test]
fn sandbox_manifest_can_be_discovered_before_source_creation() {
    let script = r#"if [ "$1" = "--obs-rs-manifest" ]; then printf 'OBSRPLUGIN1\nsandbox_plugin|Sandbox plugin|0.1.0|1|0|external_pattern\n'; else exit 1; fi"#;
    let arguments = vec![
        "-c".to_owned(),
        script.to_owned(),
        "sandbox-probe".to_owned(),
    ];
    let discovered =
        discover_sandbox_manifest("/bin/sh", &arguments).expect("manifest probe should complete");
    assert_eq!(discovered, manifest());
    let plugin = SandboxedPlugin::from_process("/bin/sh", arguments)
        .expect("process plugin should use discovered manifest");
    assert_eq!(plugin.source_factories().len(), 1);

    let oversized_arguments = vec![
        "-c".to_owned(),
        "head -c 32769 /dev/zero".to_owned(),
        "sandbox-oversized".to_owned(),
    ];
    assert_eq!(
        discover_sandbox_manifest("/bin/sh", &oversized_arguments),
        Err(SandboxError::ManifestTooLarge)
    );
}

#[cfg(unix)]
#[test]
fn sandbox_source_reads_one_bounded_frame_from_a_child_process() {
    let format = VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
    let expected = VideoFrame::solid(format, Timestamp::from_millis(7), [1, 2, 3, 255]);
    let packet = encode_frame_packet(&expected).expect("frame packet");
    let mut escaped = String::new();
    for byte in packet {
        write!(&mut escaped, r"\{byte:03o}").expect("escape packet byte");
    }
    let script = format!("printf '%b' '{escaped}'");
    let manifest = manifest();
    let plugin = SandboxedPlugin::new(&manifest, "/bin/sh", vec!["-c".to_owned(), script])
        .expect("sandbox process configuration");
    let factory = plugin
        .source_factories()
        .into_iter()
        .next()
        .expect("sandbox source factory");
    let mut settings = Config::new();
    settings.set("width", "1").expect("width");
    settings.set("height", "1").expect("height");
    let mut source = factory.create("fixture", &settings).expect("source");
    let received = source
        .render(&VideoRequest::new(Timestamp::ZERO, format))
        .expect("frame from sandbox")
        .expect("one frame");
    assert_eq!(received, expected);
}
