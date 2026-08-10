use obs_rs_audio::{
    AudioBuffer, AudioCancellationToken, AudioClock, AudioDeadline, AudioDropPolicy, AudioFormat,
    AudioWorker,
};
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_video::{CancellationToken, DropPolicy, FrameDeadline, VideoClock, VideoWorker};

use super::{
    cancellation::SessionCancellationToken,
    clock::MonotonicMediaClock,
    error::{MediaSessionError, TimelineError},
    report::MediaSessionReport,
};
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
