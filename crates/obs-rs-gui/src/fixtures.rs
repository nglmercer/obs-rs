use std::error::Error;

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_capture::{CameraMode, CaptureKind};
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
                .find(|(id, _)| id.starts_with("v4l2-") || id.starts_with("nokhwa-camera-"))
                .or_else(|| devices.first())
                .map_or(fallback, |(id, _)| id.as_str())
        } else {
            devices.first().map_or(fallback, |(id, _)| id.as_str())
        };
        settings.set("device_id", device_id)?;
        if kind == "camera_capture" {
            if let Some(mode) = camera_modes_for_device(device_id).first().copied() {
                settings.set("capture_width", &mode.width().to_string())?;
                settings.set("capture_height", &mode.height().to_string())?;
                settings.set("capture_fps", &camera_fps_setting(mode))?;
                settings.set("capture_pixel_format", mode.pixel_format().as_str())?;
            }
        }
    }
    if kind == "x11_screen_capture" {
        if let Ok(display) = std::env::var("DISPLAY") {
            settings.set("display", &display)?;
        }
        settings.set("device_id", "x11-screen-0")?;
        settings.set("monitor", "")?;
    }
    if kind == "x11_window_capture" {
        if let Ok(display) = std::env::var("DISPLAY") {
            settings.set("display", &display)?;
        }
        // An empty selection captures the whole desktop, so a freshly added
        // window source renders something while the user picks a window.
        let window = capture_devices(kind)
            .first()
            .map_or_else(String::new, |(id, _)| id.clone());
        settings.set(
            "device_id",
            if window.is_empty() {
                "x11-window-0"
            } else {
                &window
            },
        )?;
        settings.set("window", &window)?;
    }
    if kind == "wayland_screen_capture" {
        // The portal issues the token on the first share, so it starts empty.
        settings.set("restore_token", "")?;
        settings.set("capture_cursor", "true")?;
    }
    Ok(settings)
}

/// Source kinds whose frames come from one selectable display.
pub(crate) const MONITOR_SOURCE_KINDS: [&str; 2] = ["x11_screen_capture", "wayland_screen_capture"];

/// Returns whether `kind` reads a display the user can choose.
pub(crate) fn kind_selects_monitor(kind: &str) -> bool {
    MONITOR_SOURCE_KINDS.contains(&kind.trim())
}

/// Returns whether `kind` picks its display through the desktop portal.
///
/// On Wayland the compositor owns the picker, so OBS-RS asks the portal
/// instead of drawing a monitor list it has no way to enumerate.
pub(crate) fn kind_uses_portal(kind: &str) -> bool {
    kind.trim() == "wayland_screen_capture"
}

/// Returns whether a source kind can produce frames in this session.
///
/// Under Wayland the X11 adapter only ever sees Xwayland's own surfaces, which
/// is a black frame for a desktop capture; under X11 there is no screen-cast
/// portal to ask. Offering the wrong one is how a screen source ends up
/// showing nothing, so the Add Source list hides it instead.
pub(crate) fn kind_runs_in_this_session(kind: &str) -> bool {
    match kind.trim() {
        "wayland_screen_capture" => wayland_session(),
        // The X11 adapters share one limitation: under Wayland they only ever
        // see Xwayland's own surfaces, so both are hidden rather than offered
        // as sources that would render a black frame.
        "x11_screen_capture" | "x11_window_capture" => !wayland_session(),
        _ => true,
    }
}

/// Returns whether this process is running under a Wayland compositor.
fn wayland_session() -> bool {
    #[cfg(target_os = "linux")]
    {
        obs_rs_capture::wayland_session_available()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// One selectable display, independent of the platform that reported it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MonitorChoice {
    /// The value written to the source's `monitor` setting.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) primary: bool,
}

impl MonitorChoice {
    /// Returns the `1920x1080 at 0,0` line shown under the display name.
    pub(crate) fn geometry(&self) -> String {
        format!("{}x{} at {},{}", self.width, self.height, self.x, self.y)
    }
}

/// Lists the displays a screen capture source can be pointed at.
///
/// Returns an empty list when no display server is reachable, which the picker
/// reports rather than silently offering a choice that cannot be honoured.
#[cfg(target_os = "linux")]
pub(crate) fn screen_monitors() -> Vec<MonitorChoice> {
    let Ok(display) = std::env::var("DISPLAY") else {
        return Vec::new();
    };
    obs_rs_capture::x11_monitors(&display)
        .unwrap_or_default()
        .into_iter()
        .map(|monitor| MonitorChoice {
            // The `RandR` name is what the capture backend resolves, so it is
            // the value stored in the project rather than the catalog ID.
            id: monitor.name().to_owned(),
            name: monitor.name().to_owned(),
            x: monitor.x(),
            y: monitor.y(),
            width: monitor.width(),
            height: monitor.height(),
            primary: monitor.primary(),
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn screen_monitors() -> Vec<MonitorChoice> {
    Vec::new()
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
        "window_capture" | "x11_window_capture" => CaptureKind::Window,
        "camera_capture" => CaptureKind::Camera,
        _ => return Vec::new(),
    };
    let Ok(plugin) = BuiltinPlugin::new() else {
        return Vec::new();
    };
    let mut devices = if matches!(kind, "x11_screen_capture" | "x11_window_capture") {
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

/// Returns the native modes reported by the selected camera.
///
/// Modes are looked up separately from the displayable device list: that list
/// is also used by non-camera sources and only needs stable IDs and labels.
/// An unavailable device produces an empty list, so the properties form omits
/// controls it cannot honour.
pub(crate) fn camera_modes_for_device(device_id: &str) -> Vec<CameraMode> {
    let Ok(plugin) = BuiltinPlugin::new() else {
        return Vec::new();
    };
    plugin
        .discover_platform_capture_devices()
        .unwrap_or_default()
        .into_iter()
        .find(|device| device.kind() == CaptureKind::Camera && device.id().as_str() == device_id)
        .map_or_else(Vec::new, |device| {
            device.capabilities().camera_modes().to_vec()
        })
}

fn camera_fps_setting(mode: CameraMode) -> String {
    let frame_rate = mode.frame_rate();
    if frame_rate.denominator() == 1 {
        frame_rate.numerator().to_string()
    } else {
        format!("{}/{}", frame_rate.numerator(), frame_rate.denominator())
    }
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
