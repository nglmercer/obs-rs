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
