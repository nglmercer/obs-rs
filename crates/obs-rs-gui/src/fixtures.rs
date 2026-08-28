use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use obs_rs_builtins::BuiltinPlugin;
#[cfg(target_os = "windows")]
use obs_rs_capture::PlatformCaptureAdapter;
use obs_rs_capture::{CameraMode, CaptureDeviceInfo, CaptureKind};
#[cfg(target_os = "windows")]
use obs_rs_capture_windows::WindowsCaptureAdapter;
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_project::{Profile, Project, SceneItemSpec, SceneSpec, SourceSpec};

/// Platform discovery opens native camera descriptors and performs X11
/// round-trips. Keep one short-lived snapshot for repeated device-picker
/// refreshes while still allowing hot-plug changes to appear promptly.
const PLATFORM_DISCOVERY_CACHE_TTL: Duration = Duration::from_secs(1);
const CAMERA_MODE_CACHE_TTL: Duration = Duration::from_secs(5);
pub(crate) const DEFAULT_CANVAS_WIDTH: u32 = 1_280;
pub(crate) const DEFAULT_CANVAS_HEIGHT: u32 = 720;
pub(crate) const DEFAULT_CANVAS_FPS: u32 = 30;

type PlatformDiscoveryCache = BTreeMap<CaptureKind, (Instant, Vec<CaptureDeviceInfo>)>;
type CameraModeCache = BTreeMap<String, (Instant, Vec<CameraMode>)>;

static PLATFORM_DISCOVERY_CACHE: OnceLock<Mutex<PlatformDiscoveryCache>> = OnceLock::new();
static CAMERA_MODE_CACHE: OnceLock<Mutex<CameraModeCache>> = OnceLock::new();

/// Drops cached native capture descriptors before an explicit picker refresh.
///
/// Discovery is intentionally cached for ordinary property-window repaints,
/// but a Refresh button is an explicit request to observe hot-plug changes
/// immediately. Keeping invalidation here also makes every picker use the same
/// cache policy instead of one path showing stale devices for up to a second.
pub(crate) fn invalidate_capture_cache(kind: CaptureKind, camera_id: Option<&str>) {
    if let Some(cache) = PLATFORM_DISCOVERY_CACHE.get() {
        if let Ok(mut snapshot) = cache.lock() {
            snapshot.remove(&kind);
        }
    }
    if kind == CaptureKind::Camera {
        if let Some(camera_id) = camera_id.map(str::trim).filter(|id| !id.is_empty()) {
            if let Some(cache) = CAMERA_MODE_CACHE.get() {
                if let Ok(mut snapshot) = cache.lock() {
                    snapshot.remove(camera_id);
                }
            }
        }
    }
}

fn platform_devices_for_kind(kind: CaptureKind) -> Vec<CaptureDeviceInfo> {
    let cache = PLATFORM_DISCOVERY_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    let now = Instant::now();
    if let Ok(snapshot) = cache.lock() {
        if let Some((fetched, devices)) = snapshot.get(&kind) {
            if now.duration_since(*fetched) < PLATFORM_DISCOVERY_CACHE_TTL {
                return devices.clone();
            }
        }
    }

    let devices = BuiltinPlugin::new()
        .ok()
        .and_then(|plugin| plugin.discover_platform_capture_devices_for_kind(kind).ok())
        .unwrap_or_default();
    if let Ok(mut snapshot) = cache.lock() {
        snapshot.insert(kind, (Instant::now(), devices.clone()));
    }
    devices
}

pub(crate) fn initial_project() -> Result<Project, Box<dyn Error>> {
    let format = default_video_format()?;
    let mut project = Project::new("OBS-RS Studio")?;
    let mut profile = Profile::new("live", "Live profile", format)?;
    let (preview, preview_source) = scene("preview", "Preview", "background", "#102030FF", format)?;
    let (program, program_source) = scene(
        "program",
        "Program",
        "background_program",
        "#203040FF",
        format,
    )?;
    let (mut intermission, intermission_source) = scene(
        "intermission",
        "Intermission",
        "background_intermission",
        "#302040FF",
        format,
    )?;
    let pattern = SourceSpec::new(
        "pattern",
        "test_pattern",
        "Animated pattern",
        video_settings_for_format(format),
    )?;
    intermission.add_item(SceneItemSpec::for_source("pattern")?)?;
    profile.add_source(preview_source)?;
    profile.add_source(program_source)?;
    profile.add_source(intermission_source)?;
    profile.add_source(pattern)?;
    profile.add_scene(intermission)?;
    profile.add_scene(preview)?;
    profile.add_scene(program)?;
    project.add_profile(profile)?;
    Ok(project)
}

pub(crate) fn platform_capture_summary() -> String {
    #[cfg(target_os = "windows")]
    {
        let adapter = WindowsCaptureAdapter::default();
        let capture = match adapter.discover() {
            Ok(devices) => {
                let displays = devices
                    .iter()
                    .filter(|device| device.kind() == CaptureKind::Screen)
                    .count();
                let windows = devices
                    .iter()
                    .filter(|device| device.kind() == CaptureKind::Window)
                    .count();
                format!(
                    "Windows capture: helper={} displays={} windows={}",
                    adapter.helper().display(),
                    displays,
                    windows
                )
            }
            Err(error) => format!(
                "Windows capture unavailable: helper={} error={error}",
                adapter.helper().display()
            ),
        };
        let camera = match platform_devices_for_kind(CaptureKind::Camera).len() {
            0 => "cameras=0".to_owned(),
            count => format!("cameras={count}"),
        };
        format!("{capture}; {camera}")
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Startup diagnostics do not need a complete catalog. The old full
        // discovery walked every X11 window and opened every camera to enumerate
        // its modes before the first window appeared. Screen/camera summaries are
        // enough here; the properties picker performs kind-specific discovery.
        let mut names = [CaptureKind::Screen, CaptureKind::Camera]
            .into_iter()
            .flat_map(platform_devices_for_kind)
            .map(|device| device.name().to_owned())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        if names.is_empty() {
            "Platform capture: no devices; CPU fallback sources available".to_owned()
        } else {
            format!("Platform capture: {}", names.join(", "))
        }
    }
}

fn video_settings_for_format(format: VideoFormat) -> Config {
    video_settings_for_dimensions(format.width(), format.height())
}

fn video_settings_for_dimensions(width: u32, height: u32) -> Config {
    let mut settings = Config::new();
    settings
        .set("width", &width.to_string())
        .expect("source width setting is valid");
    settings
        .set("height", &height.to_string())
        .expect("source height setting is valid");
    settings
}

pub(crate) fn source_settings(kind: &str) -> Result<Config, Box<dyn Error>> {
    source_settings_for_canvas(kind, DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT)
}

/// Returns defaults for a newly created source at the active canvas size.
///
/// Capture devices may later negotiate a native mode, but generic source
/// defaults must not silently reintroduce the old 640x360 project assumption.
pub(crate) fn source_settings_for_canvas(
    kind: &str,
    canvas_width: u32,
    canvas_height: u32,
) -> Result<Config, Box<dyn Error>> {
    let mut settings = video_settings_for_dimensions(canvas_width, canvas_height);
    if kind.trim() == "color_source" {
        settings.set("color", "#405070FF")?;
    }
    if kind.trim() == "image_source" {
        settings.set("path", "")?;
    }
    if kind.trim() == "image_slideshow" {
        settings.set("paths", "")?;
        settings.set("slide_time_ms", "8000")?;
        settings.set("fade", "false")?;
        settings.set("transition_ms", "500")?;
        settings.set("loop", "true")?;
        settings.set("randomize", "false")?;
    }
    if kind.trim() == "media_source" {
        settings.set("path", "")?;
        settings.set("loop", "true")?;
    }
    if kind.trim() == "text_source" {
        settings.set("text", "OBS-RS")?;
        settings.set("color", "#FFFFFFFF")?;
        settings.set("font_size", "24")?;
    }
    let kind = kind.trim();
    if matches!(kind, "screen_capture" | "window_capture" | "camera_capture") {
        let fallback = match kind {
            #[cfg(target_os = "windows")]
            "screen_capture" => "wgc-screen-picker",
            #[cfg(not(target_os = "windows"))]
            "screen_capture" => "screen-0",
            #[cfg(target_os = "windows")]
            "window_capture" => "wgc-window-picker",
            #[cfg(not(target_os = "windows"))]
            "window_capture" => "window-0",
            // Keep an unplugged camera source addressable without inventing a
            // second camera backend. The runtime reports this Nokhwa ID as
            // unavailable until the device is connected.
            "camera_capture" => "nokhwa-camera-0",
            _ => unreachable!("kind was checked above"),
        };
        let devices = capture_devices(kind);
        let device_id = if kind == "camera_capture" {
            devices
                .iter()
                .find(|(id, _)| id.starts_with("v4l2-") || id.starts_with("nokhwa-camera-"))
                .map_or(fallback, |(id, _)| id.as_str())
        } else {
            devices.first().map_or(fallback, |(id, _)| id.as_str())
        };
        settings.set("device_id", device_id)?;
        #[cfg(target_os = "windows")]
        if kind == "screen_capture" && device_id.starts_with("wgc-screen-") {
            // Keep a newly created source's monitor row aligned with the WGC
            // target chosen from the live display snapshot. The automatic
            // picker remains the fallback when discovery is unavailable.
            settings.set("monitor", device_id)?;
        }
        #[cfg(target_os = "windows")]
        if matches!(kind, "screen_capture" | "window_capture") {
            settings.set("capture_cursor", "true")?;
            settings.set("capture_border", "false")?;
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
        // The empty selection lets a freshly added window source render the
        // automatic target while the user picks a window.
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
#[cfg(target_os = "linux")]
pub(crate) const MONITOR_SOURCE_KINDS: [&str; 2] = ["x11_screen_capture", "wayland_screen_capture"];
#[cfg(target_os = "windows")]
pub(crate) const MONITOR_SOURCE_KINDS: [&str; 1] = ["screen_capture"];
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) const MONITOR_SOURCE_KINDS: [&str; 0] = [];

/// Returns whether `kind` reads a display the user can choose.
pub(crate) fn kind_selects_monitor(kind: &str) -> bool {
    MONITOR_SOURCE_KINDS.contains(&kind.trim())
}

/// Returns whether `kind` picks its display through the desktop portal.
///
/// On Wayland the compositor owns the picker, so OBS-RS asks the portal
/// instead of drawing a monitor list it has no way to enumerate.
#[cfg(target_os = "linux")]
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
        #[cfg(target_os = "windows")]
        // Keep native source kinds visible even when the helper is missing or
        // the desktop is locked. The properties dialog can then explain the
        // unavailable target and the user can install/restart the helper
        // without the source disappearing from Add Source.
        "screen_capture" | "window_capture" => true,
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

/// The virtual desktop rectangle reported by a platform monitor enumerator.
///
/// This is deliberately a capability result rather than a made-up fallback:
/// callers may preserve a saved window position when the platform cannot
/// enumerate the current desktop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DesktopBounds {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

/// Returns the rectangle spanning all known monitors.
pub(crate) fn desktop_bounds(monitors: &[MonitorChoice]) -> DesktopBounds {
    let left = monitors.iter().map(|monitor| monitor.x).min().unwrap_or(0);
    let top = monitors.iter().map(|monitor| monitor.y).min().unwrap_or(0);
    let right = monitors
        .iter()
        .map(|monitor| {
            monitor
                .x
                .saturating_add(i32::try_from(monitor.width).unwrap_or(i32::MAX))
        })
        .max()
        .unwrap_or(1);
    let bottom = monitors
        .iter()
        .map(|monitor| {
            monitor
                .y
                .saturating_add(i32::try_from(monitor.height).unwrap_or(i32::MAX))
        })
        .max()
        .unwrap_or(1);
    DesktopBounds {
        left,
        top,
        // A zero extent would divide by zero in the monitor map or make a
        // restore clamp impossible; one pixel is the smallest safe extent.
        width: (right - left).max(1),
        height: (bottom - top).max(1),
        right,
        bottom,
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
    #[cfg(target_os = "windows")]
    {
        WindowsCaptureAdapter::default()
            .discover_displays()
            .unwrap_or_default()
            .into_iter()
            .map(|display| MonitorChoice {
                id: display.id,
                name: display.name,
                x: display.x,
                y: display.y,
                width: display.width,
                height: display.height,
                primary: display.primary,
            })
            .collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// Returns the devices that a source-properties editor can select.
///
/// The returned list matches the backend behind the source kind. Native
/// Windows screen/window and camera sources expose only descriptors returned by
/// their real adapters; the properties layer may add an explicit automatic or
/// unavailable entry when the backend is missing or a saved target is gone.
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
    let native_generic = {
        #[cfg(target_os = "windows")]
        {
            matches!(kind, "screen_capture" | "window_capture")
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    };
    let mut devices = if native_generic
        || kind == "camera_capture"
        || matches!(kind, "x11_screen_capture" | "x11_window_capture")
    {
        platform_devices_for_kind(wanted)
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
    if device_id.trim().is_empty()
        || (!device_id.starts_with("v4l2-") && !device_id.starts_with("nokhwa-camera-"))
    {
        return Vec::new();
    }

    let cache = CAMERA_MODE_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    let now = Instant::now();
    if let Ok(snapshot) = cache.lock() {
        if let Some((fetched, modes)) = snapshot.get(device_id) {
            if now.duration_since(*fetched) < CAMERA_MODE_CACHE_TTL {
                return modes.clone();
            }
        }
    }
    // The device list is cached separately. Mode lookup must open only the
    // selected Nokhwa camera, not rediscover every camera first.
    let modes = BuiltinPlugin::new()
        .ok()
        .and_then(|plugin| plugin.discover_platform_camera_modes(device_id).ok())
        .unwrap_or_default();
    if let Ok(mut snapshot) = cache.lock() {
        snapshot.insert(device_id.to_owned(), (Instant::now(), modes.clone()));
    }
    modes
}

fn scene(
    id: &str,
    name: &str,
    source_id: &str,
    color: &str,
    format: VideoFormat,
) -> Result<(SceneSpec, SourceSpec), Box<dyn Error>> {
    let mut settings = Config::new();
    settings.set("width", &format.width().to_string())?;
    settings.set("height", &format.height().to_string())?;
    settings.set("color", color)?;
    let mut scene = SceneSpec::new(id, name)?;
    scene.add_item(SceneItemSpec::for_source(source_id)?)?;
    let source = SourceSpec::new(source_id, "color_source", "Background", settings)?;
    Ok((scene, source))
}

fn default_video_format() -> Result<VideoFormat, Box<dyn Error>> {
    Ok(VideoFormat::new(
        DEFAULT_CANVAS_WIDTH,
        DEFAULT_CANVAS_HEIGHT,
        FrameRate::new(DEFAULT_CANVAS_FPS, 1)?,
    )?)
}
