use super::*;
use std::sync::Arc;

fn format() -> VideoFormat {
    VideoFormat::new(2, 2, FrameRate::new(60, 2).expect("valid rate")).expect("valid format")
}

#[path = "media_tests_color.rs"]
mod color;
#[path = "media_tests_composition.rs"]
mod composition;
#[path = "media_tests_frames.rs"]
mod frames;
#[path = "media_tests_stinger.rs"]
mod stinger;
#[path = "media_tests_stinger_loader.rs"]
mod stinger_loader;
#[path = "media_tests_transitions.rs"]
mod transitions;
