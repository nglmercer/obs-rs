//! Portable media values for the OBS-RS reference engine.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod delay;
mod error;
mod filters;
mod format;
mod frame;
mod frame_transitions;
mod metrics;
mod pixel;
mod scale;
mod time;
mod transform;
mod transition;

#[cfg(test)]
mod tests;

pub use delay::{
    RenderDelayBuffer, RenderDelayError, MAX_RENDER_DELAY_HISTORY_BYTES,
    MAX_RENDER_DELAY_HISTORY_FRAMES, MAX_RENDER_DELAY_MILLISECONDS, MIN_RENDER_DELAY_MILLISECONDS,
};
pub use error::MediaError;
pub use filters::{
    ChromaKey, ColorCorrection, ColorKey, ColorMultiplyAdd, FrameFilter, LumaKey, RenderDelay,
    MAX_SCROLL_SPEED, MIN_SCROLL_SPEED,
};
pub use format::VideoFormat;
pub use frame::VideoFrame;
pub use metrics::{
    frame_memory_metrics, reset_frame_memory_metrics, reset_thread_frame_memory_metrics,
    thread_frame_memory_metrics, FrameMemoryMetrics, LatencyMetrics,
};
pub use pixel::{PixelFormat, RawVideoFrame};
pub use scale::{FrameScaler, ScaleFilter};
pub use time::{sleep_precise, FrameRate, Timestamp, SLEEP_SPIN_WINDOW};
pub use transform::FrameTransform;
pub use transition::{
    parse_rgba8_hex, FrameTransition, SlideDirection, TransitionKind, TransitionSpec,
    DEFAULT_TRANSITION_DURATION_MILLIS, MAX_TRANSITION_DURATION_MILLIS,
    MIN_TRANSITION_DURATION_MILLIS,
};
