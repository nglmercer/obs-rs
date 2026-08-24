use super::*;

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
