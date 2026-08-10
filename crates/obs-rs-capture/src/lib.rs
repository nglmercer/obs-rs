//! Rust-native capture contracts and a deterministic CPU test backend.
//!
//! Platform capture implementations can depend on this contract later. The
//! portable engine only sees owned frames, capabilities, and typed lifecycle errors.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "linux")]
pub use x11::X11CaptureDevice;

mod device;
mod error;
mod factories;
mod protocol;
mod provider;
mod settings;
mod simulated;
mod stream_device;
mod types;

#[cfg(test)]
mod tests;

pub use device::VideoCaptureDevice;
pub use error::CaptureError;
#[cfg(target_os = "linux")]
pub use factories::X11_SCREEN_CAPTURE_SOURCE_KIND;
pub use factories::{
    SimulatedCaptureFactory, TestPatternFactory, CAMERA_CAPTURE_SOURCE_KIND,
    SCREEN_CAPTURE_SOURCE_KIND, TEST_PATTERN_SOURCE_KIND, WINDOW_CAPTURE_SOURCE_KIND,
};
pub use protocol::{encode_frame_packet, FRAME_STREAM_MAGIC, MAX_FRAME_STREAM_PACKET_BYTES};
pub use provider::{CaptureProvider, PlatformCaptureProvider, SimulatedCaptureProvider};
pub use simulated::{SimulatedCaptureDevice, TestPatternDevice};
pub use stream_device::StreamCaptureDevice;
pub use types::{CaptureCatalog, CaptureDeviceInfo, CaptureEvent, CaptureKind, CapturePermission};
