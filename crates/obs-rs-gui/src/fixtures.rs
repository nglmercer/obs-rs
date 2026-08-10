use std::error::Error;

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_project::{Profile, Project, SceneSpec, SourceSpec};

pub(crate) fn initial_project() -> Result<Project, Box<dyn Error>> {
    let format = VideoFormat::new(640, 360, FrameRate::new(30, 1)?)?;
    let mut project = Project::new("OBS-RS Studio")?;
    let mut profile = Profile::new("live", "Live profile", format)?;
    profile.add_scene(scene("preview", "Preview", "#102030FF")?)?;
    profile.add_scene(scene("program", "Program", "#203040FF")?)?;
    let mut intermission = scene("intermission", "Intermission", "#302040FF")?;
    intermission.add_source(SourceSpec::new(
        "pattern",
        "test_pattern",
        "Animated pattern",
        video_settings(),
    )?)?;
    profile.add_scene(intermission)?;
    project.add_profile(profile)?;
    Ok(project)
}

pub(crate) fn platform_capture_summary() -> String {
    let plugin = match BuiltinPlugin::new() {
        Ok(plugin) => plugin,
        Err(error) => return format!("Platform capture discovery failed: {error}"),
    };
    match plugin.discover_platform_capture_devices() {
        Ok(devices) if devices.is_empty() => {
            "Platform capture: no devices; CPU fallback sources available".to_owned()
        }
        Ok(devices) => {
            let names = devices
                .iter()
                .map(|device| device.name().to_owned())
                .collect::<Vec<_>>()
                .join(", ");
            format!("Platform capture: {names}")
        }
        Err(error) => {
            format!("Platform capture unavailable: {error}; CPU fallback sources available")
        }
    }
}

fn video_settings() -> Config {
    let mut settings = Config::new();
    settings
        .set("width", "640")
        .expect("static width setting is valid");
    settings
        .set("height", "360")
        .expect("static height setting is valid");
    settings
}

pub(crate) fn source_settings(kind: &str) -> Result<Config, Box<dyn Error>> {
    let mut settings = video_settings();
    if kind.trim() == "color_source" {
        settings.set("color", "#405070FF")?;
    }
    if kind.trim() == "x11_screen_capture" {
        if let Ok(display) = std::env::var("DISPLAY") {
            settings.set("display", &display)?;
        }
    }
    Ok(settings)
}

fn scene(id: &str, name: &str, color: &str) -> Result<SceneSpec, Box<dyn Error>> {
    let mut settings = Config::new();
    settings.set("width", "640")?;
    settings.set("height", "360")?;
    settings.set("color", color)?;
    let mut scene = SceneSpec::new(id, name)?;
    scene.add_source(SourceSpec::new(
        "background",
        "color_source",
        "Background",
        settings,
    )?)?;
    Ok(scene)
}
