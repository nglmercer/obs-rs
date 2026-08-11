#[cfg(target_os = "linux")]
use std::{env, fs, process::Command};

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
            let mut devices = Vec::new();
            if let Ok(display) = env::var("DISPLAY") {
                if !display.trim().is_empty() {
                    devices.push(CaptureDeviceInfo::new(
                        "x11-screen-0",
                        "X11 screen",
                        CaptureKind::Screen,
                    )?);
                }
            }
            devices.extend(discover_v4l2_devices());
            if devices.is_empty() {
                return Err(CaptureError::PlatformUnavailable {
                    message: "DISPLAY and /dev/video* are unavailable".to_owned(),
                });
            }
            Ok(devices)
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
