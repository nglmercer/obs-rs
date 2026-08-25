use super::*;
use crate::helpers::escape_html;
use obs_rs_audio::AudioBuffer;
use obs_rs_audio::AudioFormat;
use obs_rs_config::Config;
use obs_rs_media::{
    FrameRate, FrameTransition, MediaError, Timestamp, TransitionSpec, VideoFormat,
};
use obs_rs_project::{
    Profile, Project, ProjectCommand, ProjectFileStore, SceneItemDuplicateMode, SceneItemSpec,
    SceneSpec, SourceSpec,
};
use std::time::{Duration, Instant};

fn project() -> Project {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut project = Project::new("UI fixture").expect("project");
    let mut profile = Profile::new("live", "Live", format).expect("profile");
    profile
        .add_scene(SceneSpec::new("preview", "Preview").expect("scene"))
        .expect("scene");
    profile
        .add_scene(SceneSpec::new("program", "Program").expect("scene"))
        .expect("scene");
    let mut source_scene = SceneSpec::new("source_scene", "Source").expect("scene");
    source_scene
        .add_item(SceneItemSpec::for_source("source").expect("scene item"))
        .expect("item");
    profile
        .add_source(
            SourceSpec::new("source", "color_source", "Color", Config::new()).expect("source"),
        )
        .expect("source registry");
    profile.add_scene(source_scene).expect("scene");
    project.add_profile(profile).expect("profile");
    project
}

#[path = "ui_tests_persistence.rs"]
mod persistence;
#[path = "ui_tests_selection.rs"]
mod selection;
#[path = "ui_tests_shortcuts.rs"]
mod shortcuts;
#[path = "ui_tests_stinger_loader.rs"]
mod stinger_loader;
