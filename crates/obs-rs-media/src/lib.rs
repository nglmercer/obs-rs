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
mod stinger;
mod stinger_loader;
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
pub use stinger::{
    StingerClip, StingerSpec, MAX_STINGER_DURATION_NANOS, MAX_STINGER_FRAMES,
    MAX_STINGER_FRAME_DURATION_NANOS, MAX_STINGER_MEMORY_BYTES, MAX_STINGER_RESOURCE_PATH_BYTES,
    MAX_STINGER_TRANSITION_POINT_MILLI, MIN_STINGER_FRAME_DURATION_NANOS,
    MIN_STINGER_TRANSITION_POINT_MILLI,
};
pub use stinger_loader::{
    StingerLoadCancellation, StingerLoadQueueError, StingerLoadRequest, StingerLoadResult,
    StingerLoadWorker, StingerResourceLoader, STINGER_LOAD_QUEUE_CAPACITY,
    STINGER_LOAD_RESULT_CAPACITY,
};
pub use time::{sleep_precise, FrameRate, Timestamp, SLEEP_SPIN_WINDOW};
pub use transform::FrameTransform;
pub use transition::{
    parse_rgba8_hex, FrameTransition, LumaWipePattern, SlideDirection, TransitionKind,
    TransitionSpec, DEFAULT_LUMA_WIPE_SOFTNESS_MILLI, DEFAULT_TRANSITION_DURATION_MILLIS,
    MAX_LUMA_WIPE_SOFTNESS_MILLI, MAX_TRANSITION_DURATION_MILLIS, MIN_LUMA_WIPE_SOFTNESS_MILLI,
    MIN_TRANSITION_DURATION_MILLIS,
};
