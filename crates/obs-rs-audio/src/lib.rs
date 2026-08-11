//! Safe audio values and the offline reference mixer for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod buffer;
mod callback_clock;
mod device;
mod error;
mod mixer;
mod monitor;
mod pacing;
mod queue;
mod resampler;
mod sync;
mod types;
mod worker;

#[cfg(test)]
mod tests;

pub use buffer::{AudioBuffer, AudioBufferPool};
pub use callback_clock::{AudioCallbackClock, AudioCallbackObservation};
pub use device::{
    AudioDeviceError, AudioDeviceInfo, AudioDeviceKind, AudioInput, AudioInputProvider,
    AudioInputState, SharedAudioInputProvider, SimulatedAudioInput, SimulatedAudioProvider,
};
pub use error::{AudioError, AudioWorkerError};
pub use mixer::AudioMixer;
pub use monitor::AudioMonitorTap;
pub use pacing::{
    AudioClock, AudioDeadline, AudioPacer, AudioPacingResult, AudioScheduler, MonotonicAudioClock,
};
pub use queue::{AudioDropPolicy, AudioPushOutcome, AudioQueue};
pub use resampler::AudioResampler;
pub use sync::{
    AudioCorrection, AvSyncController, AvSyncMetrics, AvSyncMonitor, AvSyncObservation, SyncAction,
    SyncState,
};
pub use types::{
    AudioFormat, AudioMonitorTapId, AudioSourceId, MAX_AUDIO_FRAMES, MAX_CALLBACK_CORRECTION_PPM,
};
pub use worker::{AudioCancellationToken, AudioWorker, AudioWorkerReport};
