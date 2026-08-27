use obs_rs_media::{FrameTransition, LumaWipePattern, SlideDirection, TransitionKind};
use obs_rs_project::SceneSpec;
use obs_rs_ui::UiLocale;

pub(crate) fn transition_label_for_locale(locale: UiLocale, transition: FrameTransition) -> String {
    crate::i18n::with_catalog(locale, |text| match transition {
        FrameTransition::Cut => text.cut.to_string(),
        FrameTransition::CrossFade { progress_milli } => {
            format!("{} {progress_milli}/1000", text.fade)
        }
        FrameTransition::FadeToColor {
            progress_milli,
            color,
        } => format!(
            "{} {progress_milli}/1000 #{:02X}{:02X}{:02X}{:02X}",
            text.fade_to_color, color[0], color[1], color[2], color[3]
        ),
        FrameTransition::Slide {
            progress_milli,
            direction,
        } => format!(
            "{} {progress_milli}/1000 ({})",
            text.slide,
            direction.as_str()
        ),
        FrameTransition::Swipe {
            progress_milli,
            direction,
            swipe_in,
        } => {
            let label = if swipe_in {
                &text.swipe_in
            } else {
                &text.swipe
            };
            format!("{} {progress_milli}/1000 ({})", label, direction.as_str())
        }
        FrameTransition::LumaWipe {
            progress_milli,
            pattern,
            invert,
            softness_milli,
        } => format!(
            "{} {} {progress_milli}/1000 (softness {softness_milli}{})",
            text.luma_wipe,
            pattern.as_str(),
            if invert { ", invert" } else { "" }
        ),
    })
}

pub(crate) fn transition_kind(transition: FrameTransition) -> &'static str {
    match transition {
        FrameTransition::Cut => "cut",
        FrameTransition::CrossFade { .. } => "cross_fade",
        FrameTransition::FadeToColor { .. } => "fade_to_color",
        FrameTransition::Slide { .. } => "slide",
        FrameTransition::Swipe { .. } => "swipe",
        FrameTransition::LumaWipe { .. } => "luma_wipe",
    }
}

pub(crate) struct SceneStingerFields {
    pub(crate) path: String,
    pub(crate) transition_point: String,
    pub(crate) preload: bool,
    pub(crate) hardware_decode: bool,
}

pub(crate) struct SceneTransitionFields {
    pub(crate) index: i32,
    pub(crate) direction_index: i32,
    pub(crate) swipe_in: bool,
    pub(crate) luma_pattern_index: i32,
    pub(crate) luma_invert: bool,
    pub(crate) duration: String,
    pub(crate) color: String,
    pub(crate) softness: String,
    pub(crate) stinger: SceneStingerFields,
}

fn scene_stinger_fields(scene: Option<&SceneSpec>) -> SceneStingerFields {
    scene.and_then(SceneSpec::stinger_override).map_or_else(
        || SceneStingerFields {
            path: String::new(),
            transition_point: "500".to_owned(),
            preload: true,
            hardware_decode: false,
        },
        |stinger| SceneStingerFields {
            path: stinger.resource_path().to_owned(),
            transition_point: stinger.transition_point_milli().to_string(),
            preload: stinger.preload(),
            hardware_decode: stinger.hardware_decode(),
        },
    )
}

/// Projects persisted transition policy into the compact scene-properties
/// dialog model. Index zero deliberately means inheritance.
pub(crate) fn scene_transition_fields(scene: Option<&SceneSpec>) -> SceneTransitionFields {
    let stinger = scene_stinger_fields(scene);
    let Some(transition) = scene.and_then(SceneSpec::transition_override) else {
        return SceneTransitionFields {
            index: 0,
            direction_index: 0,
            swipe_in: false,
            luma_pattern_index: 0,
            luma_invert: false,
            duration: "300".to_owned(),
            color: "#000000FF".to_owned(),
            softness: "30".to_owned(),
            stinger,
        };
    };
    let (index, direction_index, swipe_in, luma_pattern_index, luma_invert, color, softness) =
        match transition.kind() {
            TransitionKind::Cut => (
                1,
                0,
                false,
                0,
                false,
                "#000000FF".to_owned(),
                "30".to_owned(),
            ),
            TransitionKind::CrossFade => (
                2,
                0,
                false,
                0,
                false,
                "#000000FF".to_owned(),
                "30".to_owned(),
            ),
            TransitionKind::FadeToColor { color } => (
                3,
                0,
                false,
                0,
                false,
                format!(
                    "#{:02X}{:02X}{:02X}{:02X}",
                    color[0], color[1], color[2], color[3]
                ),
                "30".to_owned(),
            ),
            TransitionKind::Slide { direction } => (
                4,
                slide_direction_index(direction),
                false,
                0,
                false,
                "#000000FF".to_owned(),
                "30".to_owned(),
            ),
            TransitionKind::Swipe {
                direction,
                swipe_in,
            } => (
                5,
                slide_direction_index(direction),
                swipe_in,
                0,
                false,
                "#000000FF".to_owned(),
                "30".to_owned(),
            ),
            TransitionKind::LumaWipe {
                pattern,
                invert,
                softness_milli,
            } => (
                6,
                0,
                false,
                luma_pattern_index(pattern),
                invert,
                "#000000FF".to_owned(),
                softness_milli.to_string(),
            ),
        };
    SceneTransitionFields {
        index,
        direction_index,
        swipe_in,
        luma_pattern_index,
        luma_invert,
        duration: transition.duration_millis().to_string(),
        color,
        softness,
        stinger,
    }
}

fn luma_pattern_index(pattern: LumaWipePattern) -> i32 {
    match pattern {
        LumaWipePattern::LinearHorizontal => 0,
        LumaWipePattern::LinearVertical => 1,
    }
}

fn slide_direction_index(direction: SlideDirection) -> i32 {
    match direction {
        SlideDirection::Left => 0,
        SlideDirection::Right => 1,
        SlideDirection::Up => 2,
        SlideDirection::Down => 3,
    }
}
