#[cfg(target_os = "linux")]
use std::{env, fs, process::Command};

use super::{
    adapter::PlatformCaptureAdapter,
    device::VideoCaptureDevice,
    error::CaptureError,
    types::{CaptureCatalog, CaptureDeviceInfo, CaptureKind},
};

/// Safe discovery boundary implemented by platform and fallback providers.
pub trait CaptureProvider {
    /// Returns one complete discovery snapshot.
    ///
    /// # Errors
    ///
    /// Returns a provider-specific [`CaptureError`] when discovery cannot produce
    /// a valid descriptor set.
    fn discover(&self) -> Result<Vec<CaptureDeviceInfo>, CaptureError>;

    /// Replaces a catalog with the provider's latest snapshot.
    ///
    /// # Errors
    ///
    /// Propagates discovery or duplicate-ID validation errors.
    fn refresh(&self, catalog: &mut CaptureCatalog) -> Result<(), CaptureError> {
        catalog.replace_all(self.discover()?)
    }
}

/// A host-platform discovery provider with an explicit unavailable fallback.
///
/// Linux exposes a real local X11 screen descriptor when `DISPLAY` is present
/// and discovers capture-capable V4L2 camera nodes. macOS and Windows keep a
/// typed unavailable result until their safe Rust capture adapters are
/// supplied. Callers can therefore show capability state instead of confusing
/// a missing platform backend with an empty device list.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlatformCaptureProvider;

impl PlatformCaptureProvider {
    /// Creates the host-platform provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl CaptureProvider for PlatformCaptureProvider {
    fn discover(&self) -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
        #[cfg(target_os = "linux")]
        {
            LinuxCaptureAdapter.discover()
        }

        #[cfg(target_os = "macos")]
        {
            Err(CaptureError::PlatformUnavailable {
                message: "macOS ScreenCaptureKit adapter is not enabled".to_owned(),
            })
        }

        #[cfg(target_os = "windows")]
        {
            Err(CaptureError::PlatformUnavailable {
                message: "Windows Graphics Capture adapter is not enabled".to_owned(),
            })
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Err(CaptureError::PlatformUnavailable {
                message: "no platform capture adapter is enabled for this target".to_owned(),
            })
        }
    }
}

/// Linux platform boundary for portal/PipeWire, X11, and V4L2 capture.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxCaptureAdapter;

#[cfg(target_os = "linux")]
impl PlatformCaptureAdapter for LinuxCaptureAdapter {
    fn backend_name(&self) -> &'static str {
        "linux-portal-x11-v4l2"
    }

    fn discover(&self) -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
        let mut devices = Vec::new();
        if crate::wayland::wayland_session_available() {
            let mut portal = CaptureDeviceInfo::new(
                "wayland-screen-picker",
                "Wayland display picker",
                CaptureKind::Screen,
            )?;
            portal.set_permission(super::types::CapturePermission::PromptRequired);
            devices.push(portal);
        }
        if let Ok(display) = env::var("DISPLAY") {
            if !display.trim().is_empty() {
                devices.push(CaptureDeviceInfo::new(
                    "x11-screen-0",
                    "X11 screen (all monitors)",
                    CaptureKind::Screen,
                )?);
                devices.extend(discover_x11_monitors(&display));
                devices.extend(discover_x11_windows(&display));
            }
        }
        devices.extend(discover_v4l2_devices());
        if devices.is_empty() {
            return Err(CaptureError::PlatformUnavailable {
                message: "Wayland portal, DISPLAY, and /dev/video* are unavailable".to_owned(),
            });
        }
        Ok(devices)
    }

    fn open(&self, stable_id: &str) -> Result<Box<dyn VideoCaptureDevice>, CaptureError> {
        if stable_id == "wayland-screen-picker" {
            return crate::WaylandCaptureDevice::open(stable_id, "Wayland display", None)
                .map(|device| Box::new(device) as Box<dyn VideoCaptureDevice>);
        }
        if let Some(node) = stable_id.strip_prefix("v4l2-") {
            if !node.starts_with("video") || !node[5..].bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(CaptureError::InvalidDevice {
                    reason: "V4L2 stable ID is invalid".to_owned(),
                });
            }
            let path = std::path::PathBuf::from("/dev").join(node);
            return crate::V4l2CaptureDevice::new(stable_id, stable_id, path)
                .map(|device| Box::new(device) as Box<dyn VideoCaptureDevice>);
        }
        let display = env::var("DISPLAY").map_err(|error| CaptureError::PlatformUnavailable {
            message: format!("DISPLAY is unavailable: {error}"),
        })?;
        if stable_id == "x11-screen-0" {
            return crate::X11CaptureDevice::connect(&display, stable_id, "X11 screen")
                .map(|device| Box::new(device) as Box<dyn VideoCaptureDevice>);
        }
        if stable_id.starts_with("x11-monitor-") {
            let monitor = crate::x11_monitors(&display)?
                .into_iter()
                .find(|monitor| monitor.device_id() == stable_id)
                .ok_or_else(|| CaptureError::InvalidDevice {
                    reason: format!("monitor {stable_id} is unavailable"),
                })?;
            let mut device =
                crate::X11CaptureDevice::connect(&display, stable_id, &monitor.label())?;
            device.select_monitor(Some(monitor.name()))?;
            return Ok(Box::new(device));
        }
        if stable_id.starts_with("x11-window-") {
            let window = crate::x11_windows(&display)?
                .into_iter()
                .find(|window| window.device_id() == stable_id)
                .ok_or_else(|| CaptureError::InvalidDevice {
                    reason: format!("window {stable_id} is unavailable"),
                })?;
            let mut device =
                crate::X11CaptureDevice::connect(&display, stable_id, &window.label())?;
            device.select_window(Some(stable_id))?;
            return Ok(Box::new(device));
        }
        Err(CaptureError::InvalidDevice {
            reason: format!("stable capture ID {stable_id} is unknown"),
        })
    }
}

/// Lists one screen descriptor per active `RandR` monitor.
///
/// A multi-head desktop is one X11 screen, so without this the UI could only
/// ever offer "the whole desktop". Discovery failures degrade to no extra
/// entries: the full-desktop descriptor already covers the fallback.
#[cfg(target_os = "linux")]
fn discover_x11_monitors(display: &str) -> Vec<CaptureDeviceInfo> {
    let Ok(monitors) = crate::x11::x11_monitors(display) else {
        return Vec::new();
    };
    // A single monitor is the whole root window, which is already listed.
    if monitors.len() < 2 {
        return Vec::new();
    }
    monitors
        .iter()
        .filter_map(|monitor| {
            CaptureDeviceInfo::new(&monitor.device_id(), &monitor.label(), CaptureKind::Screen).ok()
        })
        .collect()
}

/// Lists one window descriptor per capturable top-level X11 window.
///
/// Discovery failures degrade to no entries rather than to an error: a session
/// with no window manager running still has a usable desktop capture, and a
/// picker that is merely empty is better than one that refuses to open.
#[cfg(target_os = "linux")]
fn discover_x11_windows(display: &str) -> Vec<CaptureDeviceInfo> {
    let Ok(windows) = crate::x11::x11_windows(display) else {
        return Vec::new();
    };
    windows
        .iter()
        .filter_map(|window| {
            CaptureDeviceInfo::new(&window.device_id(), &window.label(), CaptureKind::Window).ok()
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn discover_v4l2_devices() -> Vec<CaptureDeviceInfo> {
    let Ok(entries) = fs::read_dir("/dev") else {
        return Vec::new();
    };
    let mut devices = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let suffix = name.strip_prefix("video")?;
            if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let label = v4l2_capture_label(&path)?;
            let id = format!("v4l2-{name}");
            let label = format!("{label} ({})", path.display());
            CaptureDeviceInfo::new(&id, &label, CaptureKind::Camera).ok()
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.id().cmp(right.id()));
    devices
}

#[cfg(target_os = "linux")]
fn v4l2_capture_label(path: &std::path::Path) -> Option<String> {
    // `video4linux` also exposes metadata-only nodes. Ask the userspace
    // utility for capabilities so those nodes never become misleading camera
    // choices in the GUI. If the utility is absent, the deterministic camera
    // fallback remains available through SimulatedCaptureProvider.
    let output = Command::new("v4l2-ctl")
        .args(["--device", path.to_string_lossy().as_ref(), "--all"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let document = String::from_utf8_lossy(&output.stdout);
    if !document.contains("Format Video Capture") {
        return None;
    }
    document
        .lines()
        .find_map(|line| line.trim().strip_prefix("Card type")?.split_once(':'))
        .map(|(_, value)| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// A deterministic discovery provider for the portable CPU fallback devices.
///
/// This provider is intentionally not an operating-system adapter. It gives the
/// runtime and UI a complete discovery contract while portable screen/window
/// and camera fallback sources remain available behind [`CaptureProvider`].
#[derive(Clone, Copy, Debug, Default)]
pub struct SimulatedCaptureProvider;

impl SimulatedCaptureProvider {
    /// Creates the deterministic fallback provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl CaptureProvider for SimulatedCaptureProvider {
    fn discover(&self) -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
        Ok(vec![
            CaptureDeviceInfo::new("test-pattern", "Test pattern", CaptureKind::TestPattern)?,
            CaptureDeviceInfo::new("screen-0", "Simulated screen", CaptureKind::Screen)?,
            CaptureDeviceInfo::new("window-0", "Simulated window", CaptureKind::Window)?,
            CaptureDeviceInfo::new("camera-0", "Simulated camera", CaptureKind::Camera)?,
        ])
    }
}
