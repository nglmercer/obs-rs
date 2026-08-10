//! Safe audio values and the offline reference mixer for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use obs_rs_media::Timestamp;

/// Maximum number of interleaved audio frames in one owned buffer.
pub const MAX_AUDIO_FRAMES: usize = 1_048_576;

/// A validated interleaved audio format.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AudioFormat {
    sample_rate: u32,
    channels: u16,
}

impl AudioFormat {
    /// Creates a format with a positive sample rate and channel count.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidFormat`] when either value is zero.
    pub const fn new(sample_rate: u32, channels: u16) -> Result<Self, AudioError> {
        if sample_rate == 0 || channels == 0 {
            return Err(AudioError::InvalidFormat);
        }
        Ok(Self {
            sample_rate,
            channels,
        })
    }

    /// Returns samples per second.
    #[must_use]
    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    /// Returns the number of interleaved channels.
    #[must_use]
    pub const fn channels(self) -> u16 {
        self.channels
    }
}

/// A stable handle for a mixer source.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AudioSourceId(u64);

impl AudioSourceId {
    /// Returns the numeric value for logs and fixtures.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// A stable handle for a bounded post-mix monitoring tap.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AudioMonitorTapId(u64);

impl AudioMonitorTapId {
    /// Returns the numeric value for logs and fixtures.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// A bounded queue of cloned post-mix buffers for monitoring or diagnostics.
pub struct AudioMonitorTap {
    capacity_buffers: usize,
    dropped_buffers: u64,
    buffers: VecDeque<AudioBuffer>,
}

impl AudioMonitorTap {
    /// Creates a tap that retains at most `capacity_buffers` complete buffers.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroMonitorCapacity`] for a zero capacity.
    pub fn new(capacity_buffers: usize) -> Result<Self, AudioError> {
        if capacity_buffers == 0 {
            return Err(AudioError::ZeroMonitorCapacity);
        }
        Ok(Self {
            capacity_buffers,
            dropped_buffers: 0,
            buffers: VecDeque::with_capacity(capacity_buffers),
        })
    }

    fn observe(&mut self, buffer: &AudioBuffer) {
        if self.buffers.len() == self.capacity_buffers {
            let _ = self.buffers.pop_front();
            self.dropped_buffers = self.dropped_buffers.saturating_add(1);
        }
        self.buffers.push_back(buffer.clone());
    }

    /// Removes and returns the oldest monitored buffer.
    pub fn pop(&mut self) -> Option<AudioBuffer> {
        self.buffers.pop_front()
    }

    /// Returns the number of retained monitored buffers.
    #[must_use]
    pub fn queued_buffers(&self) -> usize {
        self.buffers.len()
    }

    /// Returns the number of buffers discarded due to tap pressure.
    #[must_use]
    pub const fn dropped_buffers(&self) -> u64 {
        self.dropped_buffers
    }
}

/// An owned interleaved `f32` audio buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    format: AudioFormat,
    timestamp: Timestamp,
    samples: Vec<f32>,
}

/// A deterministic linear resampler for interleaved buffers with equal channels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioResampler {
    input: AudioFormat,
    output: AudioFormat,
}

/// One exact sample-clock deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioDeadline {
    index: u64,
    timestamp: Timestamp,
}

impl AudioDeadline {
    /// Returns the zero-based sample-frame index.
    #[must_use]
    pub const fn index(self) -> u64 {
        self.index
    }

    /// Returns the exact integer nanosecond timestamp for the frame.
    #[must_use]
    pub const fn timestamp(self) -> Timestamp {
        self.timestamp
    }
}

/// A rational sample-clock scheduler without floating-point drift.
pub struct AudioScheduler {
    format: AudioFormat,
    next_index: u64,
}

/// Clock operations needed by the portable audio pacer.
pub trait AudioClock {
    /// Returns elapsed monotonic time from the clock's origin.
    fn now(&self) -> Timestamp;

    /// Waits until `deadline`, returning immediately when it has already passed.
    fn sleep_until(&mut self, deadline: Timestamp);
}

/// A monotonic wall-clock origin for audio callback integration.
pub struct MonotonicAudioClock {
    origin: Instant,
}

/// The result of waiting for one audio block deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioPacingResult {
    deadline: AudioDeadline,
    frames: usize,
    requested_at: Timestamp,
    observed_at: Timestamp,
    waited_nanos: u64,
}

impl AudioPacingResult {
    /// Returns the first sample-frame deadline in the block.
    #[must_use]
    pub const fn deadline(self) -> AudioDeadline {
        self.deadline
    }

    /// Returns the number of sample frames represented by the block.
    #[must_use]
    pub const fn frames(self) -> usize {
        self.frames
    }

    /// Returns the clock reading before waiting.
    #[must_use]
    pub const fn requested_at(self) -> Timestamp {
        self.requested_at
    }

    /// Returns the clock reading after waiting.
    #[must_use]
    pub const fn observed_at(self) -> Timestamp {
        self.observed_at
    }

    /// Returns elapsed time spent waiting.
    #[must_use]
    pub const fn waited_nanos(self) -> u64 {
        self.waited_nanos
    }

    /// Returns whether the block was observed after its first-sample deadline.
    #[must_use]
    pub const fn missed(self) -> bool {
        self.observed_at.as_nanos() > self.deadline.timestamp().as_nanos()
    }

    /// Returns lateness after the first-sample deadline, or zero when on time.
    #[must_use]
    pub const fn lateness_nanos(self) -> u64 {
        self.observed_at
            .as_nanos()
            .saturating_sub(self.deadline.timestamp().as_nanos())
    }
}

/// A block-based sample-clock pacer with an injected wall clock.
pub struct AudioPacer {
    scheduler: AudioScheduler,
}

/// A thread-safe cancellation flag checked between audio blocks.
#[derive(Clone, Debug)]
pub struct AudioCancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl Default for AudioCancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCancellationToken {
    /// Creates an uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Requests cancellation before the next block begins.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Clears a previous cancellation request for reuse.
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Delta diagnostics from one paced audio worker run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AudioWorkerReport {
    requested_blocks: u64,
    processed_blocks: u64,
    cancelled: bool,
    underflow_blocks: u64,
    produced_frames: u64,
    dropped_oldest_frames: u64,
    dropped_newest_frames: u64,
    missed_deadlines: u64,
    total_lateness_nanos: u64,
    remaining_queue_frames: usize,
}

impl AudioWorkerReport {
    /// Returns the number of blocks requested from the worker.
    #[must_use]
    pub const fn requested_blocks(self) -> u64 {
        self.requested_blocks
    }

    /// Returns the number of paced blocks completed.
    #[must_use]
    pub const fn processed_blocks(self) -> u64 {
        self.processed_blocks
    }

    /// Returns whether cancellation stopped the run before its requested count.
    #[must_use]
    pub const fn cancelled(self) -> bool {
        self.cancelled
    }

    /// Returns the number of blocks for which the producer returned no audio.
    #[must_use]
    pub const fn underflow_blocks(self) -> u64 {
        self.underflow_blocks
    }

    /// Returns the number of produced sample frames, including dropped output.
    #[must_use]
    pub const fn produced_frames(self) -> u64 {
        self.produced_frames
    }

    /// Returns the number of old sample frames removed under drop-oldest pressure.
    #[must_use]
    pub const fn dropped_oldest_frames(self) -> u64 {
        self.dropped_oldest_frames
    }

    /// Returns the number of submitted sample frames discarded under drop-newest pressure.
    #[must_use]
    pub const fn dropped_newest_frames(self) -> u64 {
        self.dropped_newest_frames
    }

    /// Returns the number of post-callback deadlines observed late.
    #[must_use]
    pub const fn missed_deadlines(self) -> u64 {
        self.missed_deadlines
    }

    /// Returns total post-callback lateness in nanoseconds.
    #[must_use]
    pub const fn total_lateness_nanos(self) -> u64 {
        self.total_lateness_nanos
    }

    /// Returns the number of sample frames remaining in the output queue.
    #[must_use]
    pub const fn remaining_queue_frames(self) -> usize {
        self.remaining_queue_frames
    }
}

/// Errors from a paced audio worker.
#[derive(Debug, Eq, PartialEq)]
pub enum AudioWorkerError<E> {
    /// The sample-clock pacer could not advance its timeline.
    Pacing(AudioError),
    /// The producer callback failed.
    Source(E),
    /// The producer returned a buffer that violated the worker contract.
    Submit(AudioError),
}

impl<E: fmt::Display> fmt::Display for AudioWorkerError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pacing(error) => write!(formatter, "audio worker pacing failed: {error}"),
            Self::Source(error) => write!(formatter, "audio source failed: {error}"),
            Self::Submit(error) => write!(formatter, "audio submission failed: {error}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for AudioWorkerError<E> {}

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
    ) -> Result<Option<AudioBuffer>, AudioError> {
        let observation = self.observe(video_timestamp, buffer.timestamp());
        let correction = correction_for(observation, buffer.format().sample_rate())?;
        match correction {
            AudioCorrection::Keep => Ok(Some(buffer.clone())),
            AudioCorrection::TrimLeading { frames } => buffer.trim_front(frames),
            AudioCorrection::InsertSilence { frames } => {
                Ok(Some(buffer.prepend_silence(frames, video_timestamp)?))
            }
        }
    }

    /// Returns the configured in-sync tolerance.
    #[must_use]
    pub const fn tolerance_nanos(self) -> u64 {
        self.tolerance_nanos
    }
}

impl AudioBuffer {
    /// Creates a buffer after checking interleaving and sample finiteness.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError`] when the sample count is not divisible by the channel
    /// count, a sample is non-finite, or the buffer exceeds the frame limit.
    pub fn new(
        format: AudioFormat,
        timestamp: Timestamp,
        samples: Vec<f32>,
    ) -> Result<Self, AudioError> {
        let channels = usize::from(format.channels);
        if !samples.len().is_multiple_of(channels) {
            return Err(AudioError::SamplesNotInterleaved {
                samples: samples.len(),
                channels: format.channels,
            });
        }
        let frames = samples.len() / channels;
        if frames > MAX_AUDIO_FRAMES {
            return Err(AudioError::BufferTooLarge { frames });
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(AudioError::NonFiniteSample);
        }

        Ok(Self {
            format,
            timestamp,
            samples,
        })
    }

    /// Creates a silence buffer with `frames` interleaved frames.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::BufferTooLarge`] when `frames` exceeds the reference
    /// buffer limit.
    pub fn silence(
        format: AudioFormat,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<Self, AudioError> {
        if frames > MAX_AUDIO_FRAMES {
            return Err(AudioError::BufferTooLarge { frames });
        }
        let sample_count = frames
            .checked_mul(usize::from(format.channels))
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let samples = vec![0.0; sample_count];
        Ok(Self {
            format,
            timestamp,
            samples,
        })
    }

    /// Returns the audio format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }

    /// Returns the first-sample timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Returns the number of interleaved audio frames.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.format.channels)
    }

    /// Returns the immutable interleaved sample slice.
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Returns a sample if its frame and channel are in range.
    #[must_use]
    pub fn sample(&self, frame: usize, channel: usize) -> Option<f32> {
        if channel >= usize::from(self.format.channels) {
            return None;
        }
        self.samples
            .get(frame * usize::from(self.format.channels) + channel)
            .copied()
    }

    /// Returns the integer nanosecond duration represented by this buffer.
    #[must_use]
    pub fn duration_nanos(&self) -> Option<u64> {
        let duration = u128::try_from(self.frames())
            .ok()?
            .checked_mul(1_000_000_000)?
            / u128::from(self.format.sample_rate);
        u64::try_from(duration).ok()
    }

    /// Returns the exclusive timestamp at the end of this buffer.
    #[must_use]
    pub fn end_timestamp(&self) -> Option<Timestamp> {
        self.timestamp.checked_add(self.duration_nanos()?)
    }

    /// Drops complete leading sample frames and advances the timestamp.
    ///
    /// Returns `Ok(None)` when all frames were removed.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ScheduleOverflow`] when the new timestamp cannot be
    /// represented.
    pub fn trim_front(&self, frames: usize) -> Result<Option<Self>, AudioError> {
        if frames >= self.frames() {
            return Ok(None);
        }
        let timestamp = self
            .timestamp
            .checked_add(audio_duration_nanos(frames, self.format.sample_rate())?)
            .ok_or(AudioError::ScheduleOverflow)?;
        let channels = usize::from(self.format.channels);
        let start = frames
            .checked_mul(channels)
            .ok_or(AudioError::ScheduleOverflow)?;
        Ok(Some(Self::new(
            self.format,
            timestamp,
            self.samples[start..].to_vec(),
        )?))
    }

    /// Prefixes complete silence frames and assigns `timestamp` to the new buffer.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::BufferTooLarge`] if the combined buffer exceeds the
    /// reference limit.
    pub fn prepend_silence(&self, frames: usize, timestamp: Timestamp) -> Result<Self, AudioError> {
        let total_frames = frames
            .checked_add(self.frames())
            .filter(|total| *total <= MAX_AUDIO_FRAMES)
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let channels = usize::from(self.format.channels);
        let silence_samples = frames
            .checked_mul(channels)
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let mut samples = vec![0.0; silence_samples];
        samples.extend_from_slice(&self.samples);
        Self::new(self.format, timestamp, samples).inspect(|buffer| {
            debug_assert_eq!(buffer.frames(), total_frames);
        })
    }
}

impl AudioScheduler {
    /// Creates a scheduler beginning at sample-frame index zero.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self {
            format,
            next_index: 0,
        }
    }

    /// Returns and advances the next exact sample-clock deadline.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ScheduleOverflow`] when the timeline no longer fits
    /// in the public integer representation.
    pub fn next_deadline(&mut self) -> Result<AudioDeadline, AudioError> {
        let index = self.next_index;
        let timestamp = audio_timestamp_for(index, self.format.sample_rate())?;
        self.next_index = self
            .next_index
            .checked_add(1)
            .ok_or(AudioError::ScheduleOverflow)?;
        Ok(AudioDeadline { index, timestamp })
    }

    /// Returns and advances the first deadline of a non-empty audio block.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroBlock`] for an empty block or
    /// [`AudioError::ScheduleOverflow`] when advancing the sample index fails.
    pub fn next_block_deadline(&mut self, frames: usize) -> Result<AudioDeadline, AudioError> {
        if frames == 0 {
            return Err(AudioError::ZeroBlock);
        }
        let deadline = AudioDeadline {
            index: self.next_index,
            timestamp: audio_timestamp_for(self.next_index, self.format.sample_rate())?,
        };
        let frames = u64::try_from(frames).map_err(|_| AudioError::ScheduleOverflow)?;
        self.next_index = self
            .next_index
            .checked_add(frames)
            .ok_or(AudioError::ScheduleOverflow)?;
        Ok(deadline)
    }

    /// Resets the scheduler to sample-frame index zero.
    pub fn reset(&mut self) {
        self.next_index = 0;
    }

    /// Returns the scheduled audio format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }
}

impl MonotonicAudioClock {
    /// Starts a clock at the current monotonic instant.
    #[must_use]
    pub fn start() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    /// Returns elapsed nanoseconds since [`Self::start`].
    #[must_use]
    pub fn now(&self) -> Timestamp {
        Timestamp::from_nanos(u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }
}

impl AudioClock for MonotonicAudioClock {
    fn now(&self) -> Timestamp {
        Self::now(self)
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        let current = Self::now(self);
        let remaining = deadline.as_nanos().saturating_sub(current.as_nanos());
        if remaining != 0 {
            std::thread::sleep(Duration::from_nanos(remaining));
        }
    }
}

impl AudioPacer {
    /// Creates a block pacer beginning at sample-frame index zero.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self {
            scheduler: AudioScheduler::new(format),
        }
    }

    /// Waits for the first sample deadline of the next non-empty block.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroBlock`] for an empty block or
    /// [`AudioError::ScheduleOverflow`] when the sample timeline is exhausted.
    pub fn next<C: AudioClock>(
        &mut self,
        clock: &mut C,
        frames: usize,
    ) -> Result<AudioPacingResult, AudioError> {
        let deadline = self.scheduler.next_block_deadline(frames)?;
        let requested_at = clock.now();
        clock.sleep_until(deadline.timestamp());
        let observed_at = clock.now();
        Ok(AudioPacingResult {
            deadline,
            frames,
            requested_at,
            observed_at,
            waited_nanos: observed_at
                .as_nanos()
                .saturating_sub(requested_at.as_nanos()),
        })
    }

    /// Resets the pacer to sample-frame index zero.
    pub fn reset(&mut self) {
        self.scheduler.reset();
    }

    /// Returns the configured audio format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.scheduler.format()
    }
}

impl AudioResampler {
    /// Creates a resampler between two sample rates.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ChannelMismatch`] when the formats do not have the
    /// same channel layout.
    pub const fn new(input: AudioFormat, output: AudioFormat) -> Result<Self, AudioError> {
        if input.channels != output.channels {
            return Err(AudioError::ChannelMismatch);
        }
        Ok(Self { input, output })
    }

    /// Converts one owned buffer with linear interpolation.
    ///
    /// The output frame count is rounded to the nearest integer. Empty input
    /// remains empty and timestamps are preserved at the beginning of the buffer.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FormatMismatch`] for a buffer with the wrong source
    /// format or [`AudioError::BufferTooLarge`] when the output exceeds the limit.
    #[allow(clippy::cast_precision_loss)]
    pub fn process(&self, input: &AudioBuffer) -> Result<AudioBuffer, AudioError> {
        if input.format() != self.input {
            return Err(AudioError::FormatMismatch {
                expected: self.input,
                actual: input.format(),
            });
        }
        if input.frames() == 0 {
            return AudioBuffer::silence(self.output, input.timestamp(), 0);
        }

        let output_frames = (input.frames() as u128 * u128::from(self.output.sample_rate)
            + u128::from(self.input.sample_rate) / 2)
            / u128::from(self.input.sample_rate);
        let output_frames = usize::try_from(output_frames)
            .ok()
            .filter(|frames| *frames <= MAX_AUDIO_FRAMES)
            .ok_or(AudioError::BufferTooLarge { frames: usize::MAX })?;
        let channels = usize::from(self.input.channels);
        let mut samples = vec![0.0; output_frames * channels];

        for output_frame in 0..output_frames {
            let position = output_frame as u128 * u128::from(self.input.sample_rate) * 1_000_000
                / u128::from(self.output.sample_rate);
            let base = usize::try_from(position / 1_000_000).unwrap_or(usize::MAX);
            let fraction = (position % 1_000_000) as f32 / 1_000_000.0;
            let first = base.min(input.frames() - 1);
            let second = (first + 1).min(input.frames() - 1);
            for channel in 0..channels {
                let first_sample = input.sample(first, channel).unwrap_or(0.0);
                let second_sample = input.sample(second, channel).unwrap_or(first_sample);
                samples[output_frame * channels + channel] =
                    first_sample + (second_sample - first_sample) * fraction;
            }
        }

        AudioBuffer::new(self.output, input.timestamp(), samples)
    }

    /// Returns the input format.
    #[must_use]
    pub const fn input_format(&self) -> AudioFormat {
        self.input
    }

    /// Returns the output format.
    #[must_use]
    pub const fn output_format(&self) -> AudioFormat {
        self.output
    }
}

/// Drop behavior for a bounded audio queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioDropPolicy {
    /// Remove complete oldest buffers until the new buffer fits.
    DropOldest,
    /// Keep queued buffers and discard the new buffer.
    DropNewest,
}

/// Result of pushing a buffer into an [`AudioQueue`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioPushOutcome {
    /// The buffer was queued without dropping data.
    Enqueued,
    /// This many old frames were removed to make room.
    DroppedOldest { frames: usize },
    /// The submitted buffer was discarded.
    DroppedNewest { frames: usize },
}

/// Errors raised by audio values, queues, and mixing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioError {
    /// Sample rate or channel count is zero.
    InvalidFormat,
    /// The sample count is not a complete interleaved frame sequence.
    SamplesNotInterleaved { samples: usize, channels: u16 },
    /// A sample is NaN or infinite.
    NonFiniteSample,
    /// An audio buffer exceeds [`MAX_AUDIO_FRAMES`].
    BufferTooLarge { frames: usize },
    /// A buffer format does not match a queue or mixer.
    FormatMismatch {
        /// Expected format.
        expected: AudioFormat,
        /// Supplied format.
        actual: AudioFormat,
    },
    /// A worker buffer starts at a different sample-clock timestamp.
    BufferTimestampMismatch {
        /// Timestamp required by the worker deadline.
        expected: Timestamp,
        /// Timestamp supplied by the producer.
        actual: Timestamp,
    },
    /// Two audio formats have different channel counts.
    ChannelMismatch,
    /// A buffer has a different frame count from a mix request.
    FrameCountMismatch { expected: usize, actual: usize },
    /// A queue capacity is zero.
    ZeroCapacity,
    /// A mixer source ID is unknown.
    UnknownSource(AudioSourceId),
    /// A source occurs more than once in one mix input list.
    DuplicateInput(AudioSourceId),
    /// A source gain is not finite.
    InvalidGain,
    /// A source pan is not finite or is outside `[-1.0, 1.0]`.
    InvalidPan,
    /// A mix sum overflowed the finite `f32` range.
    MixOverflow,
    /// Source IDs are exhausted.
    SourceIdExhausted,
    /// A monitor tap capacity is zero.
    ZeroMonitorCapacity,
    /// A monitor tap ID is unknown.
    UnknownMonitorTap(AudioMonitorTapId),
    /// Monitor tap IDs are exhausted.
    MonitorTapIdExhausted,
    /// An audio pacing block must contain at least one sample frame.
    ZeroBlock,
    /// The sample-clock timestamp or frame index overflowed.
    ScheduleOverflow,
}

impl fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => formatter.write_str("audio format values must be non-zero"),
            Self::SamplesNotInterleaved { samples, channels } => write!(
                formatter,
                "{samples} samples cannot be interleaved across {channels} channels"
            ),
            Self::NonFiniteSample => formatter.write_str("audio samples must be finite"),
            Self::BufferTooLarge { frames } => {
                write!(formatter, "audio buffer has too many frames: {frames}")
            }
            Self::FormatMismatch { expected, actual } => {
                write!(
                    formatter,
                    "audio format {actual:?} does not match {expected:?}"
                )
            }
            Self::BufferTimestampMismatch { expected, actual } => {
                write!(
                    formatter,
                    "audio buffer starts at {actual:?}; expected {expected:?}"
                )
            }
            Self::ChannelMismatch => formatter.write_str("audio channel layouts do not match"),
            Self::FrameCountMismatch { expected, actual } => {
                write!(
                    formatter,
                    "audio buffer has {actual} frames; expected {expected}"
                )
            }
            Self::ZeroCapacity => formatter.write_str("audio queue capacity must be non-zero"),
            Self::UnknownSource(source) => {
                write!(formatter, "audio source {} does not exist", source.value())
            }
            Self::DuplicateInput(source) => {
                write!(
                    formatter,
                    "audio source {} occurs more than once",
                    source.value()
                )
            }
            Self::InvalidGain => formatter.write_str("audio gain must be finite"),
            Self::InvalidPan => {
                formatter.write_str("audio pan must be finite and between -1 and 1")
            }
            Self::MixOverflow => formatter.write_str("audio mix exceeded finite sample range"),
            Self::SourceIdExhausted => formatter.write_str("audio source ID space is exhausted"),
            Self::ZeroMonitorCapacity => {
                formatter.write_str("audio monitor tap capacity must be non-zero")
            }
            Self::UnknownMonitorTap(tap) => {
                write!(
                    formatter,
                    "audio monitor tap {} does not exist",
                    tap.value()
                )
            }
            Self::MonitorTapIdExhausted => {
                formatter.write_str("audio monitor tap ID space is exhausted")
            }
            Self::ZeroBlock => formatter.write_str("audio pacing blocks must be non-empty"),
            Self::ScheduleOverflow => formatter.write_str("audio schedule timestamp overflowed"),
        }
    }
}

impl std::error::Error for AudioError {}

/// A bounded queue of complete audio buffers.
pub struct AudioQueue {
    format: AudioFormat,
    capacity_frames: usize,
    policy: AudioDropPolicy,
    queued_frames: usize,
    buffers: VecDeque<AudioBuffer>,
}

impl AudioQueue {
    /// Creates a queue with a maximum number of buffered frames.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroCapacity`] for a zero capacity.
    pub fn new(
        format: AudioFormat,
        capacity_frames: usize,
        policy: AudioDropPolicy,
    ) -> Result<Self, AudioError> {
        if capacity_frames == 0 {
            return Err(AudioError::ZeroCapacity);
        }
        Ok(Self {
            format,
            capacity_frames,
            policy,
            queued_frames: 0,
            buffers: VecDeque::new(),
        })
    }

    /// Pushes a complete buffer under the queue's drop policy.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FormatMismatch`] for a different format or
    /// [`AudioError::BufferTooLarge`] when one buffer cannot fit in the queue.
    pub fn push(&mut self, buffer: AudioBuffer) -> Result<AudioPushOutcome, AudioError> {
        if buffer.format() != self.format {
            return Err(AudioError::FormatMismatch {
                expected: self.format,
                actual: buffer.format(),
            });
        }
        let incoming_frames = buffer.frames();
        if incoming_frames > self.capacity_frames {
            return Err(AudioError::BufferTooLarge {
                frames: incoming_frames,
            });
        }

        let free_frames = self.capacity_frames.saturating_sub(self.queued_frames);
        if incoming_frames <= free_frames {
            self.queued_frames += incoming_frames;
            self.buffers.push_back(buffer);
            return Ok(AudioPushOutcome::Enqueued);
        }

        match self.policy {
            AudioDropPolicy::DropNewest => Ok(AudioPushOutcome::DroppedNewest {
                frames: incoming_frames,
            }),
            AudioDropPolicy::DropOldest => {
                let mut dropped_frames = 0;
                while incoming_frames > self.capacity_frames.saturating_sub(self.queued_frames) {
                    let Some(dropped) = self.buffers.pop_front() else {
                        break;
                    };
                    let frames = dropped.frames();
                    self.queued_frames -= frames;
                    dropped_frames += frames;
                }
                self.queued_frames += incoming_frames;
                self.buffers.push_back(buffer);
                Ok(AudioPushOutcome::DroppedOldest {
                    frames: dropped_frames,
                })
            }
        }
    }

    /// Removes and returns the oldest queued buffer.
    pub fn pop(&mut self) -> Option<AudioBuffer> {
        let buffer = self.buffers.pop_front()?;
        self.queued_frames -= buffer.frames();
        Some(buffer)
    }

    /// Returns the number of queued frames.
    #[must_use]
    pub const fn queued_frames(&self) -> usize {
        self.queued_frames
    }

    /// Returns the maximum number of queued frames.
    #[must_use]
    pub const fn capacity_frames(&self) -> usize {
        self.capacity_frames
    }

    /// Returns whether the queue has no buffers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }

    /// Removes every queued buffer without changing the queue configuration.
    pub fn clear(&mut self) {
        self.buffers.clear();
        self.queued_frames = 0;
    }
}

/// A paced, cancellation-aware audio producer over a bounded output queue.
pub struct AudioWorker {
    pacer: AudioPacer,
    queue: AudioQueue,
}

impl AudioWorker {
    /// Creates a worker for one audio format and bounded output policy.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroCapacity`] when `capacity_frames` is zero.
    pub fn new(
        format: AudioFormat,
        capacity_frames: usize,
        policy: AudioDropPolicy,
    ) -> Result<Self, AudioError> {
        Ok(Self {
            pacer: AudioPacer::new(format),
            queue: AudioQueue::new(format, capacity_frames, policy)?,
        })
    }

    /// Runs up to `block_count` paced blocks without blocking the output consumer.
    ///
    /// The producer is called after each block reaches its sample-clock deadline.
    /// A produced buffer must match the worker format, block length, and exact
    /// first-sample timestamp. The worker retains complete buffers in its bounded
    /// queue; callers consume them with [`Self::take_next`].
    ///
    /// # Errors
    ///
    /// Returns [`AudioWorkerError::Pacing`] for clock/timeline failures,
    /// [`AudioWorkerError::Source`] for producer failures, or
    /// [`AudioWorkerError::Submit`] for a buffer contract violation.
    pub fn run<C, E, F>(
        &mut self,
        clock: &mut C,
        block_frames: usize,
        block_count: u64,
        cancellation: &AudioCancellationToken,
        mut produce: F,
    ) -> Result<AudioWorkerReport, AudioWorkerError<E>>
    where
        C: AudioClock,
        F: FnMut(AudioDeadline, AudioFormat, usize) -> Result<Option<AudioBuffer>, E>,
    {
        let format = self.pacer.format();
        let mut report = AudioWorkerReport {
            requested_blocks: block_count,
            ..AudioWorkerReport::default()
        };

        for _ in 0..block_count {
            if cancellation.is_cancelled() {
                break;
            }

            let pacing = self
                .pacer
                .next(clock, block_frames)
                .map_err(AudioWorkerError::Pacing)?;
            let deadline = pacing.deadline();
            let produced =
                produce(deadline, format, block_frames).map_err(AudioWorkerError::Source)?;

            match produced {
                None => {
                    report.underflow_blocks = report.underflow_blocks.saturating_add(1);
                }
                Some(buffer) => {
                    if buffer.format() != format {
                        return Err(AudioWorkerError::Submit(AudioError::FormatMismatch {
                            expected: format,
                            actual: buffer.format(),
                        }));
                    }
                    if buffer.frames() != block_frames {
                        return Err(AudioWorkerError::Submit(AudioError::FrameCountMismatch {
                            expected: block_frames,
                            actual: buffer.frames(),
                        }));
                    }
                    if buffer.timestamp() != deadline.timestamp() {
                        return Err(AudioWorkerError::Submit(
                            AudioError::BufferTimestampMismatch {
                                expected: deadline.timestamp(),
                                actual: buffer.timestamp(),
                            },
                        ));
                    }

                    report.produced_frames = report
                        .produced_frames
                        .saturating_add(u64::try_from(buffer.frames()).unwrap_or(u64::MAX));
                    match self.queue.push(buffer).map_err(AudioWorkerError::Submit)? {
                        AudioPushOutcome::Enqueued => {}
                        AudioPushOutcome::DroppedOldest { frames } => {
                            report.dropped_oldest_frames = report
                                .dropped_oldest_frames
                                .saturating_add(u64::try_from(frames).unwrap_or(u64::MAX));
                        }
                        AudioPushOutcome::DroppedNewest { frames } => {
                            report.dropped_newest_frames = report
                                .dropped_newest_frames
                                .saturating_add(u64::try_from(frames).unwrap_or(u64::MAX));
                        }
                    }
                }
            }

            let observed_at = clock.now();
            let lateness = observed_at
                .as_nanos()
                .saturating_sub(deadline.timestamp().as_nanos());
            if lateness != 0 {
                report.missed_deadlines = report.missed_deadlines.saturating_add(1);
                report.total_lateness_nanos = report.total_lateness_nanos.saturating_add(lateness);
            }
            report.processed_blocks = report.processed_blocks.saturating_add(1);
        }

        report.cancelled = cancellation.is_cancelled() && report.processed_blocks < block_count;
        report.remaining_queue_frames = self.queue.queued_frames();
        Ok(report)
    }

    /// Removes and returns the oldest produced audio block.
    pub fn take_next(&mut self) -> Option<AudioBuffer> {
        self.queue.pop()
    }

    /// Returns the number of sample frames waiting for the output consumer.
    #[must_use]
    pub const fn queued_frames(&self) -> usize {
        self.queue.queued_frames()
    }

    /// Returns the configured worker format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.pacer.format()
    }

    /// Resets the timeline and discards queued output for a new run.
    pub fn reset(&mut self) {
        self.pacer.reset();
        self.queue.clear();
    }
}

struct SourceControl {
    gain: f32,
    muted: bool,
    pan: f32,
}

/// A deterministic mixer for registered audio sources.
pub struct AudioMixer {
    format: AudioFormat,
    sources: BTreeMap<AudioSourceId, SourceControl>,
    monitor_taps: BTreeMap<AudioMonitorTapId, AudioMonitorTap>,
    next_source_id: u64,
    next_monitor_tap_id: u64,
}

impl AudioMixer {
    /// Creates an empty mixer for one audio format.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self {
            format,
            sources: BTreeMap::new(),
            monitor_taps: BTreeMap::new(),
            next_source_id: 1,
            next_monitor_tap_id: 1,
        }
    }

    /// Registers a source with an initial linear gain.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidGain`] for a non-finite gain or
    /// [`AudioError::SourceIdExhausted`] when no new ID is available.
    pub fn add_source(&mut self, gain: f32) -> Result<AudioSourceId, AudioError> {
        if !gain.is_finite() {
            return Err(AudioError::InvalidGain);
        }
        let id = AudioSourceId(self.next_source_id);
        self.next_source_id = self
            .next_source_id
            .checked_add(1)
            .ok_or(AudioError::SourceIdExhausted)?;
        self.sources.insert(
            id,
            SourceControl {
                gain,
                muted: false,
                pan: 0.0,
            },
        );
        Ok(id)
    }

    /// Updates a source's gain.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidGain`] for a non-finite value or
    /// [`AudioError::UnknownSource`] for an unknown source.
    pub fn set_gain(&mut self, source: AudioSourceId, gain: f32) -> Result<(), AudioError> {
        if !gain.is_finite() {
            return Err(AudioError::InvalidGain);
        }
        let control = self
            .sources
            .get_mut(&source)
            .ok_or(AudioError::UnknownSource(source))?;
        control.gain = gain;
        Ok(())
    }

    /// Mutes or unmutes one source.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownSource`] for an unknown source.
    pub fn set_muted(&mut self, source: AudioSourceId, muted: bool) -> Result<(), AudioError> {
        let control = self
            .sources
            .get_mut(&source)
            .ok_or(AudioError::UnknownSource(source))?;
        control.muted = muted;
        Ok(())
    }

    /// Sets a source's stereo pan, where `-1` is left, `0` is center, and `1` is
    /// right. Channels after the first two are left unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidPan`] for a non-finite or out-of-range value,
    /// or [`AudioError::UnknownSource`] for an unknown source.
    pub fn set_pan(&mut self, source: AudioSourceId, pan: f32) -> Result<(), AudioError> {
        if !pan.is_finite() || !(-1.0..=1.0).contains(&pan) {
            return Err(AudioError::InvalidPan);
        }
        let control = self
            .sources
            .get_mut(&source)
            .ok_or(AudioError::UnknownSource(source))?;
        control.pan = pan;
        Ok(())
    }

    /// Removes a source from future mixes.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownSource`] for an unknown source.
    pub fn remove_source(&mut self, source: AudioSourceId) -> Result<(), AudioError> {
        self.sources
            .remove(&source)
            .map(|_| ())
            .ok_or(AudioError::UnknownSource(source))
    }

    /// Adds a bounded post-mix monitoring tap.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroMonitorCapacity`] for a zero capacity or
    /// [`AudioError::MonitorTapIdExhausted`] when no new ID is available.
    pub fn add_monitor_tap(
        &mut self,
        capacity_buffers: usize,
    ) -> Result<AudioMonitorTapId, AudioError> {
        let tap = AudioMonitorTap::new(capacity_buffers)?;
        let id = AudioMonitorTapId(self.next_monitor_tap_id);
        self.next_monitor_tap_id = self
            .next_monitor_tap_id
            .checked_add(1)
            .ok_or(AudioError::MonitorTapIdExhausted)?;
        self.monitor_taps.insert(id, tap);
        Ok(id)
    }

    /// Removes a monitoring tap and returns its retained buffers.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownMonitorTap`] for an unknown tap.
    pub fn remove_monitor_tap(
        &mut self,
        tap: AudioMonitorTapId,
    ) -> Result<AudioMonitorTap, AudioError> {
        self.monitor_taps
            .remove(&tap)
            .ok_or(AudioError::UnknownMonitorTap(tap))
    }

    /// Removes the oldest buffer from one monitoring tap.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownMonitorTap`] for an unknown tap.
    pub fn pop_monitor_buffer(
        &mut self,
        tap: AudioMonitorTapId,
    ) -> Result<Option<AudioBuffer>, AudioError> {
        self.monitor_taps
            .get_mut(&tap)
            .ok_or(AudioError::UnknownMonitorTap(tap))
            .map(AudioMonitorTap::pop)
    }

    /// Returns the number of buffers dropped by one monitoring tap.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::UnknownMonitorTap`] for an unknown tap.
    pub fn monitor_dropped_buffers(&self, tap: AudioMonitorTapId) -> Result<u64, AudioError> {
        self.monitor_taps
            .get(&tap)
            .map(AudioMonitorTap::dropped_buffers)
            .ok_or(AudioError::UnknownMonitorTap(tap))
    }

    /// Mixes `inputs` into an owned output buffer and clamps it to `[-1.0, 1.0]`.
    ///
    /// Missing registered inputs contribute silence. Every supplied input must be
    /// registered, have the mixer format, and contain exactly `frames` frames.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError`] when an input is duplicated, unknown, mismatched, or
    /// has a different frame count, or when the sum becomes non-finite.
    pub fn mix(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
        inputs: &[(AudioSourceId, &AudioBuffer)],
    ) -> Result<AudioBuffer, AudioError> {
        if frames > MAX_AUDIO_FRAMES {
            return Err(AudioError::BufferTooLarge { frames });
        }
        let channels = usize::from(self.format.channels);
        let sample_count = frames
            .checked_mul(channels)
            .ok_or(AudioError::BufferTooLarge { frames })?;
        let mut mixed = vec![0.0; sample_count];
        let mut seen = BTreeSet::new();

        for (source, buffer) in inputs {
            if !seen.insert(*source) {
                return Err(AudioError::DuplicateInput(*source));
            }
            let control = self
                .sources
                .get(source)
                .ok_or(AudioError::UnknownSource(*source))?;
            if buffer.format() != self.format {
                return Err(AudioError::FormatMismatch {
                    expected: self.format,
                    actual: buffer.format(),
                });
            }
            if buffer.frames() != frames {
                return Err(AudioError::FrameCountMismatch {
                    expected: frames,
                    actual: buffer.frames(),
                });
            }
            if control.muted {
                continue;
            }
            for (sample_index, (output, input)) in
                mixed.iter_mut().zip(buffer.samples()).enumerate()
            {
                let channel = sample_index % channels;
                let pan_gain = match channel {
                    0 => 1.0 - control.pan.max(0.0),
                    1 => 1.0 + control.pan.min(0.0),
                    _ => 1.0,
                };
                *output += *input * control.gain * pan_gain;
                if !output.is_finite() {
                    return Err(AudioError::MixOverflow);
                }
            }
        }

        for sample in &mut mixed {
            *sample = sample.clamp(-1.0, 1.0);
        }
        let output = AudioBuffer::new(self.format, timestamp, mixed)?;
        for tap in self.monitor_taps.values_mut() {
            tap.observe(&output);
        }
        Ok(output)
    }

    /// Returns the mixer format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }

    /// Returns the number of registered sources.
    #[must_use]
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }
}

fn audio_timestamp_for(index: u64, sample_rate: u32) -> Result<Timestamp, AudioError> {
    let nanoseconds = u128::from(index)
        .checked_mul(1_000_000_000)
        .ok_or(AudioError::ScheduleOverflow)?
        / u128::from(sample_rate);
    let nanoseconds = u64::try_from(nanoseconds).map_err(|_| AudioError::ScheduleOverflow)?;
    Ok(Timestamp::from_nanos(nanoseconds))
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

fn audio_duration_nanos(frames: usize, sample_rate: u32) -> Result<u64, AudioError> {
    let duration = u128::try_from(frames)
        .ok()
        .and_then(|frames| frames.checked_mul(1_000_000_000))
        .ok_or(AudioError::ScheduleOverflow)?
        / u128::from(sample_rate);
    u64::try_from(duration).map_err(|_| AudioError::ScheduleOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeClock {
        now: Timestamp,
        requested_deadlines: Vec<Timestamp>,
    }

    impl AudioClock for FakeClock {
        fn now(&self) -> Timestamp {
            self.now
        }

        fn sleep_until(&mut self, deadline: Timestamp) {
            self.requested_deadlines.push(deadline);
            if deadline > self.now {
                self.now = deadline;
            }
        }
    }

    struct LateClock {
        now: Timestamp,
        delay_nanos: u64,
        requested_deadlines: Vec<Timestamp>,
    }

    impl AudioClock for LateClock {
        fn now(&self) -> Timestamp {
            self.now
        }

        fn sleep_until(&mut self, deadline: Timestamp) {
            self.requested_deadlines.push(deadline);
            self.now = Timestamp::from_nanos(deadline.as_nanos().saturating_add(self.delay_nanos));
        }
    }

    fn format() -> AudioFormat {
        AudioFormat::new(48_000, 2).expect("valid audio format")
    }

    fn buffer(values: &[f32]) -> AudioBuffer {
        AudioBuffer::new(format(), Timestamp::ZERO, values.to_vec()).expect("valid buffer")
    }

    #[test]
    fn validates_interleaved_buffers() {
        assert_eq!(AudioFormat::new(0, 2), Err(AudioError::InvalidFormat));
        assert_eq!(
            AudioBuffer::new(format(), Timestamp::ZERO, vec![0.0]),
            Err(AudioError::SamplesNotInterleaved {
                samples: 1,
                channels: 2
            })
        );
        assert_eq!(
            AudioBuffer::new(format(), Timestamp::ZERO, vec![f32::NAN, 0.0]),
            Err(AudioError::NonFiniteSample)
        );
    }

    #[test]
    fn queue_bounds_complete_buffers() {
        let mut queue =
            AudioQueue::new(format(), 3, AudioDropPolicy::DropOldest).expect("valid queue");
        queue
            .push(buffer(&[0.1, 0.1, 0.2, 0.2]))
            .expect("first buffer");
        queue.push(buffer(&[0.3, 0.3])).expect("second buffer");
        assert_eq!(queue.queued_frames(), 3);
        assert_eq!(
            queue.push(buffer(&[0.4, 0.4])).expect("third buffer"),
            AudioPushOutcome::DroppedOldest { frames: 2 }
        );
        assert_eq!(
            queue.pop().expect("remaining buffer").sample(0, 0),
            Some(0.3)
        );
        assert_eq!(queue.queued_frames(), 1);
    }

    #[test]
    fn mixer_applies_gain_mute_and_clamp() {
        let mut mixer = AudioMixer::new(format());
        let loud = mixer.add_source(2.0).expect("source");
        let muted = mixer.add_source(1.0).expect("source");
        mixer.set_muted(muted, true).expect("mute source");
        let loud_buffer = buffer(&[0.75, -0.75]);
        let muted_buffer = buffer(&[1.0, 1.0]);

        let output = mixer
            .mix(
                Timestamp::from_millis(10),
                1,
                &[(loud, &loud_buffer), (muted, &muted_buffer)],
            )
            .expect("mix succeeds");
        assert_eq!(output.timestamp(), Timestamp::from_millis(10));
        assert_eq!(output.samples(), &[1.0, -1.0]);
    }

    #[test]
    fn mixer_applies_stereo_pan_and_rejects_invalid_values() {
        let mut mixer = AudioMixer::new(format());
        let source = mixer.add_source(1.0).expect("source");
        assert_eq!(mixer.set_pan(source, 2.0), Err(AudioError::InvalidPan));
        assert_eq!(mixer.set_pan(source, f32::NAN), Err(AudioError::InvalidPan));
        mixer.set_pan(source, 1.0).expect("right pan");

        let output = mixer
            .mix(Timestamp::ZERO, 1, &[(source, &buffer(&[0.75, 0.5]))])
            .expect("mix succeeds");
        assert_eq!(output.samples(), &[0.0, 0.5]);
    }

    #[test]
    fn mixer_monitor_taps_are_bounded_and_post_mix() {
        let mut mixer = AudioMixer::new(format());
        let source = mixer.add_source(1.0).expect("source");
        let tap = mixer.add_monitor_tap(1).expect("tap");
        let first =
            AudioBuffer::new(format(), Timestamp::ZERO, vec![0.25, 0.5]).expect("first input");
        let second = AudioBuffer::new(format(), Timestamp::from_millis(1), vec![0.75, 1.0])
            .expect("second input");

        mixer
            .mix(Timestamp::ZERO, 1, &[(source, &first)])
            .expect("first mix");
        mixer
            .mix(Timestamp::from_millis(1), 1, &[(source, &second)])
            .expect("second mix");

        assert_eq!(mixer.monitor_dropped_buffers(tap), Ok(1));
        let monitored = mixer
            .pop_monitor_buffer(tap)
            .expect("tap exists")
            .expect("latest buffer");
        assert_eq!(monitored.timestamp(), Timestamp::from_millis(1));
        assert_eq!(monitored.samples(), &[0.75, 1.0]);
        assert_eq!(mixer.pop_monitor_buffer(tap), Ok(None));
        assert_eq!(
            AudioMonitorTap::new(0).map(|_| ()),
            Err(AudioError::ZeroMonitorCapacity)
        );
    }

    #[test]
    fn mixer_rejects_duplicate_and_unknown_inputs() {
        let mut mixer = AudioMixer::new(format());
        let source = mixer.add_source(1.0).expect("source");
        let input = buffer(&[0.1, 0.2]);

        assert_eq!(
            mixer.mix(Timestamp::ZERO, 1, &[(source, &input), (source, &input)]),
            Err(AudioError::DuplicateInput(source))
        );
        assert_eq!(
            mixer.mix(Timestamp::ZERO, 1, &[(AudioSourceId(99), &input)]),
            Err(AudioError::UnknownSource(AudioSourceId(99)))
        );
    }

    #[test]
    fn resampler_changes_rate_and_preserves_channel_layout() {
        let input_format = AudioFormat::new(48_000, 1).expect("input format");
        let output_format = AudioFormat::new(24_000, 1).expect("output format");
        let input = AudioBuffer::new(
            input_format,
            Timestamp::from_millis(2),
            vec![0.0, 1.0, 0.0, -1.0],
        )
        .expect("input buffer");
        let resampler = AudioResampler::new(input_format, output_format).expect("resampler");
        let output = resampler.process(&input).expect("resample succeeds");

        assert_eq!(output.format(), output_format);
        assert_eq!(output.frames(), 2);
        assert_eq!(output.timestamp(), Timestamp::from_millis(2));
        assert_eq!(output.samples(), &[0.0, 0.0]);
    }

    #[test]
    fn resampler_rejects_different_channel_counts() {
        let input = AudioFormat::new(48_000, 1).expect("input format");
        let output = AudioFormat::new(48_000, 2).expect("output format");

        assert_eq!(
            AudioResampler::new(input, output),
            Err(AudioError::ChannelMismatch)
        );
    }

    #[test]
    fn scheduler_and_buffer_end_use_sample_clock_timestamps() {
        let mono = AudioFormat::new(48_000, 1).expect("mono format");
        let mut scheduler = AudioScheduler::new(mono);
        assert_eq!(
            scheduler.next_deadline().expect("first deadline"),
            AudioDeadline {
                index: 0,
                timestamp: Timestamp::ZERO
            }
        );
        assert_eq!(
            scheduler
                .next_deadline()
                .expect("second deadline")
                .timestamp(),
            Timestamp::from_nanos(20_833)
        );
        assert_eq!(
            scheduler
                .next_deadline()
                .expect("third deadline")
                .timestamp(),
            Timestamp::from_nanos(41_666)
        );

        let buffer = AudioBuffer::silence(mono, Timestamp::from_millis(10), 48_000)
            .expect("one second of silence");
        assert_eq!(buffer.duration_nanos(), Some(1_000_000_000));
        assert_eq!(buffer.end_timestamp(), Some(Timestamp::from_millis(1_010)));
    }

    #[test]
    fn audio_pacer_advances_by_blocks_with_an_injected_clock() {
        let mut clock = FakeClock {
            now: Timestamp::from_millis(5),
            requested_deadlines: Vec::new(),
        };
        let mut pacer = AudioPacer::new(format());
        assert_eq!(pacer.next(&mut clock, 0), Err(AudioError::ZeroBlock));

        let first = pacer.next(&mut clock, 480).expect("first block");
        assert_eq!(first.deadline().index(), 0);
        assert_eq!(first.frames(), 480);
        assert!(first.missed());
        assert_eq!(first.waited_nanos(), 0);

        let second = pacer.next(&mut clock, 480).expect("second block");
        assert_eq!(second.deadline().index(), 480);
        assert_eq!(second.deadline().timestamp(), Timestamp::from_millis(10));
        assert_eq!(second.observed_at(), Timestamp::from_millis(10));
        assert_eq!(second.waited_nanos(), Timestamp::from_millis(5).as_nanos());
        assert_eq!(
            clock.requested_deadlines,
            vec![Timestamp::ZERO, Timestamp::from_millis(10)]
        );
    }

    #[test]
    fn audio_worker_reports_underflow_drop_pressure_and_lateness() {
        let mut clock = LateClock {
            now: Timestamp::ZERO,
            delay_nanos: 100,
            requested_deadlines: Vec::new(),
        };
        let token = AudioCancellationToken::new();
        let mut worker =
            AudioWorker::new(format(), 4, AudioDropPolicy::DropNewest).expect("worker");
        let report = worker
            .run(
                &mut clock,
                2,
                4,
                &token,
                |deadline, output_format, frames| {
                    if deadline.index() == 2 {
                        return Ok::<_, std::convert::Infallible>(None);
                    }
                    Ok(Some(
                        AudioBuffer::silence(output_format, deadline.timestamp(), frames)
                            .expect("valid block"),
                    ))
                },
            )
            .expect("worker run");

        assert_eq!(report.requested_blocks(), 4);
        assert_eq!(report.processed_blocks(), 4);
        assert!(!report.cancelled());
        assert_eq!(report.underflow_blocks(), 1);
        assert_eq!(report.produced_frames(), 6);
        assert_eq!(report.dropped_oldest_frames(), 0);
        assert_eq!(report.dropped_newest_frames(), 2);
        assert_eq!(report.missed_deadlines(), 4);
        assert_eq!(report.total_lateness_nanos(), 400);
        assert_eq!(report.remaining_queue_frames(), 4);
        assert_eq!(clock.requested_deadlines.len(), 4);

        assert_eq!(worker.take_next().expect("first block").frames(), 2);
        assert_eq!(worker.take_next().expect("second block").frames(), 2);
        assert_eq!(worker.take_next(), None);
    }

    #[test]
    fn audio_worker_cancels_between_blocks_and_shares_token_state() {
        let mut clock = FakeClock {
            now: Timestamp::ZERO,
            requested_deadlines: Vec::new(),
        };
        let token = AudioCancellationToken::new();
        let callback_token = token.clone();
        let mut worker =
            AudioWorker::new(format(), 8, AudioDropPolicy::DropOldest).expect("worker");
        let report = worker
            .run(
                &mut clock,
                2,
                10,
                &token,
                |deadline, output_format, frames| {
                    if deadline.index() == 2 {
                        callback_token.cancel();
                    }
                    Ok::<_, std::convert::Infallible>(Some(
                        AudioBuffer::silence(output_format, deadline.timestamp(), frames)
                            .expect("valid block"),
                    ))
                },
            )
            .expect("worker run");

        assert_eq!(report.processed_blocks(), 2);
        assert!(report.cancelled());
        assert!(token.is_cancelled());
        assert_eq!(report.remaining_queue_frames(), 4);
        token.reset();
        assert!(!token.is_cancelled());
        worker.reset();
        assert_eq!(worker.queued_frames(), 0);
    }

    #[test]
    fn audio_worker_rejects_wrong_timestamp_before_queueing() {
        let mut clock = FakeClock {
            now: Timestamp::ZERO,
            requested_deadlines: Vec::new(),
        };
        let token = AudioCancellationToken::new();
        let mut worker =
            AudioWorker::new(format(), 8, AudioDropPolicy::DropOldest).expect("worker");
        let result = worker.run(&mut clock, 2, 1, &token, |_, output_format, frames| {
            Ok::<_, std::convert::Infallible>(Some(
                AudioBuffer::silence(output_format, Timestamp::from_nanos(1), frames)
                    .expect("valid block"),
            ))
        });

        assert_eq!(
            result,
            Err(AudioWorkerError::Submit(
                AudioError::BufferTimestampMismatch {
                    expected: Timestamp::ZERO,
                    actual: Timestamp::from_nanos(1),
                }
            ))
        );
        assert_eq!(worker.queued_frames(), 0);
    }

    #[test]
    fn av_sync_reports_signed_drift_and_safe_actions() {
        let controller = AvSyncController::new(5_000_000);
        let aligned = controller.observe(Timestamp::from_millis(10), Timestamp::from_millis(12));
        assert_eq!(aligned.state(), SyncState::InSync);
        assert_eq!(aligned.action(), SyncAction::Keep);
        assert_eq!(aligned.delta_nanos(), 2_000_000);

        let audio_behind =
            controller.observe(Timestamp::from_millis(20), Timestamp::from_millis(1));
        assert_eq!(audio_behind.state(), SyncState::AudioBehind);
        assert_eq!(audio_behind.action(), SyncAction::DropEarlyAudio);
        assert_eq!(audio_behind.delta_nanos(), -19_000_000);

        let audio_ahead = controller.observe(Timestamp::from_millis(1), Timestamp::from_millis(20));
        assert_eq!(audio_ahead.state(), SyncState::AudioAhead);
        assert_eq!(audio_ahead.action(), SyncAction::WaitForAudio);
        assert_eq!(audio_ahead.delta_nanos(), 19_000_000);
    }

    #[test]
    fn av_sync_reconciles_early_late_and_obsolete_audio() {
        let controller = AvSyncController::new(1_000);
        let early = AudioBuffer::silence(format(), Timestamp::ZERO, 100).expect("early buffer");
        let trimmed = controller
            .reconcile(Timestamp::from_millis(1), &early)
            .expect("trim succeeds")
            .expect("some audio remains");
        assert_eq!(trimmed.timestamp(), Timestamp::from_millis(1));
        assert_eq!(trimmed.frames(), 52);

        let late =
            AudioBuffer::silence(format(), Timestamp::from_millis(10), 2).expect("late buffer");
        let prefixed = controller
            .reconcile(Timestamp::ZERO, &late)
            .expect("prefix succeeds")
            .expect("late audio remains");
        assert_eq!(prefixed.timestamp(), Timestamp::ZERO);
        assert_eq!(prefixed.frames(), 482);
        assert_eq!(prefixed.sample(0, 0), Some(0.0));
        assert_eq!(prefixed.sample(480, 0), Some(0.0));

        let obsolete = AudioBuffer::silence(format(), Timestamp::ZERO, 2).expect("buffer");
        assert_eq!(
            controller
                .reconcile(Timestamp::from_millis(100), &obsolete)
                .expect("drop succeeds"),
            None
        );
    }

    #[test]
    fn av_sync_monitor_accumulates_long_run_diagnostics() {
        let mut monitor = AvSyncMonitor::new(100);
        for index in 1..=10_000_u64 {
            let video = Timestamp::from_nanos(index * 1_000_000);
            let audio = match index % 3 {
                0 => Timestamp::from_nanos(video.as_nanos() - 200),
                1 => Timestamp::from_nanos(video.as_nanos() + 50),
                _ => Timestamp::from_nanos(video.as_nanos() + 1_000),
            };
            let _ = monitor.observe(video, audio);
        }

        let metrics = monitor.metrics();
        assert_eq!(metrics.observations(), 10_000);
        assert_eq!(metrics.in_sync(), 3_334);
        assert_eq!(metrics.audio_behind(), 3_333);
        assert_eq!(metrics.audio_ahead(), 3_333);
        assert_eq!(metrics.max_abs_delta_nanos(), 1_000);
        assert!(metrics.total_abs_delta_nanos() > metrics.max_abs_delta_nanos());
        monitor.reset();
        assert_eq!(monitor.metrics(), AvSyncMetrics::default());
    }
}
