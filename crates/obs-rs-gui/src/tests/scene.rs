use super::*;

#[test]
fn app_settings_round_trip_the_selected_audio_input() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-settings-{token}.toml"));
    let settings = AppSettings {
        audio_input_id: "pipewire-node-42".to_owned(),
        audio_monitor_output_id: "pipewire-output-7".to_owned(),
        microphone_monitor_mode: obs_rs_audio::AudioMonitorMode::MonitorOnly,
        desktop_audio_monitor_mode: obs_rs_audio::AudioMonitorMode::MonitorAndOutput,
        audio_input_sync_offset_millis: 125,
        desktop_audio_sync_offset_millis: 2_500,
        ..AppSettings::default()
    };
    settings.save(&path).expect("settings should save");
    assert_eq!(
        AppSettings::load(&path).audio_input_id,
        settings.audio_input_id
    );
    let reloaded = AppSettings::load(&path);
    assert_eq!(
        reloaded.audio_input_sync_offset_millis,
        settings.audio_input_sync_offset_millis
    );
    assert_eq!(
        reloaded.desktop_audio_sync_offset_millis,
        settings.desktop_audio_sync_offset_millis
    );
    assert_eq!(
        reloaded.audio_monitor_output_id,
        settings.audio_monitor_output_id
    );
    assert_eq!(
        reloaded.microphone_monitor_mode,
        settings.microphone_monitor_mode
    );
    assert_eq!(
        reloaded.desktop_audio_monitor_mode,
        settings.desktop_audio_monitor_mode
    );
    std::fs::remove_file(path).expect("remove settings fixture");
}

#[test]
fn startup_restores_persisted_scene_selection_after_project_load() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-scenes-{token}.json"));
    let path_text = path.to_string_lossy().into_owned();
    let state = Rc::new(RefCell::new(DesktopState::new(
        initial_project().expect("initial project"),
    )));
    let store = crate::project_store(&path_text).expect("project store");
    state
        .borrow_mut()
        .save_project(&store)
        .expect("project fixture should save");

    let settings = AppSettings {
        project_path: path_text,
        last_preview_scene: "intermission".to_owned(),
        last_program_scene: "program".to_owned(),
        ..AppSettings::default()
    };
    let message = restore_project(&state, &settings).expect("project should restore");

    assert!(message.starts_with("Restored project"));
    assert_eq!(state.borrow().preview_scene(), Some("intermission"));
    assert_eq!(state.borrow().program_scene(), Some("program"));

    std::fs::remove_file(path).expect("remove project fixture");
}

#[test]
fn startup_prefers_the_document_scoped_scene_selection() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-document-scenes-{token}.json"));
    let path_text = path.to_string_lossy().into_owned();
    let state = Rc::new(RefCell::new(DesktopState::new(
        initial_project().expect("initial project"),
    )));
    let store = crate::project_store(&path_text).expect("project store");
    state
        .borrow_mut()
        .save_project(&store)
        .expect("project fixture should save");
    state.borrow_mut().set_project_selection_key(&path_text);

    let settings = AppSettings {
        project_path: path_text.clone(),
        last_preview_scene: "preview".to_owned(),
        last_program_scene: "preview".to_owned(),
        project_scene_selections: vec![ProjectSceneSelection::new(
            path_text,
            "live",
            Some("intermission".to_owned()),
            Some("program".to_owned()),
        )],
        ..AppSettings::default()
    };
    restore_project(&state, &settings).expect("project should restore");
    state
        .borrow_mut()
        .restore_project_selections(&settings.project_scene_selections);
    state
        .borrow_mut()
        .restore_project_selection_for_current_key();

    assert_eq!(state.borrow().preview_scene(), Some("intermission"));
    assert_eq!(state.borrow().program_scene(), Some("program"));

    std::fs::remove_file(path).expect("remove project fixture");
}

#[test]
fn app_settings_round_trip_the_window_layout() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-layout-{token}.toml"));
    let mut settings = AppSettings::default();
    settings.layout.panel_order = vec![4, 3, 2, 1, 0, 5];
    settings.layout.show_mixer = false;
    settings.layout.view_mode = 0;
    settings.layout.dock_height = 320;
    settings.layout.panel_weights = vec![1.5, 0.8, 2.0, 1.0, 1.2, 1.1];
    settings.layout.dock_tree =
        DockNode::from_legacy(&settings.layout.panel_order, &settings.layout.panel_weights)
            .expect("test layout should have a valid dock tree");
    settings.layout.floating_panels = vec![2, 3];
    settings.restore_project = false;
    settings.save_project_on_exit = false;

    settings.save(&path).expect("settings should save");
    let reloaded = AppSettings::load(&path);

    assert_eq!(reloaded, settings);
    std::fs::remove_file(path).expect("remove settings fixture");
}

#[test]
fn a_layout_that_lost_a_dock_falls_back_to_the_default_order() {
    let mut config = obs_rs_config::Config::new();
    config
        .set("layout_panel_order", "1,0,2,3")
        .expect("panel order key");
    config
        .set("layout_dock_height", "9999")
        .expect("dock height key");
    let document = config.serialize();
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-gui-layout-invalid-{token}.toml"));
    std::fs::write(&path, document).expect("write settings fixture");

    let settings = AppSettings::load(&path);

    let defaults = AppSettings::default();
    assert_eq!(settings.layout.panel_order, defaults.layout.panel_order);
    assert_eq!(settings.layout.dock_height, defaults.layout.dock_height);
    std::fs::remove_file(path).expect("remove settings fixture");
}

#[test]
fn output_runtime_switches_the_selected_audio_input_without_rebuilding_video() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);
    assert_eq!(output.audio_input_id(), None);
    output
        .set_audio_input_id(Some("missing-pipewire-input"))
        .expect("switch should fall back safely");
    assert_eq!(output.audio_input_id(), Some("missing-pipewire-input"));
    output
        .set_audio_input_id(None)
        .expect("automatic input should be accepted");
    assert_eq!(output.audio_input_id(), None);
}

#[test]
fn output_runtime_applies_bounded_audio_sync_offsets_on_the_worker() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);

    output
        .set_channel_sync_offset_millis(crate::MIC_CHANNEL_ID, 125)
        .expect("microphone offset should reach the worker");
    output
        .set_channel_sync_offset_millis(crate::DESKTOP_CHANNEL_ID, 2_500)
        .expect("desktop offset should reach the worker");
    assert!(output
        .set_channel_sync_offset_millis(crate::MIC_CHANNEL_ID, 5_001)
        .is_err());
}

#[test]
fn output_runtime_applies_audio_format_changes_at_the_idle_boundary() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let output = Rc::new(RefCell::new(OutputRuntime::new(format)));
    let mono = obs_rs_audio::AudioFormat::new(44_100, 1).expect("mono format");
    let stereo = obs_rs_audio::AudioFormat::new(48_000, 2).expect("stereo format");

    output
        .borrow_mut()
        .set_audio_format(mono)
        .expect("idle audio format should reach the worker");
    assert_eq!(output.borrow().audio_format(), mono);

    output.borrow_mut().stage_audio_format(stereo);
    assert!(output.borrow().has_staged_audio_format());
    let applied = crate::callbacks::settings::apply_staged_audio_format(&output)
        .expect("a staged audio format is pending")
        .expect("the staged format applies");
    assert_eq!(applied, stereo);
    assert_eq!(output.borrow().audio_format(), stereo);
}

#[test]
fn output_runtime_applies_monitor_controls_on_the_worker() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut output = OutputRuntime::new(format);

    output
        .set_channel_monitor_mode(
            crate::MIC_CHANNEL_ID,
            obs_rs_audio::AudioMonitorMode::MonitorOnly,
        )
        .expect("microphone monitor mode should reach the worker");
    output
        .set_channel_monitor_mode(
            crate::DESKTOP_CHANNEL_ID,
            obs_rs_audio::AudioMonitorMode::MonitorAndOutput,
        )
        .expect("desktop monitor mode should reach the worker");
    output
        .set_monitor_output_id(None)
        .expect("clearing the monitor sink should reach the worker");
    assert_eq!(output.monitor_output_id(), None);
}

#[test]
fn preview_renderer_rebuilds_after_project_edit() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("new-scene", "New scene").expect("scene"),
        })
        .expect("add scene");

    // A different revision is what tells the renderer to apply the change.
    assert!(renderer
        .sync_project(&project, 1)
        .expect("renderer should apply the edited project"));
    // The same revision must not trigger another sync.
    assert!(!renderer
        .sync_project(&project, 1)
        .expect("unchanged revision is a no-op"));
    assert!(renderer
        .render("new-scene")
        .expect("empty scene should be renderable")
        .is_none());
}

#[test]
fn a_transform_commits_to_the_item_it_started_on() {
    let mut project = initial_project().expect("initial GUI project should validate");
    // The preview scene needs a second item for a selection change to have
    // anywhere to go.
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: obs_rs_project::SceneItemSpec::new("overlay", "pattern").expect("item"),
        })
        .expect("add a second item");
    let state = Rc::new(RefCell::new(DesktopState::new(project)));
    let scene = state
        .borrow()
        .preview_scene()
        .expect("a preview scene is selected")
        .to_owned();
    let items = {
        let state = state.borrow();
        let session = state.project_session();
        session
            .project()
            .active_profile_spec()
            .expect("profile")
            .scene(scene.as_str())
            .expect("scene")
            .items()
            .iter()
            .map(|item| item.id().as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert!(items.len() > 1, "the fixture needs two items to confuse");

    let target = crate::source_target(&state.borrow(), &items[0]).expect("target");
    // The gesture started on the first item; the selection has since moved on,
    // which is what a dock click during a drag does.
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: items[1].clone(),
        })
        .expect("select the other item");

    assert_eq!(target.item, items[0]);
    assert_ne!(
        crate::selected_target(&state.borrow())
            .expect("selection")
            .item,
        target.item,
        "the pinned target must not follow the selection"
    );
}

#[test]
fn moving_a_source_does_not_recreate_the_scene_sources() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let before = {
        let mut ids = renderer.runtime.source_ids();
        ids.sort();
        ids
    };

    // A canvas drag is a stream of these. Not one of them may recreate a
    // source: for a camera or a screen cast, recreating is reopening.
    for step in 1..=25_u64 {
        project
            .apply(ProjectCommand::SetSceneItemTransform {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                item: "background".to_owned(),
                transform: FrameTransform::new(
                    500,
                    500,
                    i32::try_from(step).expect("step"),
                    0,
                    false,
                    false,
                    255,
                )
                .expect("transform"),
            })
            .expect("move source");
        assert!(renderer
            .sync_project(&project, step)
            .expect("renderer should apply the move"));
    }

    let after = {
        let mut ids = renderer.runtime.source_ids();
        ids.sort();
        ids
    };
    assert_eq!(before, after, "a move must not rebuild the runtime sources");
}

#[test]
fn repeated_scene_item_references_share_the_runtime_source() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let source_count = renderer.runtime.source_count();
    let transform = FrameTransform::new(500, 500, 120, 40, false, false, 128).expect("transform");

    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: obs_rs_project::SceneItemSpec::new("background-copy", "background")
                .expect("reference item"),
        })
        .expect("add reference item");
    project
        .apply(ProjectCommand::SetSceneItemTransform {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "background-copy".to_owned(),
            transform,
        })
        .expect("transform reference item");
    renderer
        .sync_project(&project, 1)
        .expect("renderer should apply the duplicate reference");

    assert_eq!(renderer.runtime.source_count(), source_count);
    let scene_sources = renderer
        .runtime
        .scene_sources("preview")
        .expect("preview scene is live");
    assert_eq!(scene_sources.len(), 2);
    assert_eq!(scene_sources[0], scene_sources[1]);
    assert_eq!(
        renderer.runtime.scene_item_ids("preview"),
        Some(vec!["background".to_owned(), "background-copy".to_owned()])
    );
    assert_eq!(
        renderer.runtime.scene_item_transform("preview", 1),
        Some(transform)
    );

    let layers = renderer
        .runtime
        .render_scene_layers(
            "preview",
            &VideoRequest::new(Timestamp::ZERO, renderer.format),
        )
        .expect("duplicate scene should render");
    assert_eq!(layers.len(), 2);
    assert_eq!(
        renderer
            .runtime
            .compositor_metrics()
            .capture_latency()
            .samples(),
        1
    );
}

#[test]
fn nested_scene_references_render_without_reopening_shared_sources() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let child_transform =
        FrameTransform::new(1_500, 800, 10, -4, false, false, 200).expect("child transform");
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    let mut child_item =
        obs_rs_project::SceneItemSpec::for_source("background").expect("child item");
    child_item.set_transform(child_transform);
    child.add_item(child_item).expect("child item attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        })
        .expect("add child scene");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("parent", "Parent").expect("parent scene"),
        })
        .expect("add parent scene");
    let parent_transform =
        FrameTransform::new(2_000, 1_500, 20, 30, false, false, 128).expect("parent transform");
    let mut nested =
        obs_rs_project::SceneItemSpec::for_scene("child-item", "child").expect("nested item");
    nested.set_transform(parent_transform);
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "parent".to_owned(),
            item: nested,
        })
        .expect("add nested item");

    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let source_count = renderer.runtime.source_count();
    assert_eq!(
        renderer
            .runtime
            .scene_sources("parent")
            .expect("parent scene is live")
            .len(),
        1
    );
    assert_eq!(renderer.runtime.source_count(), source_count);
    assert_eq!(
        renderer.runtime.scene_item_transform("parent", 0),
        Some(
            child_transform
                .compose_simple(parent_transform)
                .expect("compose")
        )
    );

    let layers = renderer
        .runtime
        .render_scene_layers(
            "parent",
            &VideoRequest::new(Timestamp::ZERO, renderer.format),
        )
        .expect("nested scene should render");
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].item_id(), "child-item/background");
    assert_eq!(
        renderer
            .runtime
            .compositor_metrics()
            .capture_latency()
            .samples(),
        1
    );
}

#[test]
fn group_items_render_without_reopening_shared_sources() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut group = obs_rs_project::SceneItemSpec::for_group("group", "Group").expect("group");
    group
        .group_mut()
        .expect("group target")
        .add_item(obs_rs_project::SceneItemSpec::for_source("background").expect("group child"))
        .expect("group child attach");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: group,
        })
        .expect("add group");

    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let source_count = renderer.runtime.source_count();
    assert_eq!(
        renderer
            .runtime
            .scene_sources("preview")
            .expect("preview scene")
            .len(),
        2
    );
    assert_eq!(renderer.runtime.source_count(), source_count);
    let layers = renderer
        .runtime
        .render_scene_layers(
            "preview",
            &VideoRequest::new(Timestamp::ZERO, renderer.format),
        )
        .expect("group should render");
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0].item_id(), "background");
    assert_eq!(layers[1].item_id(), "group/background");
    assert_eq!(
        renderer
            .runtime
            .compositor_metrics()
            .capture_latency()
            .samples(),
        1
    );
}

#[test]
fn hiding_a_source_keeps_the_others_running() {
    let mut project = initial_project().expect("initial GUI project should validate");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    let before = {
        let mut ids = renderer.runtime.source_ids();
        ids.sort();
        ids
    };

    project
        .apply(ProjectCommand::SetSceneItemVisibility {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "background".to_owned(),
            visible: false,
        })
        .expect("hide source");
    renderer
        .sync_project(&project, 1)
        .expect("renderer should apply the visibility change");

    let after = {
        let mut ids = renderer.runtime.source_ids();
        ids.sort();
        ids
    };
    // Hiding detaches the item from the scene; the source definition and every
    // other device in the project stay exactly as they were.
    assert_eq!(before, after);
    assert!(renderer
        .render("preview")
        .expect("hidden scene should render")
        .is_none());
}

#[test]
fn preview_renderer_honors_hidden_scene_sources() {
    let mut project = initial_project().expect("initial GUI project should validate");
    project
        .apply(ProjectCommand::SetSceneItemVisibility {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "background".to_owned(),
            visible: false,
        })
        .expect("hide source");
    let mut renderer = PreviewRenderer::new(&project, 0).expect("preview renderer should build");
    assert!(renderer
        .render("preview")
        .expect("hidden scene should render")
        .is_none());
}
