use super::{buffer::AudioBuffer, error::AudioError};
use obs_rs_media::Timestamp;
use std::borrow::Cow;
/// Whether audio is aligned with, early relative to, or late relative to video.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncState {
    /// The two timestamps are within the configured tolerance.
    InSync,
    /// Audio is earlier than the video timestamp.
    AudioBehind,
    /// Audio is later than the video timestamp.
    AudioAhead,
}

/// Safe action suggested by one A/V synchronization observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncAction {
    /// Continue without changing either timeline.
    Keep,
    /// Discard or trim audio that is already behind the video timeline.
    DropEarlyAudio,
    /// Hold video or insert audio silence until later audio catches up.
    WaitForAudio,
}

/// The sample-level correction selected by an A/V observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioCorrection {
    /// Keep the buffer and its timestamp unchanged.
    Keep,
    /// Remove this many leading sample frames from an early buffer.
    TrimLeading { frames: usize },
    /// Prefix this many silence frames before a late buffer.
    InsertSilence { frames: usize },
}

/// One signed A/V timestamp comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AvSyncObservation {
    video_timestamp: Timestamp,
    audio_timestamp: Timestamp,
    delta_nanos: i64,
    state: SyncState,
    action: SyncAction,
}

impl AvSyncObservation {
    /// Returns the video timestamp used for comparison.
    #[must_use]
    pub const fn video_timestamp(self) -> Timestamp {
        self.video_timestamp
    }

    /// Returns the audio timestamp used for comparison.
    #[must_use]
    pub const fn audio_timestamp(self) -> Timestamp {
        self.audio_timestamp
    }

    /// Returns `audio - video` in nanoseconds, saturated to `i64`.
    #[must_use]
    pub const fn delta_nanos(self) -> i64 {
        self.delta_nanos
    }

    /// Returns the classified synchronization state.
    #[must_use]
    pub const fn state(self) -> SyncState {
        self.state
    }

    /// Returns the non-destructive correction recommendation.
    #[must_use]
    pub const fn action(self) -> SyncAction {
        self.action
    }
}

/// A deterministic A/V clock comparison policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AvSyncController {
    tolerance_nanos: u64,
}

/// Aggregated drift telemetry from a sequence of A/V observations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AvSyncMetrics {
    observations: u64,
    in_sync: u64,
    audio_behind: u64,
    audio_ahead: u64,
    max_abs_delta_nanos: u64,
    total_abs_delta_nanos: u64,
}

impl AvSyncMetrics {
    /// Returns the total number of observations.
    #[must_use]
    pub const fn observations(self) -> u64 {
        self.observations
    }

    /// Returns observations within tolerance.
    #[must_use]
    pub const fn in_sync(self) -> u64 {
        self.in_sync
    }

    /// Returns observations where audio was behind video.
    #[must_use]
    pub const fn audio_behind(self) -> u64 {
        self.audio_behind
    }

    /// Returns observations where audio was ahead of video.
    #[must_use]
    pub const fn audio_ahead(self) -> u64 {
        self.audio_ahead
    }

    /// Returns the largest absolute observed drift.
    #[must_use]
    pub const fn max_abs_delta_nanos(self) -> u64 {
        self.max_abs_delta_nanos
    }

    /// Returns the saturating sum of absolute observed drift.
    #[must_use]
    pub const fn total_abs_delta_nanos(self) -> u64 {
        self.total_abs_delta_nanos
    }
}

/// A bounded-memory accumulator for long-running A/V drift diagnostics.
pub struct AvSyncMonitor {
    controller: AvSyncController,
    metrics: AvSyncMetrics,
}

impl AvSyncMonitor {
    /// Creates an empty monitor with the supplied classification tolerance.
    #[must_use]
    pub const fn new(tolerance_nanos: u64) -> Self {
        Self {
            controller: AvSyncController::new(tolerance_nanos),
            metrics: AvSyncMetrics {
                observations: 0,
                in_sync: 0,
                audio_behind: 0,
                audio_ahead: 0,
                max_abs_delta_nanos: 0,
                total_abs_delta_nanos: 0,
            },
        }
    }

    /// Records and returns one classified observation.
    #[must_use]
    pub fn observe(
        &mut self,
        video_timestamp: Timestamp,
        audio_timestamp: Timestamp,
    ) -> AvSyncObservation {
        let observation = self.controller.observe(video_timestamp, audio_timestamp);
        self.metrics.observations = self.metrics.observations.saturating_add(1);
        match observation.state() {
            SyncState::InSync => {
                self.metrics.in_sync = self.metrics.in_sync.saturating_add(1);
            }
            SyncState::AudioBehind => {
                self.metrics.audio_behind = self.metrics.audio_behind.saturating_add(1);
            }
            SyncState::AudioAhead => {
                self.metrics.audio_ahead = self.metrics.audio_ahead.saturating_add(1);
            }
        }
        let absolute = observation.delta_nanos().unsigned_abs();
        self.metrics.max_abs_delta_nanos = self.metrics.max_abs_delta_nanos.max(absolute);
        self.metrics.total_abs_delta_nanos =
            self.metrics.total_abs_delta_nanos.saturating_add(absolute);
        observation
    }

    /// Returns the accumulated metrics without resetting the monitor.
    #[must_use]
    pub const fn metrics(&self) -> AvSyncMetrics {
        self.metrics
    }

    /// Clears accumulated observations while preserving the tolerance policy.
    pub fn reset(&mut self) {
        self.metrics = AvSyncMetrics::default();
    }
}

impl AvSyncController {
    /// Creates a controller with an in-sync tolerance in nanoseconds.
    #[must_use]
    pub const fn new(tolerance_nanos: u64) -> Self {
        Self { tolerance_nanos }
    }

    /// Compares one video and audio timestamp.
    #[must_use]
    pub fn observe(
        self,
        video_timestamp: Timestamp,
        audio_timestamp: Timestamp,
    ) -> AvSyncObservation {
        let signed_delta =
            i128::from(audio_timestamp.as_nanos()) - i128::from(video_timestamp.as_nanos());
        let delta_nanos = i64::try_from(signed_delta).unwrap_or_else(|_| {
            if signed_delta.is_negative() {
                i64::MIN
            } else {
                i64::MAX
            }
        });
        let tolerance = i128::from(self.tolerance_nanos);
        let (state, action) = if signed_delta < -tolerance {
            (SyncState::AudioBehind, SyncAction::DropEarlyAudio)
        } else if signed_delta > tolerance {
            (SyncState::AudioAhead, SyncAction::WaitForAudio)
        } else {
            (SyncState::InSync, SyncAction::Keep)
        };
        AvSyncObservation {
            video_timestamp,
            audio_timestamp,
            delta_nanos,
            state,
            action,
        }
    }

    /// Reconciles one audio buffer against a video timestamp.
    ///
    /// A buffer that begins materially before video has its leading samples
    /// trimmed. A buffer that begins materially after video receives explicit
    /// silence at the video timestamp. If trimming consumes the entire buffer,
    /// the result is `Ok(None)` and the caller may drop it.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ScheduleOverflow`] if the sample-count conversion or
    /// corrected timestamp cannot be represented.
    pub fn reconcile(
        self,
        video_timestamp: Timestamp,
        buffer: &AudioBuffer,
    ) -> Result<Option<Cow<'_, AudioBuffer>>, AudioError> {
        let observation = self.observe(video_timestamp, buffer.timestamp());
        let correction = correction_for(observation, buffer.format().sample_rate())?;
        match correction {
            // Already in sync: hand back the caller's buffer rather than
            // copying a payload that no correction would change.
            AudioCorrection::Keep => Ok(Some(Cow::Borrowed(buffer))),
            AudioCorrection::TrimLeading { frames } => {
                Ok(buffer.trim_front(frames)?.map(Cow::Owned))
            }
            AudioCorrection::InsertSilence { frames } => Ok(Some(Cow::Owned(
                buffer.prepend_silence(frames, video_timestamp)?,
            ))),
        }
    }

    /// Reconciles one audio buffer in place against a video timestamp.
    ///
    /// Equivalent to [`AvSyncController::reconcile`] but reuses `buffer`'s
    /// allocation for every correction, so an in-sync buffer costs nothing and a
    /// trim or silence insert grows or shrinks the existing payload. Returns
    /// `Ok(false)` when trimming would consume the whole buffer, in which case
    /// `buffer` is left unchanged and the caller may drop it.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ScheduleOverflow`] if the sample-count conversion or
    /// corrected timestamp cannot be represented.
    pub fn reconcile_in_place(
        self,
        video_timestamp: Timestamp,
        buffer: &mut AudioBuffer,
    ) -> Result<bool, AudioError> {
        let observation = self.observe(video_timestamp, buffer.timestamp());
        let correction = correction_for(observation, buffer.format().sample_rate())?;
        match correction {
            AudioCorrection::Keep => Ok(true),
            AudioCorrection::TrimLeading { frames } => buffer.trim_front_in_place(frames),
            AudioCorrection::InsertSilence { frames } => {
                buffer.prepend_silence_in_place(frames, video_timestamp)?;
                Ok(true)
            }
        }
    }

    /// Returns the configured in-sync tolerance.
    #[must_use]
    pub const fn tolerance_nanos(self) -> u64 {
        self.tolerance_nanos
    }
}

fn correction_for(
    observation: AvSyncObservation,
    sample_rate: u32,
) -> Result<AudioCorrection, AudioError> {
    let frames = usize::try_from(frames_for_nanos(
        observation.delta_nanos().unsigned_abs(),
        sample_rate,
    )?)
    .map_err(|_| AudioError::ScheduleOverflow)?;
    Ok(match observation.state() {
        SyncState::InSync => AudioCorrection::Keep,
        SyncState::AudioBehind => AudioCorrection::TrimLeading { frames },
        SyncState::AudioAhead => AudioCorrection::InsertSilence { frames },
    })
}

fn frames_for_nanos(nanoseconds: u64, sample_rate: u32) -> Result<u128, AudioError> {
    let numerator = u128::from(nanoseconds)
        .checked_mul(u128::from(sample_rate))
        .and_then(|value| value.checked_add(999_999_999))
        .ok_or(AudioError::ScheduleOverflow)?;
    Ok(numerator / 1_000_000_000)
}
