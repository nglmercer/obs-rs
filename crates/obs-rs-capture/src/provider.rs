#[cfg(target_os = "linux")]
use std::env;

#[cfg(target_os = "linux")]
use super::{adapter::PlatformCaptureAdapter, device::VideoCaptureDevice};
use super::{
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
/// and discovers capture-capable cameras through Nokhwa. macOS and Windows
/// expose their Nokhwa cameras here while their screen/window adapters remain
/// in the separate native-helper crates. Callers can therefore show capability
/// state instead of confusing a missing platform backend with an empty device
/// list.
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
            discover_nokhwa_platform_devices()
        }

        #[cfg(target_os = "windows")]
        {
            discover_nokhwa_platform_devices()
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Err(CaptureError::PlatformUnavailable {
                message: "no platform capture adapter is enabled for this target".to_owned(),
            })
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn discover_nokhwa_platform_devices() -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
    let devices = crate::discover_nokhwa_cameras()?
        .into_iter()
        .map(super::types::CameraDevice::into_info)
        .collect::<Vec<_>>();
    if devices.is_empty() {
        Err(CaptureError::PlatformUnavailable {
            message: "Nokhwa reported no native camera devices".to_owned(),
        })
    } else {
        Ok(devices)
    }
}

/// Linux platform boundary for portal/PipeWire, X11, and native camera capture.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxCaptureAdapter;

#[cfg(target_os = "linux")]
impl PlatformCaptureAdapter for LinuxCaptureAdapter {
    fn backend_name(&self) -> &'static str {
        "linux-portal-x11-nokhwa"
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
        devices.extend(discover_nokhwa_devices());
        if devices.is_empty() {
            return Err(CaptureError::PlatformUnavailable {
                message: "Wayland portal, DISPLAY, and native camera devices are unavailable"
                    .to_owned(),
            });
        }
        Ok(devices)
    }

    fn open(&self, stable_id: &str) -> Result<Box<dyn VideoCaptureDevice>, CaptureError> {
        if stable_id == "wayland-screen-picker" {
            return crate::WaylandCaptureDevice::open(stable_id, "Wayland display", None)
                .map(|device| Box::new(device) as Box<dyn VideoCaptureDevice>);
        }
        if stable_id.starts_with("v4l2-") || stable_id.starts_with("nokhwa-camera-") {
            let device = crate::NokhwaCaptureDevice::from_device_id(stable_id, stable_id)?;
            return Ok(Box::new(device));
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
fn discover_nokhwa_devices() -> Vec<CaptureDeviceInfo> {
    crate::discover_nokhwa_cameras()
        .unwrap_or_default()
        .into_iter()
        .map(super::types::CameraDevice::into_info)
        .collect::<Vec<_>>()
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
