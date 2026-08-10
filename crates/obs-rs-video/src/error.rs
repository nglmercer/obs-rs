use obs_rs_media::VideoFormat;
use std::fmt;
/// Errors raised by the video scheduler and queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoError {
    /// A queue must have at least one slot.
    ZeroCapacity,
    /// A frame does not match the queue's configured format.
    FormatMismatch {
        /// Format accepted by the queue.
        expected: VideoFormat,
        /// Format supplied by the caller.
        actual: VideoFormat,
    },
    /// The timestamp calculation or frame index would overflow.
    ScheduleOverflow,
    /// A multi-worker soak was requested without any workers.
    ZeroWorkers,
    /// A multi-worker soak exceeds the bounded worker limit.
    TooManyWorkers { workers: usize },
    /// A multi-worker soak worker terminated unexpectedly.
    WorkerPanic,
}

impl fmt::Display for VideoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => formatter.write_str("video queue capacity must be non-zero"),
            Self::FormatMismatch { expected, actual } => {
                write!(
                    formatter,
                    "video format {actual:?} does not match {expected:?}"
                )
            }
            Self::ScheduleOverflow => formatter.write_str("video schedule timestamp overflowed"),
            Self::ZeroWorkers => formatter.write_str("video worker count must be non-zero"),
            Self::TooManyWorkers { workers } => {
                write!(formatter, "video worker count is too large: {workers}")
            }
            Self::WorkerPanic => formatter.write_str("video soak worker terminated unexpectedly"),
        }
    }
}

impl std::error::Error for VideoError {}
/// Errors returned by one callback-driven render step.
#[derive(Debug, Eq, PartialEq)]
pub enum RenderError<E> {
    /// The scheduler could not produce a deadline.
    Schedule(VideoError),
    /// The source callback failed.
    Source(E),
    /// The callback produced a frame that the queue rejected.
    Submit(VideoError),
}

impl<E: fmt::Display> fmt::Display for RenderError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schedule(error) => write!(formatter, "video schedule failed: {error}"),
            Self::Source(error) => write!(formatter, "video source failed: {error}"),
            Self::Submit(error) => write!(formatter, "video submission failed: {error}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for RenderError<E> {}
/// Errors from a paced video worker.
#[derive(Debug, Eq, PartialEq)]
pub enum WorkerError<E> {
    /// The wall-clock pacer could not advance its timeline.
    Pacing(VideoError),
    /// The render callback or bounded submission failed.
    Render(RenderError<E>),
}

impl<E: fmt::Display> fmt::Display for WorkerError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pacing(error) => write!(formatter, "video worker pacing failed: {error}"),
            Self::Render(error) => write!(formatter, "video worker render failed: {error}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for WorkerError<E> {}
