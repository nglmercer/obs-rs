//! Rust-native capture contracts and a deterministic CPU test backend.
//!
//! Platform capture implementations use this contract while the portable
//! engine only sees owned frames, capabilities, and typed lifecycle errors.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

#[cfg(target_os = "linux")]
mod dbus;
#[cfg(target_os = "linux")]
mod raw_reader;
#[cfg(target_os = "linux")]
mod v4l2;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "linux")]
pub use dbus::{open_screencast, CursorMode, ScreenCastSession};
#[cfg(target_os = "linux")]
pub use raw_reader::RawFrameReader;
#[cfg(target_os = "linux")]
pub use v4l2::V4l2CaptureDevice;
#[cfg(target_os = "linux")]
pub use wayland::{wayland_session_available, WaylandCaptureDevice};
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
mod types;

#[cfg(test)]
mod tests;

pub use adapter::PlatformCaptureAdapter;
pub use device::VideoCaptureDevice;
pub use error::CaptureError;
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
pub use types::{
    CaptureCapabilities, CaptureCatalog, CaptureDeviceInfo, CaptureEvent, CaptureKind,
    CapturePermission,
};
