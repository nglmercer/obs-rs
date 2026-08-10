#[cfg(target_os = "linux")]
use std::env;

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
/// Linux exposes a real local X11 screen descriptor when `DISPLAY` is present;
/// macOS and Windows keep a typed unavailable result until their safe Rust
/// capture adapters are supplied. Callers can therefore show capability state
/// instead of confusing a missing platform backend with an empty device list.
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
            let display =
                env::var("DISPLAY").map_err(|error| CaptureError::PlatformUnavailable {
                    message: format!("DISPLAY is unavailable: {error}"),
                })?;
            if display.trim().is_empty() {
                return Err(CaptureError::PlatformUnavailable {
                    message: "DISPLAY is empty".to_owned(),
                });
            }
            Ok(vec![CaptureDeviceInfo::new(
                "x11-screen-0",
                "X11 screen",
                CaptureKind::Screen,
            )?])
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

/// A deterministic discovery provider for the portable CPU fallback devices.
///
/// This provider is intentionally not an operating-system adapter. It gives the
/// runtime and UI a complete discovery contract while real screen/window/camera
/// providers are added behind [`CaptureProvider`].
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
