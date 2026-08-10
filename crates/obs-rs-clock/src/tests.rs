use super::*;
use obs_rs_audio::{
    AudioBuffer, AudioClock, AudioDropPolicy, AudioError, AudioFormat, AvSyncMetrics,
};
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
use obs_rs_video::{DropPolicy, VideoClock};
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
