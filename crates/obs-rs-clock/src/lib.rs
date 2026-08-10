//! Shared, safe audio/video session timing for OBS-RS.
//!
//! This crate owns the relationship between independent sample and frame
//! timelines. Platform device clocks can be adapted to the `AudioClock` and
//! `VideoClock` traits without changing the portable coordinator.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use obs_rs_audio::{
    AudioBuffer, AudioCancellationToken, AudioClock, AudioDeadline, AudioDropPolicy, AudioError,
    AudioFormat, AudioScheduler, AudioWorker, AudioWorkerError, AvSyncMetrics, AvSyncMonitor,
    AvSyncObservation,
};
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
use obs_rs_video::{
    CancellationToken, DropPolicy, FrameDeadline, VideoClock, VideoError, VideoScheduler,
    VideoWorker, WorkerError,
};

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

/// A deterministic pair of rational media timelines with shared drift telemetry.
pub struct MediaTimeline {
    video: VideoScheduler,
    audio: AudioScheduler,
    sync: AvSyncMonitor,
}

impl MediaTimeline {
    /// Creates a timeline pair with an A/V classification tolerance in nanoseconds.
    #[must_use]
    pub fn new(video_rate: FrameRate, audio_format: AudioFormat, tolerance_nanos: u64) -> Self {
        Self {
            video: VideoScheduler::new(video_rate),
            audio: AudioScheduler::new(audio_format),
            sync: AvSyncMonitor::new(tolerance_nanos),
        }
    }

    /// Returns and advances the next video frame deadline.
    ///
    /// # Errors
    ///
    /// Returns [`TimelineError::Video`] if the integer video timeline overflows.
    pub fn next_video_frame(&mut self) -> Result<FrameDeadline, TimelineError> {
        self.video.next_deadline().map_err(TimelineError::Video)
    }

    /// Returns and advances the first deadline of the next audio block.
    ///
    /// # Errors
    ///
    /// Returns [`TimelineError::Audio`] for an empty block or timeline overflow.
    pub fn next_audio_block(&mut self, frames: usize) -> Result<AudioDeadline, TimelineError> {
        self.audio
            .next_block_deadline(frames)
            .map_err(TimelineError::Audio)
    }

    /// Records one cross-domain timestamp comparison.
    #[must_use]
    pub fn observe(
        &mut self,
        video_timestamp: Timestamp,
        audio_timestamp: Timestamp,
    ) -> AvSyncObservation {
        self.sync.observe(video_timestamp, audio_timestamp)
    }

    /// Returns accumulated synchronization diagnostics.
    #[must_use]
    pub const fn metrics(&self) -> AvSyncMetrics {
        self.sync.metrics()
    }

    /// Returns the configured video frame rate.
    #[must_use]
    pub const fn video_rate(&self) -> FrameRate {
        self.video.frame_rate()
    }

    /// Returns the configured audio format.
    #[must_use]
    pub const fn audio_format(&self) -> AudioFormat {
        self.audio.format()
    }

    /// Resets both schedules and accumulated drift diagnostics.
    pub fn reset(&mut self) {
        self.video.reset();
        self.audio.reset();
        self.sync.reset();
    }
}

/// One monotonic wall-clock origin that can drive both worker traits.
pub struct MonotonicMediaClock {
    origin: Instant,
}

impl MonotonicMediaClock {
    /// Starts a shared media clock at the current monotonic instant.
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

    fn sleep_until(&self, deadline: Timestamp) {
        let current = self.now();
        let remaining = deadline.as_nanos().saturating_sub(current.as_nanos());
        if remaining != 0 {
            std::thread::sleep(Duration::from_nanos(remaining));
        }
    }
}

impl AudioClock for MonotonicMediaClock {
    fn now(&self) -> Timestamp {
        Self::now(self)
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        Self::sleep_until(self, deadline);
    }
}

impl VideoClock for MonotonicMediaClock {
    fn now(&self) -> Timestamp {
        Self::now(self)
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        Self::sleep_until(self, deadline);
    }
}

const CLOCK_PPM_SCALE: i128 = 1_000_000;

/// Maximum supported simulated device-clock error in parts per million.
pub const MAX_CLOCK_DRIFT_PPM: i32 = 500_000;

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

/// A validated clock-rate offset expressed in parts per million.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockRate {
    drift_ppm: i32,
}

impl ClockRate {
    /// Creates a rate offset bounded to [`MAX_CLOCK_DRIFT_PPM`].
    ///
    /// # Errors
    ///
    /// Returns [`ClockRateError::DriftOutOfRange`] when the requested offset is
    /// outside the safe positive-rate interval.
    pub const fn new(drift_ppm: i32) -> Result<Self, ClockRateError> {
        if drift_ppm < -MAX_CLOCK_DRIFT_PPM || drift_ppm > MAX_CLOCK_DRIFT_PPM {
            return Err(ClockRateError::DriftOutOfRange { ppm: drift_ppm });
        }
        Ok(Self { drift_ppm })
    }

    /// Returns the signed rate offset in parts per million.
    #[must_use]
    pub const fn drift_ppm(self) -> i32 {
        self.drift_ppm
    }

    fn scale(self) -> i128 {
        CLOCK_PPM_SCALE + i128::from(self.drift_ppm)
    }

    fn observed_at(self, reference: Timestamp) -> Timestamp {
        let nanos = i128::from(reference.as_nanos()) * self.scale() / CLOCK_PPM_SCALE;
        Timestamp::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    fn reference_for(self, deadline: Timestamp) -> Timestamp {
        let numerator = i128::from(deadline.as_nanos()) * CLOCK_PPM_SCALE;
        let reference = (numerator + self.scale() - 1) / self.scale();
        Timestamp::from_nanos(u64::try_from(reference).unwrap_or(u64::MAX))
    }
}

/// A deterministic adapter that models independent audio and video device clocks.
///
/// The adapter advances one shared reference timeline whenever either domain waits,
/// then exposes each domain's independently scaled reading. This makes hardware
/// clock drift testable without an operating-system dependency while preserving the
/// [`AudioClock`] and [`VideoClock`] contracts used by production adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndependentMediaClock {
    reference_nanos: u64,
    audio_rate: ClockRate,
    video_rate: ClockRate,
}

impl IndependentMediaClock {
    /// Creates a two-domain clock from signed audio and video drift in ppm.
    ///
    /// # Errors
    ///
    /// Returns [`ClockRateError::DriftOutOfRange`] when either domain is outside
    /// the supported rate interval.
    pub fn new(audio_drift_ppm: i32, video_drift_ppm: i32) -> Result<Self, ClockRateError> {
        Ok(Self::with_rates(
            ClockRate::new(audio_drift_ppm)?,
            ClockRate::new(video_drift_ppm)?,
        ))
    }

    /// Creates a two-domain clock from already validated rates.
    #[must_use]
    pub const fn with_rates(audio_rate: ClockRate, video_rate: ClockRate) -> Self {
        Self {
            reference_nanos: 0,
            audio_rate,
            video_rate,
        }
    }

    /// Returns the configured audio-domain rate.
    #[must_use]
    pub const fn audio_rate(self) -> ClockRate {
        self.audio_rate
    }

    /// Returns the configured video-domain rate.
    #[must_use]
    pub const fn video_rate(self) -> ClockRate {
        self.video_rate
    }

    /// Returns the shared reference time used by the deterministic adapter.
    #[must_use]
    pub const fn reference_now(self) -> Timestamp {
        Timestamp::from_nanos(self.reference_nanos)
    }

    /// Returns the current audio-device clock reading.
    #[must_use]
    pub fn audio_now(self) -> Timestamp {
        self.audio_rate.observed_at(self.reference_now())
    }

    /// Returns the current video-device clock reading.
    #[must_use]
    pub fn video_now(self) -> Timestamp {
        self.video_rate.observed_at(self.reference_now())
    }

    fn wait_until(&mut self, rate: ClockRate, deadline: Timestamp) {
        let required = rate.reference_for(deadline).as_nanos();
        self.reference_nanos = self.reference_nanos.max(required);
    }
}

impl AudioClock for IndependentMediaClock {
    fn now(&self) -> Timestamp {
        self.audio_now()
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        self.wait_until(self.audio_rate, deadline);
    }
}

impl VideoClock for IndependentMediaClock {
    fn now(&self) -> Timestamp {
        self.video_now()
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        self.wait_until(self.video_rate, deadline);
    }
}

/// A thread-safe cancellation request for a coordinated media session.
#[derive(Clone, Debug)]
pub struct SessionCancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl Default for SessionCancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionCancellationToken {
    /// Creates an uncancelled session token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Requests cancellation before the next coordinated tick.
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

/// Aggregate diagnostics from a coordinated audio/video session run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MediaSessionReport {
    requested_ticks: u64,
    completed_ticks: u64,
    cancelled: bool,
    audio_blocks: u64,
    video_frames: u64,
    audio_underflow_blocks: u64,
    video_empty_frames: u64,
    audio_dropped_oldest_frames: u64,
    audio_dropped_newest_frames: u64,
    video_dropped_oldest: u64,
    video_dropped_newest: u64,
    audio_missed_deadlines: u64,
    video_missed_deadlines: u64,
    audio_lateness_nanos: u64,
    video_lateness_nanos: u64,
    video_wait_nanos: u64,
    video_render_nanos: u64,
    video_max_lateness_nanos: u64,
}

impl MediaSessionReport {
    /// Returns the number of coordinated ticks requested.
    #[must_use]
    pub const fn requested_ticks(self) -> u64 {
        self.requested_ticks
    }

    /// Returns the number of ticks that completed both audio and video work.
    #[must_use]
    pub const fn completed_ticks(self) -> u64 {
        self.completed_ticks
    }

    /// Returns whether cancellation stopped the run early.
    #[must_use]
    pub const fn cancelled(self) -> bool {
        self.cancelled
    }

    /// Returns completed audio blocks.
    #[must_use]
    pub const fn audio_blocks(self) -> u64 {
        self.audio_blocks
    }

    /// Returns completed video frames.
    #[must_use]
    pub const fn video_frames(self) -> u64 {
        self.video_frames
    }

    /// Returns audio producer underflow blocks.
    #[must_use]
    pub const fn audio_underflow_blocks(self) -> u64 {
        self.audio_underflow_blocks
    }

    /// Returns video callbacks that produced no frame.
    #[must_use]
    pub const fn video_empty_frames(self) -> u64 {
        self.video_empty_frames
    }

    /// Returns audio frames removed under drop-oldest pressure.
    #[must_use]
    pub const fn audio_dropped_oldest_frames(self) -> u64 {
        self.audio_dropped_oldest_frames
    }

    /// Returns audio frames discarded under drop-newest pressure.
    #[must_use]
    pub const fn audio_dropped_newest_frames(self) -> u64 {
        self.audio_dropped_newest_frames
    }

    /// Returns video frames removed under drop-oldest pressure.
    #[must_use]
    pub const fn video_dropped_oldest(self) -> u64 {
        self.video_dropped_oldest
    }

    /// Returns video frames discarded under drop-newest pressure.
    #[must_use]
    pub const fn video_dropped_newest(self) -> u64 {
        self.video_dropped_newest
    }

    /// Returns late audio block observations.
    #[must_use]
    pub const fn audio_missed_deadlines(self) -> u64 {
        self.audio_missed_deadlines
    }

    /// Returns late video frame observations.
    #[must_use]
    pub const fn video_missed_deadlines(self) -> u64 {
        self.video_missed_deadlines
    }

    /// Returns total audio post-callback lateness.
    #[must_use]
    pub const fn audio_lateness_nanos(self) -> u64 {
        self.audio_lateness_nanos
    }

    /// Returns total video post-render lateness.
    #[must_use]
    pub const fn video_lateness_nanos(self) -> u64 {
        self.video_lateness_nanos
    }

    /// Returns total video time spent waiting for deadlines.
    #[must_use]
    pub const fn video_wait_nanos(self) -> u64 {
        self.video_wait_nanos
    }

    /// Returns total video time spent inside render callbacks.
    #[must_use]
    pub const fn video_render_nanos(self) -> u64 {
        self.video_render_nanos
    }

    /// Returns the greatest video lateness observed during the run.
    #[must_use]
    pub const fn video_max_lateness_nanos(self) -> u64 {
        self.video_max_lateness_nanos
    }
}

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

/// A coordinated audio/video worker pair driven by one clock origin.
pub struct MediaSession<C> {
    clock: C,
    audio: AudioWorker,
    video: VideoWorker,
    audio_cancellation: AudioCancellationToken,
    video_cancellation: CancellationToken,
}

impl MediaSession<MonotonicMediaClock> {
    /// Creates a session using a real monotonic wall clock.
    ///
    /// # Errors
    ///
    /// Returns [`TimelineError::Video`] or [`TimelineError::Audio`] when a
    /// bounded worker cannot be constructed.
    pub fn new(
        video_format: VideoFormat,
        video_capacity: usize,
        video_policy: DropPolicy,
        audio_format: AudioFormat,
        audio_capacity_frames: usize,
        audio_policy: AudioDropPolicy,
    ) -> Result<Self, TimelineError> {
        Self::with_clock(
            MonotonicMediaClock::start(),
            video_format,
            video_capacity,
            video_policy,
            audio_format,
            audio_capacity_frames,
            audio_policy,
        )
    }
}

impl<C> MediaSession<C>
where
    C: AudioClock + VideoClock,
{
    /// Creates a session with an injected clock for deterministic tests or a
    /// platform-adapted clock implementation.
    ///
    /// # Errors
    ///
    /// Returns [`TimelineError::Video`] or [`TimelineError::Audio`] when a
    /// bounded worker cannot be constructed.
    pub fn with_clock(
        clock: C,
        video_format: VideoFormat,
        video_capacity: usize,
        video_policy: DropPolicy,
        audio_format: AudioFormat,
        audio_capacity_frames: usize,
        audio_policy: AudioDropPolicy,
    ) -> Result<Self, TimelineError> {
        let video = VideoWorker::new(video_format, video_capacity, video_policy)
            .map_err(TimelineError::Video)?;
        let audio = AudioWorker::new(audio_format, audio_capacity_frames, audio_policy)
            .map_err(TimelineError::Audio)?;
        Ok(Self {
            clock,
            audio,
            video,
            audio_cancellation: AudioCancellationToken::new(),
            video_cancellation: CancellationToken::new(),
        })
    }

    /// Runs one audio block and one video frame per coordinated tick.
    ///
    /// Audio output is consumed once per tick after production, keeping this
    /// reference session bounded while leaving each worker's queue policy visible
    /// in the aggregate report.
    ///
    /// # Errors
    ///
    /// Returns [`MediaSessionError::Audio`] or [`MediaSessionError::Video`] for
    /// producer, pacing, or bounded-contract failures.
    pub fn run<VE, AE, VF, AF>(
        &mut self,
        audio_block_frames: usize,
        tick_count: u64,
        cancellation: &SessionCancellationToken,
        mut render_video: VF,
        mut produce_audio: AF,
    ) -> Result<MediaSessionReport, MediaSessionError<VE, AE>>
    where
        VF: FnMut(FrameDeadline, VideoFormat) -> Result<Option<VideoFrame>, VE>,
        AF: FnMut(AudioDeadline, AudioFormat, usize) -> Result<Option<AudioBuffer>, AE>,
    {
        let mut report = MediaSessionReport {
            requested_ticks: tick_count,
            ..MediaSessionReport::default()
        };
        for _ in 0..tick_count {
            if cancellation.is_cancelled() {
                break;
            }

            let audio = self
                .audio
                .run(
                    &mut self.clock,
                    audio_block_frames,
                    1,
                    &self.audio_cancellation,
                    &mut produce_audio,
                )
                .map_err(MediaSessionError::Audio)?;
            accumulate_audio(&mut report, audio);
            let _ = self.audio.take_next();
            if cancellation.is_cancelled() {
                break;
            }

            let video = self
                .video
                .run(
                    &mut self.clock,
                    1,
                    &self.video_cancellation,
                    &mut render_video,
                )
                .map_err(MediaSessionError::Video)?;
            accumulate_video(&mut report, video);
            report.completed_ticks = report.completed_ticks.saturating_add(1);
        }
        report.cancelled = cancellation.is_cancelled() && report.completed_ticks < tick_count;
        Ok(report)
    }
}

fn accumulate_audio(report: &mut MediaSessionReport, audio: obs_rs_audio::AudioWorkerReport) {
    report.audio_blocks = report.audio_blocks.saturating_add(audio.processed_blocks());
    report.audio_underflow_blocks = report
        .audio_underflow_blocks
        .saturating_add(audio.underflow_blocks());
    report.audio_dropped_oldest_frames = report
        .audio_dropped_oldest_frames
        .saturating_add(audio.dropped_oldest_frames());
    report.audio_dropped_newest_frames = report
        .audio_dropped_newest_frames
        .saturating_add(audio.dropped_newest_frames());
    report.audio_missed_deadlines = report
        .audio_missed_deadlines
        .saturating_add(audio.missed_deadlines());
    report.audio_lateness_nanos = report
        .audio_lateness_nanos
        .saturating_add(audio.total_lateness_nanos());
}

fn accumulate_video(report: &mut MediaSessionReport, video: obs_rs_video::VideoWorkerReport) {
    report.video_frames = report.video_frames.saturating_add(video.processed_frames());
    report.video_empty_frames = report
        .video_empty_frames
        .saturating_add(video.empty_frames());
    report.video_dropped_oldest = report
        .video_dropped_oldest
        .saturating_add(video.dropped_oldest());
    report.video_dropped_newest = report
        .video_dropped_newest
        .saturating_add(video.dropped_newest());
    report.video_missed_deadlines = report
        .video_missed_deadlines
        .saturating_add(video.missed_deadlines());
    report.video_lateness_nanos = report
        .video_lateness_nanos
        .saturating_add(video.total_lateness_nanos());
    report.video_wait_nanos = report
        .video_wait_nanos
        .saturating_add(video.total_wait_nanos());
    report.video_render_nanos = report
        .video_render_nanos
        .saturating_add(video.total_render_nanos());
    report.video_max_lateness_nanos = report
        .video_max_lateness_nanos
        .max(video.max_lateness_nanos());
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ManualClock {
        now: Timestamp,
    }

    impl AudioClock for ManualClock {
        fn now(&self) -> Timestamp {
            self.now
        }

        fn sleep_until(&mut self, deadline: Timestamp) {
            self.now = deadline;
        }
    }

    impl VideoClock for ManualClock {
        fn now(&self) -> Timestamp {
            self.now
        }

        fn sleep_until(&mut self, deadline: Timestamp) {
            self.now = deadline;
        }
    }

    fn video_rate() -> FrameRate {
        FrameRate::new(30, 1).expect("valid video rate")
    }

    fn audio_format() -> AudioFormat {
        AudioFormat::new(48_000, 2).expect("valid audio format")
    }

    fn video_format() -> VideoFormat {
        VideoFormat::new(2, 2, video_rate()).expect("valid video format")
    }

    #[test]
    fn audio_blocks_and_video_frames_share_exact_rational_boundaries() {
        let mut timeline = MediaTimeline::new(video_rate(), audio_format(), 100);
        for _ in 0..3 {
            let video = timeline.next_video_frame().expect("video deadline");
            let audio = timeline
                .next_audio_block(1_600)
                .expect("audio block deadline");
            assert_eq!(video.timestamp(), audio.timestamp());
            let observation = timeline.observe(video.timestamp(), audio.timestamp());
            assert_eq!(observation.delta_nanos(), 0);
        }

        assert_eq!(timeline.metrics().observations(), 3);
        assert_eq!(timeline.metrics().in_sync(), 3);
        assert_eq!(timeline.metrics().max_abs_delta_nanos(), 0);
    }

    #[test]
    fn long_run_timeline_keeps_audio_and_video_on_exact_boundaries() {
        const TICKS: u64 = 10_000;
        let mut timeline = MediaTimeline::new(video_rate(), audio_format(), 100);
        for _ in 0..TICKS {
            let video = timeline.next_video_frame().expect("video deadline");
            let audio = timeline
                .next_audio_block(1_600)
                .expect("audio block deadline");
            assert_eq!(video.timestamp(), audio.timestamp());
            assert_eq!(
                timeline
                    .observe(video.timestamp(), audio.timestamp())
                    .delta_nanos(),
                0
            );
        }

        let metrics = timeline.metrics();
        assert_eq!(metrics.observations(), TICKS);
        assert_eq!(metrics.in_sync(), TICKS);
        assert_eq!(metrics.audio_behind(), 0);
        assert_eq!(metrics.audio_ahead(), 0);
        assert_eq!(metrics.max_abs_delta_nanos(), 0);
    }

    #[test]
    fn timeline_reports_drift_and_reset_restores_both_domains() {
        let mut timeline = MediaTimeline::new(video_rate(), audio_format(), 100);
        let video = timeline.next_video_frame().expect("video deadline");
        let audio = timeline
            .next_audio_block(1_600)
            .expect("audio block deadline");
        let observation = timeline.observe(
            video.timestamp(),
            Timestamp::from_nanos(audio.timestamp().as_nanos() + 1_000),
        );
        assert_eq!(observation.delta_nanos(), 1_000);
        assert_ne!(observation.action(), obs_rs_audio::SyncAction::Keep);
        assert_eq!(timeline.metrics().audio_ahead(), 1);

        timeline.reset();
        assert_eq!(
            timeline
                .next_video_frame()
                .expect("reset video")
                .timestamp(),
            Timestamp::ZERO
        );
        assert_eq!(
            timeline
                .next_audio_block(1_600)
                .expect("reset audio")
                .timestamp(),
            Timestamp::ZERO
        );
        assert_eq!(timeline.metrics(), AvSyncMetrics::default());
    }

    #[test]
    fn rejects_zero_audio_blocks_without_advancing_video_schedule() {
        let mut timeline = MediaTimeline::new(video_rate(), audio_format(), 100);
        assert_eq!(
            timeline.next_audio_block(0),
            Err(TimelineError::Audio(AudioError::ZeroBlock))
        );
        assert_eq!(
            timeline.next_video_frame().expect("video deadline").index(),
            0
        );
    }

    #[test]
    fn one_clock_implements_both_worker_clock_traits() {
        let mut clock = MonotonicMediaClock::start();
        let mut video_pacer = obs_rs_video::VideoPacer::new(video_rate());
        let mut audio_pacer = obs_rs_audio::AudioPacer::new(audio_format());
        let video = video_pacer.next(&mut clock).expect("video pacing");
        let audio = audio_pacer.next(&mut clock, 1_600).expect("audio pacing");
        assert_eq!(video.deadline().timestamp(), Timestamp::ZERO);
        assert_eq!(audio.deadline().timestamp(), Timestamp::ZERO);
    }

    #[test]
    fn independent_clock_rejects_non_safe_rates() {
        assert_eq!(
            ClockRate::new(MAX_CLOCK_DRIFT_PPM + 1),
            Err(ClockRateError::DriftOutOfRange {
                ppm: MAX_CLOCK_DRIFT_PPM + 1
            })
        );
        assert_eq!(
            IndependentMediaClock::new(-MAX_CLOCK_DRIFT_PPM - 1, 0),
            Err(ClockRateError::DriftOutOfRange {
                ppm: -MAX_CLOCK_DRIFT_PPM - 1
            })
        );
    }

    #[test]
    fn independent_device_clocks_accumulate_drift_without_missed_wait_contracts() {
        const TICKS: usize = 3_000;
        let mut clock = IndependentMediaClock::new(1_000, -1_000).expect("valid drift rates");
        let mut audio_pacer = obs_rs_audio::AudioPacer::new(audio_format());
        let mut video_pacer = obs_rs_video::VideoPacer::new(video_rate());
        let controller = obs_rs_audio::AvSyncController::new(1_000_000);

        for _ in 0..TICKS {
            let audio = audio_pacer
                .next(&mut clock, 1_600)
                .expect("audio pacing succeeds");
            let video = video_pacer.next(&mut clock).expect("video pacing succeeds");
            assert!(audio.observed_at() >= audio.deadline().timestamp());
            assert!(video.observed_at() >= video.deadline().timestamp());
        }

        let observation = controller.observe(clock.video_now(), clock.audio_now());
        assert_eq!(observation.state(), obs_rs_audio::SyncState::AudioAhead);
        assert!(observation.delta_nanos() > 100_000_000);
        assert!(clock.audio_now() > clock.video_now());
        assert_eq!(clock.audio_rate().drift_ppm(), 1_000);
        assert_eq!(clock.video_rate().drift_ppm(), -1_000);
    }

    #[test]
    fn media_session_advances_both_workers_on_one_injected_clock() {
        let mut session = MediaSession::with_clock(
            ManualClock {
                now: Timestamp::ZERO,
            },
            video_format(),
            4,
            DropPolicy::DropOldest,
            audio_format(),
            3_200,
            AudioDropPolicy::DropOldest,
        )
        .expect("session");
        let cancellation = SessionCancellationToken::new();
        let report = session
            .run(
                1_600,
                2,
                &cancellation,
                |deadline, format| {
                    Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                        format,
                        deadline.timestamp(),
                        [1, 2, 3, 255],
                    )))
                },
                |deadline, format, frames| {
                    Ok::<_, std::convert::Infallible>(Some(
                        AudioBuffer::silence(format, deadline.timestamp(), frames)
                            .expect("audio block"),
                    ))
                },
            )
            .expect("session run");

        assert_eq!(report.requested_ticks(), 2);
        assert_eq!(report.completed_ticks(), 2);
        assert!(!report.cancelled());
        assert_eq!(report.audio_blocks(), 2);
        assert_eq!(report.video_frames(), 2);
        assert_eq!(report.audio_underflow_blocks(), 0);
        assert_eq!(report.video_empty_frames(), 0);
        assert_eq!(report.audio_missed_deadlines(), 0);
        assert_eq!(report.video_missed_deadlines(), 0);
    }

    #[test]
    fn media_session_cancellation_stops_before_the_next_tick() {
        let mut session = MediaSession::with_clock(
            ManualClock {
                now: Timestamp::ZERO,
            },
            video_format(),
            4,
            DropPolicy::DropOldest,
            audio_format(),
            3_200,
            AudioDropPolicy::DropOldest,
        )
        .expect("session");
        let cancellation = SessionCancellationToken::new();
        let callback_cancellation = cancellation.clone();
        let report = session
            .run(
                1_600,
                10,
                &cancellation,
                |deadline, format| {
                    if deadline.index() == 0 {
                        callback_cancellation.cancel();
                    }
                    Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                        format,
                        deadline.timestamp(),
                        [1, 2, 3, 255],
                    )))
                },
                |deadline, format, frames| {
                    Ok::<_, std::convert::Infallible>(Some(
                        AudioBuffer::silence(format, deadline.timestamp(), frames)
                            .expect("audio block"),
                    ))
                },
            )
            .expect("session run");

        assert_eq!(report.requested_ticks(), 10);
        assert_eq!(report.completed_ticks(), 1);
        assert!(report.cancelled());
        assert_eq!(report.audio_blocks(), 1);
        assert_eq!(report.video_frames(), 1);
        assert!(cancellation.is_cancelled());
    }
}
