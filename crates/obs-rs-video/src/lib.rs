//! Deterministic video scheduling and bounded frame transport.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod clock;
mod error;
mod pipeline;
mod queue;
mod soak;
mod types;
mod worker;

#[cfg(test)]
mod tests;

pub use clock::{
    DeadlineObservation, FrameDeadline, MonotonicClock, PacingResult, VideoClock, VideoPacer,
    VideoScheduler,
};
pub use error::{RenderError, VideoError, WorkerError};
pub use pipeline::{RenderOutcome, SustainedRunReport, VideoMetrics, VideoPipeline};
pub use queue::FrameQueue;
pub use soak::{run_multi_worker_soak, MultiWorkerSoakReport, MAX_SOAK_WORKERS};
pub use types::{DropPolicy, PushOutcome};
pub use worker::{CancellationToken, VideoWorker, VideoWorkerReport};
