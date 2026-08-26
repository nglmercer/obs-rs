use super::*;

/// Verifies that the focused Scenes dock maps OBS's two scene-removal keys to
/// the existing Rust callback and keeps the current preview selection valid.
pub(super) fn exercise_scene_dock_delete_keyboard(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    for (id, key) in [
        ("dock-keyboard-delete-scene", Key::Delete),
        ("dock-keyboard-backspace-scene", Key::Backspace),
    ] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddScene {
                profile: "live".to_owned(),
                scene: SceneSpec::new(id, id).expect("dock keyboard scene"),
            }))
            .expect("add dock keyboard scene");
        refresh_ui(ui, state, surface);

        ui.invoke_select_preview(id.into());
        refresh_ui(ui, state, surface);
        focus_scene_row(ui, id);

        dispatch_shifted(ui, key);
        assert!(
            scene_exists(state, id),
            "modified scene-removal keys must not remove the selected scene"
        );
        dispatch_plain(ui, key);

        assert!(
            !scene_exists(state, id),
            "Scenes dock removal key must remove the selected scene"
        );
        assert_ne!(
            state.borrow().preview_scene(),
            Some(id),
            "removing the preview scene must restore a valid fallback"
        );
    }

    exercise_single_scene_guard(ui, state, surface);
}

fn focus_scene_row(ui: &MainWindow, id: &str) {
    let row = ElementHandle::find_by_accessible_label(ui, id)
        .find(|row| row.size().height > 30.0)
        .expect("visible scene row for keyboard removal");
    row.query_descendants()
        .match_inherits("TouchArea")
        .find_first()
        .expect("scene row keyboard removal focus target")
        .mock_single_click(PointerEventButton::Left);
}

fn scene_exists(state: &Rc<RefCell<DesktopState>>, id: &str) -> bool {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .is_some_and(|profile| profile.scene(id).is_some())
}

fn exercise_single_scene_guard(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let restored = ["intermission", "program"]
        .into_iter()
        .map(|id| {
            state
                .borrow()
                .project_session()
                .project()
                .active_profile_spec()
                .and_then(|profile| profile.scene(id))
                .cloned()
                .expect("starter scene to restore after one-scene guard")
        })
        .collect::<Vec<_>>();
    for id in ["intermission", "program"] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::RemoveScene {
                profile: "live".to_owned(),
                scene: id.to_owned(),
            }))
            .expect("remove scene for one-scene guard");
    }
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectPreviewScene {
            id: "preview".to_owned(),
        })
        .expect("select only remaining scene");
    refresh_ui(ui, state, surface);
    assert_eq!(
        scene_count(state),
        1,
        "guard fixture must contain one scene"
    );
    focus_scene_row(ui, "preview");

    for key in [Key::Delete, Key::Backspace] {
        dispatch_plain(ui, key);
        assert!(
            scene_exists(state, "preview"),
            "a one-scene project must consume removal without mutation"
        );
    }

    for (scene, target_index) in restored.into_iter().zip([0, 2]) {
        let id = scene.id().to_string();
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddScene {
                profile: "live".to_owned(),
                scene,
            }))
            .expect("restore starter scene after one-scene guard");
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::MoveScene {
                profile: "live".to_owned(),
                scene: id,
                target_index,
            }))
            .expect("restore starter scene order after one-scene guard");
    }
    refresh_ui(ui, state, surface);
}

fn scene_count(state: &Rc<RefCell<DesktopState>>) -> usize {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .map_or(0, |profile| profile.scenes().count())
}

fn dispatch_shifted(ui: &MainWindow, key: Key) {
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    dispatch_plain(ui, key);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
}

fn dispatch_plain(ui: &MainWindow, key: Key) {
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
}
