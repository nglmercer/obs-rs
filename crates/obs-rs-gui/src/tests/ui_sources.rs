use super::*;

/// Verifies that the focused Sources dock uses the same Rust removal callback
/// as the canvas, including the locked-item failure path.
pub(super) fn exercise_source_keyboard_delete(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let output = Rc::new(RefCell::new(OutputRuntime::new(surface.borrow().format)));
    crate::callbacks::install_callbacks(ui, state, surface, &output);
    for (id, locked) in [("keyboard-delete", false), ("keyboard-delete-locked", true)] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                source: SourceSpec::new(
                    id,
                    "color_source",
                    id,
                    source_settings("color_source").expect("source defaults"),
                )
                .expect("keyboard source"),
            }))
            .expect("add keyboard source");
        if locked {
            state
                .borrow_mut()
                .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemLocked {
                    profile: "live".to_owned(),
                    scene: "preview".to_owned(),
                    item: id.to_owned(),
                    locked: true,
                }))
                .expect("lock keyboard source");
        }
        refresh_ui(ui, state, surface);
        if locked {
            focus_last_source_row(ui);
        } else {
            focus_canvas(ui);
        }
        // The click used to focus the list also selects whichever visible row
        // is under the test point. Restore the intended target after that
        // gesture so the keyboard assertion covers the selected item rather
        // than depending on row ordering in the larger fixture.
        ui.invoke_select_source(id.into());
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Delete.into(),
        });
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Delete.into(),
        });

        let exists = state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("preview"))
            .is_some_and(|scene| scene.item(id).is_some());
        assert_eq!(exists, locked, "Delete must respect source locking");
        if locked {
            assert!(ui.get_status_message().contains("locked"));
        }
    }
}

/// Verifies that Delete applies to the complete bounded canvas selection and
/// that the project session restores that selection in one undo step.
pub(super) fn exercise_multi_source_keyboard_delete(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    for id in ["keyboard-delete-first", "keyboard-delete-second"] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                source: SourceSpec::new(
                    id,
                    "color_source",
                    id,
                    source_settings("color_source").expect("source defaults"),
                )
                .expect("multi-delete source"),
            }))
            .expect("add multi-delete source");
    }
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSources {
            ids: vec![
                "keyboard-delete-first".to_owned(),
                "keyboard-delete-second".to_owned(),
            ],
            additive: false,
        })
        .expect("select both sources");
    refresh_ui(ui, state, surface);
    focus_canvas(ui);
    // The focus click may also hit an existing canvas item; restore the
    // intended multi-selection after focus has been established.
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSources {
            ids: vec![
                "keyboard-delete-first".to_owned(),
                "keyboard-delete-second".to_owned(),
            ],
            additive: false,
        })
        .expect("restore both selected sources");
    refresh_ui(ui, state, surface);

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Delete.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Delete.into(),
    });
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .is_some_and(|scene| {
            scene.item("keyboard-delete-first").is_none()
                && scene.item("keyboard-delete-second").is_none()
        }));

    ui.invoke_undo_edit();
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .is_some_and(|scene| {
            scene.item("keyboard-delete-first").is_some()
                && scene.item("keyboard-delete-second").is_some()
        }));
}

/// Verifies that Sources rows translate mouse modifiers into the same Rust
/// selection modes used by keyboard navigation and canvas selection.
pub(super) fn exercise_source_mouse_selection(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    for id in [
        "mouse-select-first",
        "mouse-select-second",
        "mouse-select-third",
    ] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                source: SourceSpec::new(
                    id,
                    "color_source",
                    id,
                    source_settings("color_source").expect("source defaults"),
                )
                .expect("mouse selection source"),
            }))
            .expect("add mouse selection source");
    }
    refresh_ui(ui, state, surface);
    let rows = ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
        .filter(|row| row.size().height > 30.0)
        .collect::<Vec<_>>();
    assert!(
        rows.len() >= 5,
        "the visible source rows include the selection targets"
    );
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSources {
            ids: Vec::new(),
            additive: false,
        })
        .expect("clear selection before mouse test");
    refresh_ui(ui, state, surface);

    visible_source_row_target(ui, 2).mock_single_click(PointerEventButton::Left);
    refresh_ui(ui, state, surface);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["keyboard-delete-first"],
        "plain source-row click replaces the selection"
    );

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    visible_source_row_target(ui, 4).mock_single_click(PointerEventButton::Left);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    refresh_ui(ui, state, surface);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec![
            "keyboard-delete-first",
            "keyboard-delete-second",
            "mouse-select-first"
        ],
        "Shift-click selects the contiguous source-row range"
    );

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    visible_source_row_target(ui, 3).mock_single_click(PointerEventButton::Left);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    refresh_ui(ui, state, surface);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["keyboard-delete-first", "mouse-select-first"],
        "Ctrl-click toggles an existing source out of the selection"
    );

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    visible_source_row_target(ui, 3).mock_single_click(PointerEventButton::Left);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    refresh_ui(ui, state, surface);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["keyboard-delete-second", "mouse-select-first"],
        "Shift-click resolves the range in either direction from the active row"
    );

    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "keyboard-delete-first".to_owned(),
        })
        .expect("reset keyboard range anchor");
    refresh_ui(ui, state, surface);
    ui.invoke_navigate_source_selection(1, 1);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["keyboard-delete-first", "keyboard-delete-second"],
        "Shift keyboard navigation selects the adjacent source range"
    );
}

fn visible_source_row_target(ui: &MainWindow, index: usize) -> ElementHandle {
    let rows = ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
        .filter(|row| row.size().height > 30.0)
        .collect::<Vec<_>>();
    rows.get(index)
        .expect("visible source row")
        .query_descendants()
        .match_inherits("TouchArea")
        .match_predicate(|area| area.size().width > 150.0 && area.size().height > 30.0)
        .find_first()
        .expect("visible source row target")
}

fn focus_canvas(ui: &MainWindow) {
    let canvas = ElementHandle::find_by_element_id(ui, "CanvasEditor::surface")
        .find(|canvas| canvas.size().width > 100.0 && canvas.size().height > 100.0)
        .expect("editable canvas focus target");
    canvas.mock_single_click(PointerEventButton::Left);
}

fn focus_last_source_row(ui: &MainWindow) {
    let row = ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
        .filter(|row| row.size().height > 30.0)
        .collect::<Vec<_>>()
        .pop()
        .expect("keyboard source row");
    let target = row
        .query_descendants()
        .match_inherits("TouchArea")
        .match_predicate(|area| area.size().width > 150.0 && area.size().height > 30.0)
        .find_first()
        .expect("keyboard source row focus target");
    target.mock_single_click(PointerEventButton::Left);
}

pub(super) fn render_source_properties_window() {
    let window = SourcePropertiesWindow::new().expect("properties window should instantiate");
    window.set_source_name("Background".into());
    window.set_source_kind("color_source".into());
    window.set_source_settings("color = \"#405070FF\"\nheight = 360\nwidth = 640\n".into());
    window.set_property_rows(ModelRc::new(VecModel::from(crate::properties::rows(
        "color_source",
        "color = \"#405070FF\"\nheight = 360\nwidth = 640\n",
        UiLocale::English,
    ))));
    window.show().expect("properties window should show");
    for locale in UiLocale::supported() {
        window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(*locale));
        let snapshot = window
            .window()
            .take_snapshot()
            .expect("properties window should render");
        assert!(snapshot.width() > 0 && snapshot.height() > 0);
    }
    window.hide().expect("properties window should hide");
}

/// Exercises the standalone filter list through its project-command callbacks.
#[allow(
    clippy::too_many_lines,
    reason = "the GUI fixture keeps the complete ordered filter-window workflow together"
)]
pub(super) fn render_source_filters_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "background".to_owned(),
        })
        .expect("background source should be selectable");
    refresh_ui(ui, state, surface);

    let controller = crate::install_source_filters_window(ui, state, surface)
        .expect("filters window should instantiate");
    ui.invoke_open_source_filters_window();
    let window = crate::callbacks::source_filters::source_filters_window(&controller);
    assert_eq!(window.get_source_name(), "Background");
    window.show().expect("filters window should show");
    for locale in UiLocale::supported() {
        window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(*locale));
        let snapshot = window
            .window()
            .take_snapshot()
            .expect("filters window should render");
        assert!(snapshot.width() > 0 && snapshot.height() > 0);
    }

    window.invoke_add_filter("brightness".into());
    window.invoke_add_filter("grayscale".into());
    let selected_id = window.get_selected_filter_id();
    assert_eq!(selected_id, "grayscale");
    window.invoke_rename_filter("Scene grayscale".into());
    window.invoke_move_filter(-1);
    window.invoke_select_filter("brightness".into());
    window.invoke_toggle_filter();
    window.invoke_edit_property("milli".into(), "450".into());

    window.invoke_add_filter("color_correction".into());
    window.invoke_edit_property("gamma".into(), "1000".into());
    window.invoke_edit_property("opacity".into(), "900".into());
    window.invoke_add_filter("color_multiply_add".into());
    window.invoke_edit_property("multiply_red".into(), "220".into());
    window.invoke_edit_property("add_blue".into(), "12".into());
    window.invoke_add_filter("luma_key".into());
    window.invoke_edit_property("luma_min".into(), "250".into());
    window.invoke_add_filter("color_key".into());
    window.invoke_edit_property("similarity".into(), "200".into());
    window.invoke_add_filter("chroma_key".into());
    window.invoke_edit_property("spill".into(), "140".into());
    window.invoke_add_filter("sharpen".into());
    window.invoke_edit_property("sharpness".into(), "120".into());
    window.invoke_add_filter("scroll".into());
    window.invoke_edit_property("speed_x".into(), "120".into());
    window.invoke_edit_property("speed_y".into(), "-80".into());
    window.invoke_edit_property("loop".into(), "false".into());
    window.invoke_add_filter("render_delay".into());
    window.invoke_edit_property("milliseconds".into(), "100".into());
    window.invoke_add_filter("noise_gate".into());
    window.invoke_edit_property("open_threshold_db_milli".into(), "-26000".into());
    window.invoke_edit_property("close_threshold_db_milli".into(), "-32000".into());

    let state_ref = state.borrow();
    let source = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("background"))
        .expect("background source after filter edits");
    assert_eq!(source.filters().len(), 11);
    assert_eq!(source.filters()[0].id().as_str(), "grayscale");
    assert_eq!(source.filters()[0].name(), "Scene grayscale");
    let brightness = source
        .filters()
        .iter()
        .find(|filter| filter.id().as_str() == "brightness")
        .expect("brightness filter");
    assert!(!brightness.enabled());
    assert_eq!(brightness.settings().get("milli"), Some("450"));
    let color_correction = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "color_correction")
        .expect("color correction filter");
    assert_eq!(color_correction.settings().get("gamma"), Some("1000"));
    assert_eq!(color_correction.settings().get("opacity"), Some("900"));
    let color_multiply_add = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "color_multiply_add")
        .expect("color multiply/add filter");
    assert_eq!(
        color_multiply_add.settings().get("multiply_red"),
        Some("220")
    );
    assert_eq!(color_multiply_add.settings().get("add_blue"), Some("12"));
    let luma_key = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "luma_key")
        .expect("luma key filter");
    assert_eq!(luma_key.settings().get("luma_max"), Some("1000"));
    assert_eq!(luma_key.settings().get("luma_min"), Some("250"));
    let color_key = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "color_key")
        .expect("color key filter");
    assert_eq!(color_key.settings().get("key_green"), Some("255"));
    assert_eq!(color_key.settings().get("similarity"), Some("200"));
    let chroma_key = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "chroma_key")
        .expect("chroma key filter");
    assert_eq!(chroma_key.settings().get("key_green"), Some("255"));
    assert_eq!(chroma_key.settings().get("spill"), Some("140"));
    let sharpen = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "sharpen")
        .expect("sharpen filter");
    assert_eq!(sharpen.settings().get("sharpness"), Some("120"));
    let scroll = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "scroll")
        .expect("scroll filter");
    assert_eq!(scroll.settings().get("speed_x"), Some("120"));
    assert_eq!(scroll.settings().get("speed_y"), Some("-80"));
    assert_eq!(scroll.settings().get("loop"), Some("false"));
    let render_delay = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "render_delay")
        .expect("render delay filter");
    assert_eq!(render_delay.settings().get("milliseconds"), Some("100"));
    let noise_gate = source
        .filters()
        .iter()
        .find(|filter| filter.kind().as_str() == "noise_gate")
        .expect("noise gate filter");
    assert_eq!(
        noise_gate.settings().get("open_threshold_db_milli"),
        Some("-26000")
    );
    assert_eq!(
        noise_gate.settings().get("close_threshold_db_milli"),
        Some("-32000")
    );
    drop(state_ref);

    window.invoke_select_filter("color_key".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("chroma_key".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("sharpen".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("scroll".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("render_delay".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("noise_gate".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("luma_key".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("color_correction".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("color_multiply_add".into());
    window.invoke_remove_filter();
    window.invoke_select_filter("grayscale".into());
    window.invoke_remove_filter();
    assert_eq!(window.get_effect_rows().row_count(), 1);
    window.invoke_select_filter("brightness".into());
    window.set_selected_filter_name("Uncommitted name".into());
    window.invoke_close_window();
    ui.invoke_open_source_filters_window();
    assert_ne!(window.get_selected_filter_name(), "Uncommitted name");
    window.invoke_close_window();
}

/// Confirms the transform dialog edits scene-item state and does not add
/// transform fields back to source properties.
pub(super) fn exercise_source_transform_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let controller = crate::install_source_transform_window(ui, state, surface)
        .expect("transform window should instantiate");
    ui.invoke_open_source_transform_window();
    let window = crate::callbacks::source_transform::source_transform_window(&controller);
    assert_eq!(window.get_source_name(), "Background");
    window.show().expect("transform window should show");
    window.set_position_x(42);
    window.set_position_y(-7);
    window.set_item_opacity(200);
    window.set_flip_horizontal(true);
    window.set_rotation_degrees(90);
    window.invoke_accept_transform();

    let state_ref = state.borrow();
    let scene_id = state_ref.preview_scene().expect("preview scene");
    let _source = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("background"))
        .expect("background source after transform edit");
    let item = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .and_then(|scene| scene.item("background"))
        .expect("background item after transform edit");
    assert_eq!(item.transform().translate_x(), 42);
    assert_eq!(item.transform().translate_y(), -7);
    assert_eq!(item.transform().opacity(), 200);
    assert!(item.transform().flip_x());
    assert_eq!(item.transform().rotation_degrees(), 90);
    drop(state_ref);

    ui.invoke_open_source_transform_window();
    window.invoke_reset_transform();
    assert_eq!(window.get_position_x(), 0);
    assert_eq!(window.get_position_y(), 0);
    window.invoke_close_window();

    let state_ref = state.borrow();
    let scene_id = state_ref.preview_scene().expect("preview scene");
    let item = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .and_then(|scene| scene.item("background"))
        .expect("background item after transform cancel");
    assert_eq!(item.transform().translate_x(), 42);
}

/// Drives the Add Source window the way a user would: pick a kind, create a
/// source, then copy an existing one into the current scene.
pub(super) fn exercise_add_source_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let controller = crate::install_add_source_window(ui, state, surface)
        .expect("add source window should instantiate");
    let window = crate::callbacks::add_source_window(&controller);
    window.show().expect("add source window should show");

    // Every registered kind must produce a renderable page.
    for kind in crate::preview::builtin_source_kinds() {
        crate::callbacks::populate_add_source_window(&controller, state, &kind);
        assert!(
            window
                .window()
                .take_snapshot()
                .expect("kind page should render")
                .width()
                > 0
        );
        assert!(window.get_can_create(), "a real kind offers creation");
    }

    let scene = state
        .borrow()
        .preview_scene()
        .expect("a preview scene is selected")
        .to_owned();
    crate::callbacks::populate_add_source_window(&controller, state, "color_source");

    let before = scene_source_count(state, &scene);
    window.invoke_create_source();
    assert_eq!(
        scene_source_count(state, &scene),
        before + 1,
        "create adds exactly one source to the current scene"
    );

    // A source the current scene already shows is never offered: adding it
    // again would only produce a second identical row. The fixture's scenes all
    // hold an identically named background, so a distinct source is planted in
    // another scene to have something that *can* be added.
    let donor = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| {
            profile
                .scenes()
                .map(|value| value.id().as_str().to_owned())
                .find(|value| *value != scene)
        })
        .expect("the project has a second scene");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: donor.clone(),
            source: SourceSpec::new(
                "overlay",
                "color_source",
                "Overlay",
                source_settings("color_source").expect("colour defaults"),
            )
            .expect("overlay source"),
        }))
        .expect("plant a source in another scene");

    crate::callbacks::populate_add_source_window(&controller, state, "color_source");
    let candidate = window
        .get_candidates()
        .iter()
        .find(|row| row.name == "Overlay")
        .expect("the planted source is offered");
    assert_ne!(
        candidate.scene.as_str(),
        scene.as_str(),
        "candidates never come from the target scene"
    );
    window.invoke_toggle_candidate(candidate.id.clone());
    assert_eq!(window.get_selected_count(), 1);
    let before = scene_source_count(state, &scene);
    window.invoke_add_selected();
    assert_eq!(
        scene_source_count(state, &scene),
        before + 1,
        "adding one existing source copies exactly one spec"
    );
    assert_eq!(
        window.get_selected_count(),
        0,
        "the selection is cleared once it has been added"
    );

    // Once it is in the scene, the same source is no longer a candidate.
    crate::callbacks::populate_add_source_window(&controller, state, "color_source");
    assert!(
        !window
            .get_candidates()
            .iter()
            .any(|row| row.id == candidate.id),
        "a source that is already in the scene must not be offered again"
    );

    // "Recently added" lists existing sources only, so it offers no creation.
    crate::callbacks::populate_add_source_window(&controller, state, "@recent");
    assert!(!window.get_can_create());

    window.hide().expect("add source window should hide");
}

/// Verifies the complete screen/camera source-properties path: selecting a
/// camera source, changing its device in the `ComboBox` callback, and accepting
/// the draft writes the selected stable device ID back into the project.
pub(super) fn exercise_capture_device_properties_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let scene = state
        .borrow()
        .preview_scene()
        .expect("preview scene")
        .to_owned();
    let mut settings = source_settings("camera_capture").expect("camera defaults");
    let camera_id = crate::capture_devices("camera_capture")
        .first()
        .map_or_else(|| "nokhwa-camera-0".to_owned(), |(id, _)| id.clone());
    settings
        .set("device_id", &camera_id)
        .expect("Nokhwa camera selection");
    let source = SourceSpec::new("gui-camera", "camera_capture", "GUI camera", settings)
        .expect("camera source");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene: scene.clone(),
            source,
        }))
        .expect("add camera source");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "gui-camera".to_owned(),
        })
        .expect("select camera source");
    refresh_ui(ui, state, surface);

    let controller =
        crate::install_source_properties_window(ui, state, surface).expect("properties controller");
    ui.invoke_open_source_properties_window();
    let window =
        crate::callbacks::source_properties::SourcePropertiesController::window(&controller);
    // The camera kind renders a device drop-down as its first typed row.
    let device_row = window
        .get_property_rows()
        .row_data(0)
        .expect("the camera form has a device row");
    assert_eq!(device_row.key, "device_id");
    assert!(device_row.choices.row_count() >= 1);
    window.invoke_edit_property(device_row.key.clone(), "0".into());
    assert!(window.get_source_settings().contains("device_id = "));
    window.invoke_accept_properties();

    let state = state.borrow();
    let source = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("gui-camera"))
        .expect("camera source persisted");
    assert_eq!(
        source.settings().get("device_id"),
        Some(camera_id.as_str()),
        "ComboBox selection must reach the project command"
    );
}
