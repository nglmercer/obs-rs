//! Source plugins that work without a platform or device dependency.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::sync::Arc;

use obs_rs_capture::CaptureKind;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use obs_rs_capture::PlatformCaptureProvider;
#[cfg(target_os = "linux")]
use obs_rs_capture::{x11_monitors, x11_windows};
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

    /// Discovers only the platform devices needed by one source kind.
    ///
    /// Full platform discovery is intentionally comprehensive for diagnostics,
    /// but it is the wrong operation for a source-properties dialog: asking for
    /// camera modes should not enumerate every X11 window, and asking for a
    /// window should not open every camera just to query its formats.
    ///
    /// # Errors
    ///
    /// Returns the same typed discovery errors as the platform adapter.
    pub fn discover_platform_capture_devices_for_kind(
        &self,
        kind: CaptureKind,
    ) -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
        #[cfg(target_os = "linux")]
        {
            match kind {
                CaptureKind::Camera => Ok(obs_rs_capture::discover_nokhwa_camera_devices()?
                    .into_iter()
                    .map(obs_rs_capture::CameraDevice::into_info)
                    .collect()),
                CaptureKind::Screen => {
                    let display = std::env::var("DISPLAY").map_err(|error| {
                        CaptureError::PlatformUnavailable {
                            message: format!("DISPLAY is unavailable: {error}"),
                        }
                    })?;
                    let mut devices = vec![CaptureDeviceInfo::new(
                        "x11-screen-0",
                        "X11 screen (all monitors)",
                        CaptureKind::Screen,
                    )?];
                    let monitors = x11_monitors(&display)?;
                    if monitors.len() >= 2 {
                        devices.extend(monitors.into_iter().filter_map(|monitor| {
                            CaptureDeviceInfo::new(
                                &monitor.device_id(),
                                &monitor.label(),
                                CaptureKind::Screen,
                            )
                            .ok()
                        }));
                    }
                    Ok(devices)
                }
                CaptureKind::Window => {
                    let display = std::env::var("DISPLAY").map_err(|error| {
                        CaptureError::PlatformUnavailable {
                            message: format!("DISPLAY is unavailable: {error}"),
                        }
                    })?;
                    Ok(x11_windows(&display)?
                        .into_iter()
                        .filter_map(|window| {
                            CaptureDeviceInfo::new(
                                &window.device_id(),
                                &window.label(),
                                CaptureKind::Window,
                            )
                            .ok()
                        })
                        .collect())
                }
                CaptureKind::TestPattern | CaptureKind::External => Ok(Vec::new()),
            }
        }

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            if kind == CaptureKind::Camera {
                return Ok(obs_rs_capture::discover_nokhwa_camera_devices()?
                    .into_iter()
                    .map(obs_rs_capture::CameraDevice::into_info)
                    .collect());
            }
            Ok(self
                .discover_platform_capture_devices()?
                .into_iter()
                .filter(|device| device.kind() == kind)
                .collect())
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = kind;
            Ok(Vec::new())
        }
    }

    /// Queries native modes for one camera without enumerating unrelated
    /// devices.
    ///
    /// # Errors
    ///
    /// Returns a typed Nokhwa error when the selected camera is unavailable or
    /// its native capability query fails.
    pub fn discover_platform_camera_modes(
        &self,
        device_id: &str,
    ) -> Result<Vec<obs_rs_capture::CameraMode>, CaptureError> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            obs_rs_capture::discover_nokhwa_camera_modes(device_id)
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = device_id;
            Ok(Vec::new())
        }
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
