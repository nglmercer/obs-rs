use std::fmt;

use obs_rs_audio::{AudioError, AudioWorkerError};
use obs_rs_video::{VideoError, WorkerError};

use super::rate::MAX_CLOCK_DRIFT_PPM;
/// Errors raised while advancing one of the coordinated media timelines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimelineError {
    /// The video timeline could not advance.
    Video(VideoError),
    /// The audio timeline could not advance.
    Audio(AudioError),
}

impl fmt::Display for TimelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Video(error) => write!(formatter, "video timeline failed: {error}"),
            Self::Audio(error) => write!(formatter, "audio timeline failed: {error}"),
        }
    }
}

impl std::error::Error for TimelineError {}
/// Errors raised while configuring a deterministic device-clock model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockRateError {
    /// The requested rate would make the simulated clock non-positive or too
    /// different from the shared reference clock.
    DriftOutOfRange { ppm: i32 },
}

impl fmt::Display for ClockRateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DriftOutOfRange { ppm } => write!(
                formatter,
                "device clock drift {ppm} ppm is outside +/-{MAX_CLOCK_DRIFT_PPM} ppm"
            ),
        }
    }
}

impl std::error::Error for ClockRateError {}
/// Errors raised while a coordinated session advances one media domain.
#[derive(Debug, Eq, PartialEq)]
pub enum MediaSessionError<VE, AE> {
    /// The audio worker failed.
    Audio(AudioWorkerError<AE>),
    /// The video worker failed.
    Video(WorkerError<VE>),
}

impl<VE: fmt::Display, AE: fmt::Display> fmt::Display for MediaSessionError<VE, AE> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audio(error) => write!(formatter, "media session audio failed: {error}"),
            Self::Video(error) => write!(formatter, "media session video failed: {error}"),
        }
    }
}

impl<VE: fmt::Debug + fmt::Display, AE: fmt::Debug + fmt::Display> std::error::Error
    for MediaSessionError<VE, AE>
{
}
