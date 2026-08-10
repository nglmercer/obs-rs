//! Shared, safe audio/video session timing for OBS-RS.

//!
//! This crate owns the relationship between independent sample and frame
//! timelines. Platform device clocks can be adapted to the `AudioClock` and
//! `VideoClock` traits without changing the portable coordinator.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod cancellation;
mod clock;
mod error;
mod rate;
mod report;
mod session;
mod timeline;

#[cfg(test)]
mod tests;

pub use cancellation::SessionCancellationToken;
pub use clock::MonotonicMediaClock;
pub use error::{ClockRateError, MediaSessionError, TimelineError};
pub use rate::{ClockRate, IndependentMediaClock, MAX_CLOCK_DRIFT_PPM};
pub use report::MediaSessionReport;
pub use session::MediaSession;
pub use timeline::MediaTimeline;
