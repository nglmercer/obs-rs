//! Source plugins that work without a platform or device dependency.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::sync::Arc;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use obs_rs_capture::CaptureKind;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use obs_rs_capture::PlatformCaptureProvider;
use obs_rs_capture::{CaptureDeviceInfo, CaptureError, CaptureProvider, SimulatedCaptureProvider};
use obs_rs_plugin_api::{Plugin, PluginError, PluginManifest, SourceFactory};

mod factories;
mod portable;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(test)]
mod tests;

/// Stable kind identifier for the solid color source.
pub const COLOR_SOURCE_KIND: &str = "color_source";
pub use obs_rs_capture::CAMERA_CAPTURE_SOURCE_KIND as BUILTIN_CAMERA_SOURCE_KIND;
pub use obs_rs_capture::SCREEN_CAPTURE_SOURCE_KIND as BUILTIN_SCREEN_SOURCE_KIND;
pub use obs_rs_capture::TEST_PATTERN_SOURCE_KIND as BUILTIN_TEST_PATTERN_SOURCE_KIND;
#[cfg(target_os = "linux")]
pub use obs_rs_capture::WAYLAND_SCREEN_CAPTURE_SOURCE_KIND as BUILTIN_WAYLAND_SCREEN_SOURCE_KIND;
pub use obs_rs_capture::WINDOW_CAPTURE_SOURCE_KIND as BUILTIN_WINDOW_SOURCE_KIND;
#[cfg(target_os = "linux")]
pub use obs_rs_capture::X11_SCREEN_CAPTURE_SOURCE_KIND as BUILTIN_X11_SCREEN_SOURCE_KIND;

/// The built-in plugin bundle shipped with the headless engine.
pub struct BuiltinPlugin {
    manifest: PluginManifest,
    factories: Vec<Arc<dyn SourceFactory>>,
}

impl BuiltinPlugin {
    /// Creates the portable built-in plugin bundle.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] if a built-in identifier or plugin manifest is invalid.
    pub fn new() -> Result<Self, PluginError> {
        let manifest = PluginManifest::new("obs_rs_builtins", "OBS-RS built-in sources", "0.1.0")?;
        Ok(Self {
            manifest,
            factories: factories::build()?,
        })
    }

    /// Discovers deterministic CPU fallback capture devices.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError`] if the built-in device descriptors are invalid.
    pub fn discover_capture_devices(&self) -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
        SimulatedCaptureProvider::new().discover()
    }

    /// Discovers host-platform devices through the platform provider seam.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError`] when the host platform adapter is unavailable.
    pub fn discover_platform_capture_devices(
        &self,
    ) -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
        #[cfg(target_os = "macos")]
        {
            let mut devices = obs_rs_capture_macos::MacOsCaptureAdapter::default().discover()?;
            // Screen/window capture stays on the native helper boundary, while
            // cameras use the shared Nokhwa capability model.
            devices.retain(|device| device.kind() != CaptureKind::Camera);
            devices.extend(
                obs_rs_capture::discover_nokhwa_cameras()
                    .unwrap_or_default()
                    .into_iter()
                    .map(obs_rs_capture::CameraDevice::into_info),
            );
            return Ok(devices);
        }
        #[cfg(target_os = "windows")]
        {
            let mut devices =
                obs_rs_capture_windows::WindowsCaptureAdapter::default().discover()?;
            // Screen/window capture stays on the native helper boundary, while
            // cameras use the shared Nokhwa capability model.
            devices.retain(|device| device.kind() != CaptureKind::Camera);
            devices.extend(
                obs_rs_capture::discover_nokhwa_cameras()
                    .unwrap_or_default()
                    .into_iter()
                    .map(obs_rs_capture::CameraDevice::into_info),
            );
            return Ok(devices);
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        PlatformCaptureProvider::new().discover()
    }
}

impl Plugin for BuiltinPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    fn source_factories(&self) -> &[Arc<dyn SourceFactory>] {
        &self.factories
    }
}
