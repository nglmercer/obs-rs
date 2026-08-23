//! Rust-native capture contracts and a deterministic CPU test backend.
//!
//! Platform capture implementations use this contract while the portable
//! engine only sees owned frames, capabilities, and typed lifecycle errors.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

#[cfg(target_os = "linux")]
mod dbus;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod nokhwa_camera;
#[cfg(target_os = "linux")]
mod raw_reader;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "linux")]
pub use dbus::{open_screencast, CursorMode, ScreenCastSession};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use nokhwa_camera::{
    discover_nokhwa_camera_devices, discover_nokhwa_camera_modes, discover_nokhwa_cameras,
    NokhwaCaptureDevice,
};
#[cfg(target_os = "linux")]
pub use raw_reader::RawFrameReader;
#[cfg(target_os = "linux")]
pub use wayland::{
    pipewire_reader_available, wayland_session_available, WaylandCaptureDevice,
    PIPEWIRE_READER_COMMAND,
};
#[cfg(target_os = "linux")]
pub use x11::{
    parse_window_id, x11_monitors, x11_windows, X11CaptureDevice, X11Monitor, X11Window,
};

mod adapter;
mod device;
mod error;
mod factories;
mod lifecycle;
mod protocol;
mod provider;
mod settings;
mod simulated;
mod stream_device;
mod threaded;
mod types;

#[cfg(test)]
mod tests;

pub use adapter::PlatformCaptureAdapter;
pub use device::{CaptureRequest, VideoCaptureDevice};
pub use error::CaptureError;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use factories::NokhwaCaptureFactory;
pub use factories::{
    SimulatedCaptureFactory, TestPatternFactory, CAMERA_CAPTURE_SOURCE_KIND,
    SCREEN_CAPTURE_SOURCE_KIND, TEST_PATTERN_SOURCE_KIND, WINDOW_CAPTURE_SOURCE_KIND,
};
#[cfg(target_os = "linux")]
pub use factories::{
    WAYLAND_SCREEN_CAPTURE_SOURCE_KIND, X11_SCREEN_CAPTURE_SOURCE_KIND,
    X11_WINDOW_CAPTURE_SOURCE_KIND,
};
pub use lifecycle::{AsyncCaptureDevice, CaptureLifecycleState};
pub use protocol::{
    encode_frame_packet, write_frame_packet, FRAME_STREAM_MAGIC, MAX_FRAME_STREAM_PACKET_BYTES,
};
#[cfg(target_os = "linux")]
pub use provider::LinuxCaptureAdapter;
pub use provider::{CaptureProvider, PlatformCaptureProvider, SimulatedCaptureProvider};
pub use simulated::{SimulatedCaptureDevice, TestPatternDevice};
pub use stream_device::StreamCaptureDevice;
pub use threaded::ThreadedCaptureDevice;
pub use types::{
    CameraDevice, CameraMode, CameraPixelFormat, CaptureBackendCapabilities, CaptureCapabilities,
    CaptureCatalog, CaptureDeviceInfo, CaptureDeviceState, CaptureEvent, CaptureKind,
    CapturePermission, CaptureTarget,
};
