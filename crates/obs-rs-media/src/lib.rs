//! Portable media values for the OBS-RS reference engine.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod error;
mod filters;
mod format;
mod frame;
mod pixel;
mod time;
mod transform;
mod transition;

#[cfg(test)]
mod tests;

pub use error::MediaError;
pub use filters::FrameFilter;
pub use format::VideoFormat;
pub use frame::VideoFrame;
pub use pixel::{PixelFormat, RawVideoFrame};
pub use time::{sleep_precise, FrameRate, Timestamp, SLEEP_SPIN_WINDOW};
pub use transform::FrameTransform;
pub use transition::FrameTransition;
