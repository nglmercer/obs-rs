use super::*;
use std::time::Instant;

#[test]
fn transitions_are_deterministic_and_validate_progress() {
    let source = VideoFrame::solid(format(), Timestamp::ZERO, [0, 0, 0, 0]);
    let destination = VideoFrame::solid(format(), Timestamp::from_millis(10), [100, 200, 255, 255]);
    let transition = FrameTransition::cross_fade(500).expect("valid progress");
    let halfway = VideoFrame::transitioned(&source, destination, transition).expect("transition");
    assert_eq!(halfway.timestamp(), Timestamp::from_millis(10));
    assert_eq!(halfway.pixel(0, 0), Some([50, 100, 128, 128]));
    assert_eq!(
        FrameTransition::cross_fade(1_001),
        Err(MediaError::InvalidTransition {
            progress_milli: 1_001
        })
    );
}

#[test]
fn transition_specs_keep_policy_separate_from_render_progress() {
    let color = [16, 32, 64, 128];
    let spec = TransitionSpec::fade_to_color(450, color).expect("valid policy");
    assert_eq!(spec.duration_millis(), 450);
    assert_eq!(spec.kind(), TransitionKind::FadeToColor { color });
    assert_eq!(
        spec.at_progress(500).expect("render sample"),
        FrameTransition::FadeToColor {
            progress_milli: 500,
            color,
        }
    );
    assert_eq!(
        TransitionSpec::from_frame_transition(
            FrameTransition::CrossFade {
                progress_milli: 500,
            },
            300,
        )
        .expect("cross-fade policy")
        .kind(),
        TransitionKind::CrossFade
    );
    assert_eq!(
        TransitionSpec::cross_fade(0),
        Err(MediaError::InvalidTransitionDuration { duration_millis: 0 })
    );
    assert_eq!(
        TransitionSpec::cross_fade(60_001),
        Err(MediaError::InvalidTransitionDuration {
            duration_millis: 60_001
        })
    );
}

#[test]
fn fade_to_color_covers_then_reveals_destination() {
    let source = VideoFrame::solid(format(), Timestamp::ZERO, [255, 0, 0, 255]);
    let destination = VideoFrame::solid(format(), Timestamp::from_millis(10), [0, 0, 255, 255]);
    let color = [0, 255, 0, 255];

    let covered = VideoFrame::transitioned(
        &source,
        destination.clone(),
        FrameTransition::fade_to_color(500, color).expect("valid transition"),
    )
    .expect("covered frame");
    assert_eq!(covered.pixel(0, 0), Some(color));

    let revealed = VideoFrame::transitioned(
        &source,
        destination,
        FrameTransition::fade_to_color(750, color).expect("valid transition"),
    )
    .expect("revealed frame");
    assert_eq!(revealed.pixel(0, 0), Some([0, 128, 128, 255]));

    assert_eq!(
        FrameTransition::fade_to_color(1_001, color),
        Err(MediaError::InvalidTransition {
            progress_milli: 1_001,
        })
    );
}

#[test]
fn slide_from_left_moves_the_source_out_before_the_destination_in() {
    let format =
        VideoFormat::new(4, 1, FrameRate::new(60, 1).expect("valid rate")).expect("slide format");
    let source = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255],
    )
    .expect("source frame");
    let destination = VideoFrame::new(
        format,
        Timestamp::from_millis(10),
        vec![9, 0, 0, 255, 8, 0, 0, 255, 7, 0, 0, 255, 6, 0, 0, 255],
    )
    .expect("destination frame");

    let halfway = VideoFrame::transitioned(
        &source,
        destination,
        FrameTransition::slide(500, SlideDirection::Left).expect("slide transition"),
    )
    .expect("halfway slide");
    assert_eq!(halfway.pixel(0, 0), Some([3, 0, 0, 255]));
    assert_eq!(halfway.pixel(1, 0), Some([4, 0, 0, 255]));
    assert_eq!(halfway.pixel(2, 0), Some([9, 0, 0, 255]));
    assert_eq!(halfway.pixel(3, 0), Some([8, 0, 0, 255]));
    assert_eq!(halfway.timestamp(), Timestamp::from_millis(10));
}

#[test]
fn slide_policy_round_trips_through_a_render_sample() {
    let policy = TransitionSpec::slide_left(600).expect("slide policy");
    assert_eq!(
        policy.kind(),
        TransitionKind::Slide {
            direction: SlideDirection::Left,
        }
    );
    assert_eq!(
        policy.at_progress(250).expect("slide sample"),
        FrameTransition::Slide {
            progress_milli: 250,
            direction: SlideDirection::Left,
        }
    );
    assert_eq!(
        FrameTransition::slide(1_001, SlideDirection::Left),
        Err(MediaError::InvalidTransition {
            progress_milli: 1_001,
        })
    );
}

#[test]
fn swipe_from_left_reveals_the_stationary_destination() {
    let format =
        VideoFormat::new(4, 1, FrameRate::new(60, 1).expect("valid rate")).expect("swipe format");
    let source = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255],
    )
    .expect("source frame");
    let destination = VideoFrame::new(
        format,
        Timestamp::from_millis(10),
        vec![9, 0, 0, 255, 8, 0, 0, 255, 7, 0, 0, 255, 6, 0, 0, 255],
    )
    .expect("destination frame");

    let halfway = VideoFrame::transitioned(
        &source,
        destination,
        FrameTransition::swipe(500, SlideDirection::Left).expect("swipe transition"),
    )
    .expect("halfway swipe");
    assert_eq!(halfway.pixel(0, 0), Some([3, 0, 0, 255]));
    assert_eq!(halfway.pixel(1, 0), Some([4, 0, 0, 255]));
    assert_eq!(halfway.pixel(2, 0), Some([7, 0, 0, 255]));
    assert_eq!(halfway.pixel(3, 0), Some([6, 0, 0, 255]));
    assert_eq!(halfway.timestamp(), Timestamp::from_millis(10));
}

#[test]
fn swipe_in_from_left_brings_destination_over_the_stationary_source() {
    let format =
        VideoFormat::new(4, 1, FrameRate::new(60, 1).expect("valid rate")).expect("swipe format");
    let source = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255],
    )
    .expect("source frame");
    let destination = VideoFrame::new(
        format,
        Timestamp::from_millis(10),
        vec![9, 0, 0, 255, 8, 0, 0, 255, 7, 0, 0, 255, 6, 0, 0, 255],
    )
    .expect("destination frame");

    let halfway = VideoFrame::transitioned(
        &source,
        destination,
        FrameTransition::swipe_in(500, SlideDirection::Left).expect("swipe-in transition"),
    )
    .expect("halfway swipe-in");
    assert_eq!(halfway.pixel(0, 0), Some([1, 0, 0, 255]));
    assert_eq!(halfway.pixel(1, 0), Some([2, 0, 0, 255]));
    assert_eq!(halfway.pixel(2, 0), Some([9, 0, 0, 255]));
    assert_eq!(halfway.pixel(3, 0), Some([8, 0, 0, 255]));

    let policy = TransitionSpec::swipe_in_left(600).expect("swipe-in policy");
    assert_eq!(
        policy.kind(),
        TransitionKind::Swipe {
            direction: SlideDirection::Left,
            swipe_in: true,
        }
    );
    assert_eq!(
        policy.at_progress(250).expect("swipe-in sample"),
        FrameTransition::Swipe {
            progress_milli: 250,
            direction: SlideDirection::Left,
            swipe_in: true,
        }
    );
}

#[test]
fn swipe_policy_round_trips_through_a_render_sample() {
    let policy = TransitionSpec::swipe_left(600).expect("swipe policy");
    assert_eq!(
        policy.kind(),
        TransitionKind::Swipe {
            direction: SlideDirection::Left,
            swipe_in: false,
        }
    );
    assert_eq!(
        policy.at_progress(250).expect("swipe sample"),
        FrameTransition::Swipe {
            progress_milli: 250,
            direction: SlideDirection::Left,
            swipe_in: false,
        }
    );
    assert_eq!(
        FrameTransition::swipe(1_001, SlideDirection::Left),
        Err(MediaError::InvalidTransition {
            progress_milli: 1_001,
        })
    );
}

#[test]
fn slide_supports_all_reference_directions() {
    let (source, destination) = directional_frames();
    let cases = [
        (SlideDirection::Left, [[2_u8, 9], [4, 7]]),
        (SlideDirection::Right, [[8, 1], [6, 3]]),
        (SlideDirection::Up, [[3, 4], [9, 8]]),
        (SlideDirection::Down, [[7, 6], [1, 2]]),
    ];

    for (direction, expected) in cases {
        let frame = VideoFrame::transitioned(
            &source,
            destination.clone(),
            FrameTransition::slide(500, direction).expect("slide direction"),
        )
        .expect("directional slide");
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(
                    frame.pixel(x, y).map(|pixel| pixel[0]),
                    Some(expected[y as usize][x as usize]),
                    "direction {direction:?} at ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn swipe_supports_all_reference_directions() {
    let (source, destination) = directional_frames();
    let cases = [
        (SlideDirection::Left, [[2_u8, 8], [4, 6]]),
        (SlideDirection::Right, [[9, 1], [7, 3]]),
        (SlideDirection::Up, [[3, 4], [7, 6]]),
        (SlideDirection::Down, [[9, 8], [1, 2]]),
    ];

    for (direction, expected) in cases {
        let frame = VideoFrame::transitioned(
            &source,
            destination.clone(),
            FrameTransition::swipe(500, direction).expect("swipe direction"),
        )
        .expect("directional swipe");
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(
                    frame.pixel(x, y).map(|pixel| pixel[0]),
                    Some(expected[y as usize][x as usize]),
                    "direction {direction:?} at ({x}, {y})"
                );
            }
        }
    }
}

fn directional_frames() -> (VideoFrame, VideoFrame) {
    let format =
        VideoFormat::new(2, 2, FrameRate::new(60, 1).expect("valid rate")).expect("format");
    let source = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255],
    )
    .expect("source frame");
    let destination = VideoFrame::new(
        format,
        Timestamp::from_millis(10),
        vec![9, 0, 0, 255, 8, 0, 0, 255, 7, 0, 0, 255, 6, 0, 0, 255],
    )
    .expect("destination frame");
    (source, destination)
}

#[test]
fn luma_wipe_linear_patterns_reveal_destination_and_round_trip_policy() {
    let format =
        VideoFormat::new(3, 1, FrameRate::new(60, 1).expect("valid rate")).expect("format");
    let source = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![10, 0, 0, 255, 10, 0, 0, 255, 10, 0, 0, 255],
    )
    .expect("source");
    let destination = VideoFrame::new(
        format,
        Timestamp::from_millis(10),
        vec![200, 0, 0, 255, 200, 0, 0, 255, 200, 0, 0, 255],
    )
    .expect("destination");

    let frame = VideoFrame::transitioned(
        &source,
        destination.clone(),
        FrameTransition::luma_wipe(500, LumaWipePattern::LinearHorizontal, false, 0)
            .expect("hard luma wipe"),
    )
    .expect("horizontal wipe");
    assert_eq!(frame.pixel(0, 0).map(|pixel| pixel[0]), Some(200));
    assert_eq!(frame.pixel(1, 0).map(|pixel| pixel[0]), Some(200));
    assert_eq!(frame.pixel(2, 0).map(|pixel| pixel[0]), Some(10));

    let inverted = VideoFrame::transitioned(
        &source,
        destination,
        FrameTransition::luma_wipe(500, LumaWipePattern::LinearHorizontal, true, 0)
            .expect("inverted luma wipe"),
    )
    .expect("inverted wipe");
    assert_eq!(inverted.pixel(0, 0).map(|pixel| pixel[0]), Some(10));
    assert_eq!(inverted.pixel(1, 0).map(|pixel| pixel[0]), Some(200));
    assert_eq!(inverted.pixel(2, 0).map(|pixel| pixel[0]), Some(200));

    let policy = TransitionSpec::luma_wipe(
        650,
        LumaWipePattern::LinearVertical,
        true,
        DEFAULT_LUMA_WIPE_SOFTNESS_MILLI,
    )
    .expect("luma policy");
    assert_eq!(
        policy.kind(),
        TransitionKind::LumaWipe {
            pattern: LumaWipePattern::LinearVertical,
            invert: true,
            softness_milli: DEFAULT_LUMA_WIPE_SOFTNESS_MILLI,
        }
    );
    assert_eq!(
        policy.at_progress(250).expect("luma sample"),
        FrameTransition::LumaWipe {
            progress_milli: 250,
            pattern: LumaWipePattern::LinearVertical,
            invert: true,
            softness_milli: DEFAULT_LUMA_WIPE_SOFTNESS_MILLI,
        }
    );
}

#[test]
fn luma_wipe_softness_blends_the_transition_edge_without_allocating_a_mask() {
    let format =
        VideoFormat::new(3, 1, FrameRate::new(60, 1).expect("valid rate")).expect("format");
    let source = VideoFrame::solid(format, Timestamp::ZERO, [10, 20, 30, 255]);
    let destination = VideoFrame::solid(format, Timestamp::from_millis(10), [200, 210, 220, 255]);
    let frame = VideoFrame::transitioned(
        &source,
        destination,
        FrameTransition::luma_wipe(500, LumaWipePattern::LinearHorizontal, false, 200)
            .expect("soft luma wipe"),
    )
    .expect("soft wipe");

    assert_eq!(frame.pixel(0, 0), Some([200, 210, 220, 255]));
    assert_eq!(frame.pixel(1, 0), Some([105, 115, 125, 255]));
    assert_eq!(frame.pixel(2, 0), Some([10, 20, 30, 255]));
    assert_eq!(
        FrameTransition::luma_wipe(
            500,
            LumaWipePattern::LinearHorizontal,
            false,
            MAX_LUMA_WIPE_SOFTNESS_MILLI + 1,
        ),
        Err(MediaError::InvalidLumaWipeSoftness {
            softness_milli: MAX_LUMA_WIPE_SOFTNESS_MILLI + 1,
        })
    );
}

#[test]
#[ignore = "timing report, not a pass/fail assertion"]
fn slide_transition_timing_report() {
    let format = VideoFormat::new(640, 360, FrameRate::new(60, 1).expect("valid rate"))
        .expect("slide format");
    let source = VideoFrame::solid(format, Timestamp::ZERO, [16, 32, 64, 255]);
    let destination = VideoFrame::solid(format, Timestamp::from_millis(10), [64, 32, 16, 255]);
    let runs = 20_u32;
    let directions = [
        SlideDirection::Left,
        SlideDirection::Right,
        SlideDirection::Up,
        SlideDirection::Down,
    ];
    let samples = runs.saturating_mul(
        u32::try_from(directions.len()).expect("direction count fits in a sample count"),
    );
    let started = Instant::now();
    let mut checksum = 0_u64;
    for direction in directions {
        for progress in 0..runs {
            let progress = u16::try_from(progress * 1_000 / runs).expect("progress fits");
            let frame = VideoFrame::transitioned(
                &source,
                destination.clone(),
                FrameTransition::slide(progress, direction).expect("slide sample"),
            )
            .expect("slide frame");
            checksum = checksum.saturating_add(u64::from(frame.pixel(0, 0).expect("pixel")[0]));
        }
    }
    println!(
        "slide transition: {samples} frames x 640x360 = {:?} total (about {:?}/frame), checksum={checksum}",
        started.elapsed(),
        started.elapsed() / samples,
    );
}

#[test]
#[ignore = "timing report, not a pass/fail assertion"]
fn swipe_transition_timing_report() {
    let format = VideoFormat::new(640, 360, FrameRate::new(60, 1).expect("valid rate"))
        .expect("swipe format");
    let source = VideoFrame::solid(format, Timestamp::ZERO, [16, 32, 64, 255]);
    let destination = VideoFrame::solid(format, Timestamp::from_millis(10), [64, 32, 16, 255]);
    let runs = 20_u32;
    let directions = [
        SlideDirection::Left,
        SlideDirection::Right,
        SlideDirection::Up,
        SlideDirection::Down,
    ];
    let samples = runs.saturating_mul(
        u32::try_from(directions.len()).expect("direction count fits in a sample count"),
    );
    let started = Instant::now();
    let mut checksum = 0_u64;
    for direction in directions {
        for progress in 0..runs {
            let progress = u16::try_from(progress * 1_000 / runs).expect("progress fits");
            let frame = VideoFrame::transitioned(
                &source,
                destination.clone(),
                FrameTransition::swipe(progress, direction).expect("swipe sample"),
            )
            .expect("swipe frame");
            checksum = checksum.saturating_add(u64::from(frame.pixel(0, 0).expect("pixel")[0]));
        }
    }
    println!(
        "swipe transition: {samples} frames x 640x360 = {:?} total (about {:?}/frame), checksum={checksum}",
        started.elapsed(),
        started.elapsed() / samples,
    );
}

#[test]
fn fast_division_by_255_matches_integer_division() {
    // Blending feeds this at most `255 * 255 * 2`, so the identity is checked
    // across the whole range a composite can produce.
    for value in 0..=(255_u32 * 255 * 2) {
        assert_eq!(
            crate::frame::divide_by_255(value),
            value / 255,
            "divide_by_255({value})"
        );
    }
}
