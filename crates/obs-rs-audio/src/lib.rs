//! Safe audio values and the offline reference mixer for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod buffer;
mod callback_clock;
mod delay;
mod device;
mod error;
mod filters;
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
pub use delay::{AudioDelayLine, MAX_AUDIO_SYNC_OFFSET_MILLISECONDS};
pub use device::{
    AudioDeviceError, AudioDeviceInfo, AudioDeviceKind, AudioInput, AudioInputProvider,
    AudioInputState, AudioOutput, AudioOutputProvider, AudioOutputState, SharedAudioInputProvider,
    SharedAudioOutputProvider, SimulatedAudioInput, SimulatedAudioOutput, SimulatedAudioProvider,
};
pub use error::{AudioError, AudioWorkerError};
pub use filters::{
    AudioCompressor, AudioExpander, AudioFilter, AudioFilterChain, AudioGain, AudioLimiter,
    AudioNoiseGate, MAX_AUDIO_FILTERS, MAX_COMPRESSOR_ATTACK_MS,
    MAX_COMPRESSOR_OUTPUT_GAIN_DB_MILLI, MAX_COMPRESSOR_RATIO_MILLI, MAX_COMPRESSOR_RELEASE_MS,
    MAX_COMPRESSOR_THRESHOLD_DB_MILLI, MAX_EXPANDER_ATTACK_MS, MAX_EXPANDER_OUTPUT_GAIN_DB_MILLI,
    MAX_EXPANDER_RATIO_MILLI, MAX_EXPANDER_RELEASE_MS, MAX_EXPANDER_THRESHOLD_DB_MILLI,
    MAX_GAIN_DB_MILLI, MAX_LIMITER_RELEASE_MS, MAX_LIMITER_THRESHOLD_DB_MILLI,
    MAX_NOISE_GATE_THRESHOLD_DB_MILLI, MAX_NOISE_GATE_TIME_MS, MIN_COMPRESSOR_ATTACK_MS,
    MIN_COMPRESSOR_OUTPUT_GAIN_DB_MILLI, MIN_COMPRESSOR_RATIO_MILLI, MIN_COMPRESSOR_RELEASE_MS,
    MIN_COMPRESSOR_THRESHOLD_DB_MILLI, MIN_EXPANDER_ATTACK_MS, MIN_EXPANDER_OUTPUT_GAIN_DB_MILLI,
    MIN_EXPANDER_RATIO_MILLI, MIN_EXPANDER_RELEASE_MS, MIN_EXPANDER_THRESHOLD_DB_MILLI,
    MIN_GAIN_DB_MILLI, MIN_LIMITER_RELEASE_MS, MIN_LIMITER_THRESHOLD_DB_MILLI,
    MIN_NOISE_GATE_THRESHOLD_DB_MILLI, MIN_NOISE_GATE_TIME_MS,
};
pub use mixer::{AudioMixer, MAX_GAIN_MILLI, MAX_PAN_MILLI, MIN_PAN_MILLI};
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
