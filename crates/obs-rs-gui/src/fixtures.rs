use std::error::Error;

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_capture::CaptureKind;
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
    let kind = kind.trim();
    if matches!(kind, "screen_capture" | "window_capture" | "camera_capture") {
        let fallback = match kind {
            "screen_capture" => "screen-0",
            "window_capture" => "window-0",
            "camera_capture" => "camera-0",
            _ => unreachable!("kind was checked above"),
        };
        let devices = capture_devices(kind);
        let device_id = if kind == "camera_capture" {
            devices
                .iter()
                .find(|(id, _)| id.starts_with("v4l2-"))
                .or_else(|| devices.first())
                .map_or(fallback, |(id, _)| id.as_str())
        } else {
            devices.first().map_or(fallback, |(id, _)| id.as_str())
        };
        settings.set("device_id", device_id)?;
    }
    if kind == "x11_screen_capture" {
        if let Ok(display) = std::env::var("DISPLAY") {
            settings.set("display", &display)?;
        }
        settings.set("device_id", "x11-screen-0")?;
    }
    Ok(settings)
}

/// Returns the devices that a source-properties editor can select.
///
/// The returned list matches the backend behind the source kind. The generic
/// portable sources expose deterministic fallback devices; the Linux X11
/// source exposes only its native screen adapter. This prevents an X11 device
/// from appearing selectable in the simulated `screen_capture` factory where
/// it would silently have no effect.
pub(crate) fn capture_devices(kind: &str) -> Vec<(String, String)> {
    let kind = kind.trim();
    let wanted = match kind {
        "screen_capture" | "x11_screen_capture" => CaptureKind::Screen,
        "window_capture" => CaptureKind::Window,
        "camera_capture" => CaptureKind::Camera,
        _ => return Vec::new(),
    };
    let Ok(plugin) = BuiltinPlugin::new() else {
        return Vec::new();
    };
    let mut devices = if kind == "x11_screen_capture" {
        plugin
            .discover_platform_capture_devices()
            .unwrap_or_default()
            .into_iter()
            .filter(|device| device.kind() == wanted)
            .map(|device| (device.id().to_string(), device.name().to_owned()))
            .collect::<Vec<_>>()
    } else {
        plugin
            .discover_capture_devices()
            .unwrap_or_default()
            .into_iter()
            .filter(|device| device.kind() == wanted)
            .map(|device| (device.id().to_string(), device.name().to_owned()))
            .collect::<Vec<_>>()
    };
    if kind == "camera_capture" {
        if let Ok(platform_devices) = plugin.discover_platform_capture_devices() {
            devices.extend(
                platform_devices
                    .into_iter()
                    .filter(|device| device.kind() == wanted)
                    .map(|device| (device.id().to_string(), device.name().to_owned())),
            );
        }
    }
    devices.sort_by(|left, right| left.0.cmp(&right.0));
    devices.dedup_by(|left, right| left.0 == right.0);
    devices
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
