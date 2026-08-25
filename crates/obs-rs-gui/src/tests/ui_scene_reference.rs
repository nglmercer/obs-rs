use super::*;

/// The transform dialog uses the same flattened target path as the canvas,
/// but its values are local to the scene that owns the leaf. This fixture also
/// proves a locked Scene source blocks a child edit without mutating either
/// scene.
pub(super) fn exercise_scene_reference_transform_dialog(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let mut child = SceneSpec::new("transform-child", "Transform child").expect("child scene");
    let child_transform =
        FrameTransform::new(1_250, 900, 11, -6, false, false, 210).expect("child transform");
    let mut child_item = SceneItemSpec::for_source("background").expect("child item");
    child_item.set_transform(child_transform);
    child.add_item(child_item).expect("child item attach");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        }))
        .expect("add transform child scene");

    let parent_transform =
        FrameTransform::new(1_400, 1_100, 24, 18, true, false, 255).expect("parent transform");
    let mut reference =
        SceneItemSpec::for_scene("transform-child-ref", "transform-child").expect("reference");
    reference.set_transform(parent_transform);
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: reference,
        }))
        .expect("add transform scene reference");
    refresh_ui(ui, state, surface);

    let controller = crate::install_source_transform_window(ui, state, surface)
        .expect("scene-reference transform window should instantiate");
    ui.invoke_open_source_transform_for("transform-child-ref/background".into());
    let window = crate::callbacks::source_transform::source_transform_window(&controller);
    assert_eq!(window.get_source_name(), "Background");
    assert_eq!(window.get_position_x(), child_transform.translate_x());
    assert_eq!(window.get_position_y(), child_transform.translate_y());
    assert_eq!(
        window.get_item_opacity(),
        i32::from(child_transform.opacity())
    );

    window.set_position_x(37);
    window.set_position_y(-9);
    window.set_item_opacity(190);
    window.invoke_accept_transform();

    let state_ref = state.borrow();
    let child_after = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("transform-child"))
        .and_then(|scene| scene.item("background"))
        .map(SceneItemSpec::transform)
        .expect("child transform after dialog edit");
    let parent_after = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item("transform-child-ref"))
        .map(SceneItemSpec::transform)
        .expect("parent transform after dialog edit");
    assert_eq!(child_after.translate_x(), 37);
    assert_eq!(child_after.translate_y(), -9);
    assert_eq!(child_after.opacity(), 190);
    assert_eq!(parent_after, parent_transform);
    drop(state_ref);

    exercise_scene_reference_source_dialogs(ui, state, surface);

    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemLocked {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: "transform-child-ref".to_owned(),
            locked: true,
        }))
        .expect("lock scene reference");
    ui.invoke_open_source_transform_for("transform-child-ref/background".into());
    window.set_position_x(99);
    window.invoke_accept_transform();
    assert!(ui.get_status_message().contains("locked"));
    assert_eq!(
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("transform-child"))
            .and_then(|scene| scene.item("background"))
            .map(SceneItemSpec::transform)
            .expect("locked child transform")
            .translate_x(),
        37
    );
}

fn exercise_scene_reference_source_dialogs(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let properties = crate::install_source_properties_window(ui, state, surface)
        .expect("scene-reference properties window should instantiate");
    ui.invoke_open_source_properties_for("transform-child-ref/background".into());
    let properties_window =
        crate::callbacks::source_properties::SourcePropertiesController::window(&properties);
    assert_eq!(properties_window.get_source_name(), "Background");
    assert_eq!(properties_window.get_source_kind(), "color_source");
    properties_window.invoke_edit_property("width".into(), "900".into());
    properties_window.invoke_accept_properties();
    assert_eq!(
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.source("background"))
            .and_then(|source| source.settings().get("width")),
        Some("900"),
        "nested source properties must edit the shared source definition"
    );

    let filters = crate::install_source_filters_window(ui, state, surface)
        .expect("scene-reference filters window should instantiate");
    ui.invoke_open_source_filters_for("transform-child-ref/background".into());
    let filters_window = crate::callbacks::source_filters::source_filters_window(&filters);
    assert_eq!(filters_window.get_source_name(), "Background");
    filters_window.invoke_add_filter("compressor".into());
    assert!(
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.source("background"))
            .is_some_and(|source| {
                source
                    .filters()
                    .iter()
                    .any(|filter| filter.kind().as_str() == "compressor")
            }),
        "nested source filters must edit the shared source definition"
    );
    filters_window.invoke_close_window();

    ui.invoke_open_source_rename("transform-child-ref/background".into());
    assert_eq!(ui.get_source_name_draft(), "Background");
    ui.set_source_name_draft("Nested background".into());
    ui.invoke_apply_source_name();
    assert_eq!(
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.source("background"))
            .map(obs_rs_project::SourceSpec::name),
        Some("Nested background"),
        "nested Scene-reference rename must update the shared source definition"
    );
    ui.set_active_modal(0);

    ui.invoke_toggle_source_visibility("transform-child-ref/background".into());
    assert!(!state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("transform-child"))
        .and_then(|scene| scene.item("background"))
        .expect("nested visibility target")
        .visible());
    ui.invoke_toggle_source_visibility("transform-child-ref/background".into());
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("transform-child"))
        .and_then(|scene| scene.item("background"))
        .expect("nested visibility target restored")
        .visible());

    ui.invoke_toggle_source_locked("transform-child-ref/background".into());
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("transform-child"))
        .and_then(|scene| scene.item("background"))
        .expect("nested lock target")
        .locked());
    ui.invoke_toggle_source_locked("transform-child-ref/background".into());
    assert!(!state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("transform-child"))
        .and_then(|scene| scene.item("background"))
        .expect("nested lock target restored")
        .locked());
}
