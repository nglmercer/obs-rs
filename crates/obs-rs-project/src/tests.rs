use super::*;
use obs_rs_config::Config;
use obs_rs_media::{
    ChromaKey, ColorCorrection, ColorKey, ColorMultiplyAdd, FrameFilter, FrameRate, FrameTransform,
    LumaKey, TransitionKind, TransitionSpec, VideoFormat,
};
use obs_rs_output::OutputProfileKind;
use obs_rs_util::Identifier;
use std::path::PathBuf;

fn format() -> VideoFormat {
    VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("rate")).expect("format")
}

fn settings() -> Config {
    let mut config = Config::new();
    config.set("color", "#102030FF").expect("color");
    config
}

fn project() -> Project {
    let mut project = Project::new("Studio | Demo").expect("project");
    let mut profile = Profile::new("live", "Live profile", format()).expect("profile");
    let mut scene = SceneSpec::new("main", "Main scene").expect("scene");
    let mut source =
        SourceSpec::new("background", "color_source", "Background", settings()).expect("source");
    let mut item = SceneItemSpec::for_source("background").expect("scene item");
    item.set_transform(
        FrameTransform::new(1_000, 1_000, 4, -3, true, false, 220).expect("transform"),
    );
    source
        .add_filter(
            SourceFilterSpec::new(
                "brightness",
                "Brightness",
                "brightness",
                Config::parse("milli = 750\n").expect("brightness settings"),
            )
            .expect("brightness filter"),
        )
        .expect("brightness filter attach");
    source
        .add_filter(
            SourceFilterSpec::new(
                "opacity",
                "Opacity",
                "opacity",
                Config::parse("value = 200\n").expect("opacity settings"),
            )
            .expect("opacity filter"),
        )
        .expect("opacity filter attach");
    scene.add_item(item).expect("item attach");
    profile.add_source(source).expect("source registry");
    profile.add_scene(scene).expect("scene attach");
    project.add_profile(profile).expect("profile add");
    project
}

fn unique_paths(label: &str) -> (PathBuf, PathBuf) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    let root = std::env::temp_dir();
    (
        root.join(format!("obs-rs-project-{label}-{token}.json")),
        root.join(format!("obs-rs-project-{label}-{token}.part")),
    )
}

#[path = "project_tests_batch_remove.rs"]
mod batch_remove;
#[path = "project_tests_commands.rs"]
mod commands;
#[path = "project_tests_groups.rs"]
mod groups;
#[path = "project_tests_groups_copy.rs"]
mod groups_copy;
#[path = "project_tests_history.rs"]
mod history;
#[path = "project_tests_migration.rs"]
mod migration;
#[path = "project_tests_round_trip.rs"]
mod round_trip;
