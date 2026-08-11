use super::*;
use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameRate, FrameTransform, VideoFormat};
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
    source.set_transform(
        FrameTransform::new(1_000, 1_000, 4, -3, true, false, 220).expect("transform"),
    );
    source.add_filter(FrameFilter::Brightness { milli: 750 });
    source.add_filter(FrameFilter::Opacity(200));
    scene.add_source(source).expect("source attach");
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
        root.join(format!("obs-rs-project-{label}-{token}.txt")),
        root.join(format!("obs-rs-project-{label}-{token}.part")),
    )
}

#[test]
fn project_round_trips_deterministically_with_escaped_values() {
    let project = project();
    let encoded = project.serialize();
    let decoded = Project::parse(&encoded).expect("parse project");

    assert_eq!(decoded, project);
    assert_eq!(decoded.serialize(), encoded);
    assert!(encoded.contains("Studio%20%7C%20Demo"));
}

#[test]
fn parser_keeps_legacy_sources_visible_and_unlocked() {
    let encoded = project().serialize();
    let legacy = encoded
        .lines()
        .map(|line| {
            if line.starts_with("source|") {
                line.split('|').take(9).collect::<Vec<_>>().join("|")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let decoded = Project::parse(&legacy).expect("legacy project parses");
    let source = decoded
        .profiles()
        .next()
        .and_then(|profile| profile.scenes().next())
        .and_then(|scene| scene.sources().first())
        .expect("legacy source");
    assert!(source.visible());
    assert!(!source.locked());
}

#[test]
fn command_session_tracks_dirty_state_and_rejects_bad_references() {
    let mut session = ProjectSession::new(project());
    assert!(!session.is_dirty());
    let source =
        SourceSpec::new("foreground", "test_pattern", "Foreground", Config::new()).expect("source");
    session
        .dispatch(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source,
        })
        .expect("add source command");
    let mut replacement_settings = Config::new();
    replacement_settings
        .set("color", "#203040FF")
        .expect("replacement settings");
    session
        .dispatch(ProjectCommand::SetSourceSettings {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            settings: replacement_settings,
        })
        .expect("set source settings command");
    session
        .dispatch(ProjectCommand::SetSourceFilters {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            filters: vec![FrameFilter::Grayscale],
        })
        .expect("set source filters command");
    session
        .dispatch(ProjectCommand::SetSourceVisibility {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            visible: false,
        })
        .expect("set source visibility command");
    session
        .dispatch(ProjectCommand::SetSourceLocked {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            locked: true,
        })
        .expect("set source locked command");
    session
        .dispatch(ProjectCommand::MoveSource {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            target_index: 1,
        })
        .expect("move source command");
    session
        .dispatch(ProjectCommand::RemoveSource {
            profile: "live".to_owned(),
            scene: "main".to_owned(),
            source: "foreground".to_owned(),
        })
        .expect("remove source command");
    assert!(session.is_dirty());
    let saved = session.save();
    assert!(!session.is_dirty());
    assert_eq!(
        Project::parse(&saved).expect("saved project"),
        *session.project()
    );
    let source = session
        .project()
        .profiles()
        .next()
        .and_then(|profile| profile.scenes().next())
        .and_then(|scene| {
            scene
                .sources()
                .iter()
                .find(|source| source.id().as_str() == "background")
        })
        .expect("background source");
    assert!(!source.visible());
    assert!(source.locked());

    assert_eq!(
        session.dispatch(ProjectCommand::SetSourceTransform {
            profile: "missing".to_owned(),
            scene: "main".to_owned(),
            source: "background".to_owned(),
            transform: FrameTransform::IDENTITY,
        }),
        Err(ProjectError::UnknownProfile(
            Identifier::new("missing").expect("identifier")
        ))
    );
    assert!(!session.is_dirty());
}

#[test]
fn remove_scene_command_updates_project_without_partial_mutation() {
    let mut project = project();
    let extra = SceneSpec::new("extra", "Extra").expect("scene");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: extra,
        })
        .expect("add scene");
    project
        .apply(ProjectCommand::SetSceneName {
            profile: "live".to_owned(),
            scene: "extra".to_owned(),
            name: "Renamed".to_owned(),
        })
        .expect("rename scene");
    assert_eq!(
        project
            .profiles()
            .next()
            .expect("profile")
            .scenes()
            .find(|scene| scene.id().as_str() == "extra")
            .expect("renamed scene")
            .name(),
        "Renamed"
    );
    project
        .apply(ProjectCommand::RemoveScene {
            profile: "live".to_owned(),
            scene: "extra".to_owned(),
        })
        .expect("remove scene");
    assert!(project
        .profiles()
        .next()
        .expect("profile")
        .scenes()
        .all(|scene| scene.id().as_str() != "extra"));
    assert_eq!(
        project.apply(ProjectCommand::RemoveScene {
            profile: "live".to_owned(),
            scene: "missing".to_owned(),
        }),
        Err(ProjectError::UnknownScene(
            Identifier::new("missing").expect("id")
        ))
    );
}

#[test]
fn parser_rejects_duplicate_and_unknown_records() {
    let project = project();
    let mut duplicate = project.serialize();
    duplicate.push_str("profile|live|Other|640|360|30|1\n");
    assert!(matches!(
        Project::parse(&duplicate),
        Err(ProjectError::DuplicateProfile(_))
    ));

    let unknown = format!("{MAGIC}\nproject|title|live\nunknown|value\n");
    assert!(matches!(
        Project::parse(&unknown),
        Err(ProjectError::InvalidDocument { .. })
    ));
}

#[test]
fn project_file_store_commits_atomically_and_loads_the_same_state() {
    let (final_path, temp_path) = unique_paths("save");
    let store = ProjectFileStore::new(&final_path, &temp_path).expect("store");
    let mut session = ProjectSession::new(project());
    session
        .dispatch(ProjectCommand::SetActiveProfile {
            id: "live".to_owned(),
        })
        .expect("select profile");
    assert!(session.is_dirty());

    let bytes = store.save(&mut session).expect("save project");
    assert!(bytes > 0);
    assert!(!session.is_dirty());
    assert!(!temp_path.exists());
    assert_eq!(store.load().expect("load project"), *session.project());
    std::fs::remove_file(final_path).expect("remove project fixture");
}

#[test]
fn project_file_store_recovers_a_valid_unswitched_temporary_file() {
    let (final_path, temp_path) = unique_paths("recovery");
    let store = ProjectFileStore::new(&final_path, &temp_path).expect("store");
    let project = project();
    std::fs::write(&temp_path, project.serialize()).expect("write recovery fixture");

    assert_eq!(store.recover().expect("recover project"), Some(project));
    assert!(!final_path.exists());
    std::fs::remove_file(temp_path).expect("remove recovery fixture");
}

#[test]
fn set_profile_video_format_command_applies_and_round_trips() {
    let mut project = project();
    let updated =
        VideoFormat::new(1920, 1080, FrameRate::new(60, 1).expect("rate")).expect("format");

    project
        .apply(ProjectCommand::SetProfileVideoFormat {
            profile: "live".to_owned(),
            format: updated,
        })
        .expect("video format applies");

    let profile = project.profiles().next().expect("profile");
    assert_eq!(profile.video_format(), updated);

    let decoded = Project::parse(&project.serialize()).expect("parse project");
    assert_eq!(
        decoded.profiles().next().expect("profile").video_format(),
        updated
    );
}

#[test]
fn set_profile_video_format_command_rejects_an_unknown_profile() {
    let mut project = project();

    let error = project
        .apply(ProjectCommand::SetProfileVideoFormat {
            profile: "missing".to_owned(),
            format: format(),
        })
        .expect_err("unknown profile is rejected");

    assert!(matches!(error, ProjectError::UnknownProfile(_)));
}

fn rename_scene(name: &str) -> ProjectCommand {
    ProjectCommand::SetSceneName {
        profile: "live".to_owned(),
        scene: "main".to_owned(),
        name: name.to_owned(),
    }
}

fn scene_name(session: &ProjectSession) -> String {
    session
        .project()
        .profile("live")
        .expect("profile")
        .scene("main")
        .expect("scene")
        .name()
        .to_owned()
}

#[test]
fn undo_restores_the_state_before_the_last_accepted_command() {
    let mut session = ProjectSession::new(project());
    assert!(!session.can_undo(), "a fresh session has nothing to undo");

    session.dispatch(rename_scene("Renamed")).expect("rename");
    assert_eq!(scene_name(&session), "Renamed");
    assert!(session.can_undo());

    let before_undo = session.revision();
    assert!(session.undo());
    assert_eq!(scene_name(&session), "Main scene");
    assert!(session.can_redo());
    assert_ne!(
        session.revision(),
        before_undo,
        "an undo is a change observers must see"
    );

    assert!(session.redo());
    assert_eq!(scene_name(&session), "Renamed");
    assert!(!session.can_redo());
}

#[test]
fn undo_and_redo_are_no_ops_at_the_ends_of_the_history() {
    let mut session = ProjectSession::new(project());

    assert!(!session.undo(), "nothing precedes a fresh session");
    assert!(!session.redo(), "nothing has been undone yet");

    session.dispatch(rename_scene("Renamed")).expect("rename");
    assert!(session.undo());
    assert!(
        !session.undo(),
        "the history bottom is reached exactly once"
    );
    assert_eq!(scene_name(&session), "Main scene");
}

#[test]
fn a_rejected_command_records_no_undo_step() {
    let mut session = ProjectSession::new(project());

    session
        .dispatch(ProjectCommand::SetSceneName {
            profile: "live".to_owned(),
            scene: "missing".to_owned(),
            name: "Renamed".to_owned(),
        })
        .expect_err("an unknown scene is rejected");

    assert!(
        !session.can_undo(),
        "a rejected command must not become an undoable step"
    );
    assert!(!session.is_dirty());
}

#[test]
fn a_new_edit_discards_the_redo_branch() {
    let mut session = ProjectSession::new(project());
    session.dispatch(rename_scene("First")).expect("first");
    assert!(session.undo());
    assert!(session.can_redo());

    session.dispatch(rename_scene("Second")).expect("second");

    assert!(
        !session.can_redo(),
        "redoing onto a diverged state would reapply a replaced edit"
    );
    assert_eq!(scene_name(&session), "Second");
}

#[test]
fn history_is_bounded_and_drops_the_oldest_states_first() {
    let mut session = ProjectSession::new(project());
    // One more edit than the bound, so the very first state must have aged out.
    for index in 0..=MAX_HISTORY_DEPTH {
        session
            .dispatch(rename_scene(&format!("Take {index}")))
            .expect("rename");
    }

    let mut undone = 0;
    while session.undo() {
        undone += 1;
    }

    assert_eq!(undone, MAX_HISTORY_DEPTH);
    assert_eq!(
        scene_name(&session),
        "Take 0",
        "the oldest retained state is the one after the dropped original"
    );
}

#[test]
fn loading_a_project_clears_the_history() {
    let mut session = ProjectSession::new(project());
    session.dispatch(rename_scene("Renamed")).expect("rename");
    assert!(session.can_undo());

    session.replace(project());

    assert!(
        !session.can_undo() && !session.can_redo(),
        "undoing across a load would resurrect an unrelated project"
    );
}
