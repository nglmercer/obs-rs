use super::*;
use std::time::Instant;

fn stinger_format() -> VideoFormat {
    VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format")
}

fn clip() -> StingerClip {
    let format = stinger_format();
    StingerClip::new(
        vec![
            VideoFrame::solid(format, Timestamp::ZERO, [0, 255, 0, 128]),
            VideoFrame::solid(format, Timestamp::ZERO, [255, 255, 255, 128]),
        ],
        vec![100_000_000, 100_000_000],
        500,
    )
    .expect("stinger clip")
}

#[test]
fn stinger_clip_selects_bounded_frames_and_cuts_at_the_transition_point() {
    let format = stinger_format();
    let source = VideoFrame::solid(format, Timestamp::ZERO, [255, 0, 0, 255]);
    let destination = VideoFrame::solid(format, Timestamp::from_millis(20), [0, 0, 255, 255]);
    let clip = clip();

    let before_cut = clip
        .render(&source, destination.clone(), 250)
        .expect("pre-cut Stinger frame");
    assert_eq!(before_cut.pixel(0, 0), Some([127, 128, 0, 255]));
    assert_eq!(before_cut.timestamp(), Timestamp::from_millis(20));

    let after_cut = clip
        .render(&source, destination, 750)
        .expect("post-cut Stinger frame");
    assert_eq!(after_cut.pixel(0, 0), Some([128, 128, 255, 255]));
    assert!(clip.destination_visible(500));
    assert!(!clip.destination_visible(499));
}

#[test]
fn stinger_clip_validation_keeps_resource_bounds_explicit() {
    let format = stinger_format();
    let frame = VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 0, 0]);
    assert_eq!(
        StingerClip::new(Vec::new(), Vec::new(), 500),
        Err(MediaError::InvalidStingerFrameCount { count: 0 })
    );
    assert_eq!(
        StingerClip::new(vec![frame.clone()], Vec::new(), 500),
        Err(MediaError::InvalidStingerFrameDurations {
            expected: 1,
            actual: 0,
        })
    );
    assert_eq!(
        StingerClip::new(vec![frame.clone()], vec![0], 500),
        Err(MediaError::InvalidStingerFrameDuration { duration_nanos: 0 })
    );
    assert_eq!(
        StingerClip::new(vec![frame], vec![100_000_000], 0),
        Err(MediaError::InvalidStingerTransitionPoint {
            transition_point_milli: 0,
        })
    );
}

#[test]
fn stinger_clip_rejects_bad_progress_and_format_mismatch() {
    let clip = clip();
    assert_eq!(
        clip.frame_at_progress(1_001, Timestamp::ZERO),
        Err(MediaError::InvalidTransition {
            progress_milli: 1_001,
        })
    );
    let other = VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
    let source = VideoFrame::solid(other, Timestamp::ZERO, [0, 0, 0, 255]);
    let destination = VideoFrame::solid(other, Timestamp::ZERO, [0, 0, 0, 255]);
    assert_eq!(
        clip.render(&source, destination, 500),
        Err(MediaError::FormatMismatch {
            expected: other,
            actual: clip.format(),
        })
    );
}

#[test]
#[ignore = "timing report, not a pass/fail assertion"]
fn stinger_transition_timing_report() {
    let format =
        VideoFormat::new(640, 360, FrameRate::new(60, 1).expect("rate")).expect("stinger format");
    let clip = StingerClip::new(
        vec![
            VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 0, 128]),
            VideoFrame::solid(format, Timestamp::ZERO, [255, 255, 255, 128]),
        ],
        vec![500_000_000, 500_000_000],
        500,
    )
    .expect("stinger clip");
    let source = VideoFrame::solid(format, Timestamp::ZERO, [16, 32, 64, 255]);
    let destination = VideoFrame::solid(format, Timestamp::from_millis(10), [64, 32, 16, 255]);
    let runs = 20_u32;
    let started = Instant::now();
    let mut checksum = 0_u64;
    for progress in 0..runs {
        let progress = u16::try_from(progress * 1_000 / runs).expect("progress fits");
        let frame = clip
            .render(&source, destination.clone(), progress)
            .expect("stinger frame");
        checksum = checksum.saturating_add(u64::from(frame.pixel(0, 0).expect("pixel")[0]));
    }
    println!(
        "stinger transition: {runs} frames x 640x360 = {:?} total (about {:?}/frame), checksum={checksum}",
        started.elapsed(),
        started.elapsed() / runs,
    );
}
