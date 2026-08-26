use super::ui_layout::read_order;
use super::ui_scene_reference;
use super::*;

#[allow(
    clippy::too_many_lines,
    reason = "one integration fixture exercises the complete nested source workflow"
)]
pub(super) fn exercise_group_source_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let output = Rc::new(RefCell::new(OutputRuntime::new(surface.borrow().format)));
    crate::callbacks::install_callbacks(ui, state, surface, &output);
    ui.invoke_open_save_project_as();
    assert_eq!(
        ui.get_project_dialog_mode(),
        3,
        "Save As uses its own dialog mode"
    );
    assert_eq!(ui.get_active_modal(), 1, "Save As opens the project dialog");
    ui.set_project_dialog_mode(0);
    ui.set_active_modal(0);
    exercise_scene_keyboard_navigation(ui, state);

    // A failed Save must leave the pending action armed so the user can fix
    // the path and try again instead of silently discarding the project.
    let missing_parent = std::env::temp_dir().join(format!(
        "obs-rs-save-discard-missing-{}.directory",
        std::process::id()
    ));
    let failed_path = missing_parent.join("project.obsrproj");
    ui.set_project_path(failed_path.to_string_lossy().into_owned().into());
    ui.set_pending_discard(4);
    ui.invoke_save_discard(4);
    assert_eq!(ui.get_pending_discard(), 4);
    assert!(ui.get_status_message().contains("Save failed"));
    ui.set_pending_discard(0);

    let saved_path = std::env::temp_dir().join(format!(
        "obs-rs-save-discard-success-{}.obsrproj",
        std::process::id()
    ));
    ui.set_project_path(saved_path.to_string_lossy().into_owned().into());
    ui.set_pending_discard(8);
    ui.invoke_save_discard(8);
    assert_eq!(ui.get_pending_discard(), 0);
    super::ui_project_open::close_project_open_dialog(ui);
    assert!(!state.borrow().is_dirty());
    assert!(saved_path.is_file());
    std::fs::remove_file(&saved_path).expect("remove save/discard fixture");

    ui.set_project_path("obs-rs-project.json".into());

    let mut group =
        obs_rs_project::SceneItemSpec::for_group("overlay-group", "Overlay group").expect("group");
    group
        .group_mut()
        .expect("group target")
        .add_item(obs_rs_project::SceneItemSpec::for_source("background").expect("first child"))
        .expect("first child attach");
    group
        .group_mut()
        .expect("group target")
        .add_item(obs_rs_project::SceneItemSpec::for_source("pattern").expect("second child"))
        .expect("second child attach");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: group,
        }))
        .expect("add group to preview");
    refresh_ui(ui, state, surface);
    assert!(ui
        .get_source_rows()
        .iter()
        .any(|row| row.target == "overlay-group/background"));
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: obs_rs_project::SceneItemSpec::for_group(
                "move-target-group",
                "Move target group",
            )
            .expect("move target group"),
        }))
        .expect("add move target group");
    refresh_ui(ui, state, surface);
    let nested_move_targets = ui
        .get_source_rows()
        .iter()
        .find(|row| row.target == "overlay-group/background")
        .expect("nested source row");
    assert_eq!(
        nested_move_targets
            .move_targets
            .iter()
            .map(|target| (
                target.id.to_string(),
                target.name.to_string(),
                target.enabled
            ))
            .collect::<Vec<_>>(),
        vec![
            (String::new(), "Scene root".to_owned(), true),
            (
                "overlay-group".to_owned(),
                "  Overlay group".to_owned(),
                false
            ),
            (
                "move-target-group".to_owned(),
                "  Move target group".to_owned(),
                true
            ),
        ]
    );
    ui.invoke_move_source_to_group(
        "overlay-group/background".into(),
        "move-target-group".into(),
    );
    assert_eq!(
        state.borrow().selected_source(),
        Some("move-target-group/background"),
        "reparenting keeps selection on the new stable path"
    );
    ui.invoke_move_source_to_group(
        "move-target-group/background".into(),
        "overlay-group".into(),
    );
    assert_eq!(
        state.borrow().selected_source(),
        Some("overlay-group/background"),
        "moving back restores the nested target"
    );
    ui.invoke_remove_source("move-target-group".into());
    refresh_ui(ui, state, surface);
    ui.invoke_open_source_rename("overlay-group".into());
    assert_eq!(
        ui.get_source_name_draft(),
        "Overlay group",
        "group rename resolves the group item name instead of a source ID"
    );
    // Changing the transient selection while the modal is open must not
    // redirect the rename to the newly selected source.
    ui.invoke_select_source("background".into());
    ui.set_source_name_draft("Overlays".into());
    ui.invoke_apply_source_name();
    assert_eq!(
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("preview"))
            .and_then(|scene| scene.item("overlay-group"))
            .and_then(obs_rs_project::SceneItemSpec::group)
            .map(obs_rs_project::GroupSpec::name),
        Some("Overlays")
    );
    ui.set_active_modal(0);
    ui.invoke_select_source("overlay-group/background".into());
    assert_eq!(
        state.borrow().selected_source(),
        Some("overlay-group/background"),
        "click selection accepts the nested row target"
    );
    assert!(ui
        .get_source_rows()
        .iter()
        .find(|row| row.target == "overlay-group/background")
        .is_some_and(|row| row.selected));
    ui.invoke_open_source_rename("overlay-group/background".into());
    assert_eq!(
        ui.get_source_name_draft(),
        "Background",
        "nested source rename resolves the path-addressed source definition"
    );
    ui.set_active_modal(0);
    ui.invoke_select_source("background".into());

    ui.invoke_navigate_source_selection(1, 0);
    assert_eq!(
        state.borrow().selected_source(),
        Some("overlay-group"),
        "Down selects the next visible top-level source"
    );
    ui.invoke_navigate_source_selection(-1, 1);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["background", "overlay-group"],
        "Shift navigation selects the contiguous range without duplicating state"
    );
    ui.invoke_navigate_source_selection(2, 2);
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["background", "overlay-group", "overlay-group/pattern"],
        "Ctrl navigation preserves the ordered range and appends the toggled nested row"
    );

    // Keep the canvas selection on the root item while opening a nested
    // editor directly; the editor target is intentionally independent from
    // the active canvas geometry until nested geometry projection is added.
    ui.invoke_select_source("background".into());
    let transform = crate::install_source_transform_window(ui, state, surface)
        .expect("nested transform window should instantiate");
    ui.invoke_open_source_transform_for("overlay-group/background".into());
    let transform_window = crate::callbacks::source_transform::source_transform_window(&transform);
    assert_eq!(transform_window.get_source_name(), "Background");
    assert_eq!(
        state.borrow().selected_source(),
        Some("background"),
        "opening a nested transform must not replace canvas selection"
    );
    transform_window.set_position_x(37);
    transform_window.set_position_y(-9);
    transform_window.set_item_opacity(190);
    transform_window.invoke_accept_transform();
    let nested_transform = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item("overlay-group"))
        .and_then(obs_rs_project::SceneItemSpec::group)
        .and_then(|group| {
            group
                .items()
                .iter()
                .find(|item| item.id().as_str() == "background")
        })
        .map(obs_rs_project::SceneItemSpec::transform)
        .expect("nested transform should be committed to the child");
    assert_eq!(nested_transform.translate_x(), 37);
    assert_eq!(nested_transform.translate_y(), -9);
    assert_eq!(nested_transform.opacity(), 190);

    let properties = crate::install_source_properties_window(ui, state, surface)
        .expect("nested properties window should instantiate");
    ui.invoke_open_source_properties_for("overlay-group/background".into());
    let properties_window =
        crate::callbacks::source_properties::SourcePropertiesController::window(&properties);
    assert_eq!(properties_window.get_source_name(), "Background");
    assert_eq!(properties_window.get_source_kind(), "color_source");
    assert_eq!(
        state.borrow().selected_source(),
        Some("background"),
        "opening nested properties must not replace canvas selection"
    );
    properties_window.invoke_edit_property("width".into(), "800".into());
    properties_window.invoke_accept_properties();
    assert_eq!(
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.source("background"))
            .and_then(|source| source.settings().get("width")),
        Some("800")
    );

    let filters = crate::install_source_filters_window(ui, state, surface)
        .expect("nested filters window should instantiate");
    ui.invoke_open_source_filters_for("overlay-group/background".into());
    let filters_window = crate::callbacks::source_filters::source_filters_window(&filters);
    assert_eq!(filters_window.get_source_name(), "Background");
    assert_eq!(
        state.borrow().selected_source(),
        Some("background"),
        "opening a nested filter target must not replace canvas selection"
    );
    filters_window.invoke_add_filter("opacity".into());
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("background"))
        .is_some_and(|source| {
            source
                .filters()
                .iter()
                .any(|filter| filter.kind().as_str() == "opacity")
        }));
    filters_window.invoke_close_window();

    ui.invoke_toggle_source_visibility("overlay-group/background".into());
    ui.invoke_move_source_to("overlay-group/background".into(), 1);
    ui.invoke_flip_source("overlay-group/background".into(), true);
    ui.invoke_duplicate_source("overlay-group/background".into());
    ui.invoke_toggle_source_locked("overlay-group/background".into());
    ui.invoke_remove_source("overlay-group/pattern".into());

    {
        let state = state.borrow();
        let group = state
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("preview"))
            .and_then(|scene| scene.item("overlay-group"))
            .and_then(obs_rs_project::SceneItemSpec::group)
            .expect("group after UI callbacks");
        assert_eq!(
            group.items().len(),
            2,
            "the nested remove callback removes one child after duplication"
        );
        assert_eq!(
            group.items()[0].source_id().as_str(),
            "background",
            "the group move callback must use the group-local order"
        );
        assert!(!group.items()[0].visible());
        assert!(group.items()[0].locked());
        assert!(group.items()[0].transform().flip_x());
        assert_eq!(group.items()[1].source_id().as_str(), "background_copy");
    }

    // Root-level duplication follows OBS's selection behavior even when
    // existing copies mean the new ID is not a predictable "_copy" suffix.
    ui.invoke_select_source("background".into());
    ui.invoke_duplicate_source("background".into());
    let selected = state
        .borrow()
        .selected_source()
        .map(str::to_owned)
        .expect("duplicating a root source selects the new item");
    assert_ne!(selected, "background");
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .is_some_and(|scene| scene.item(selected.as_str()).is_some()));

    ui.invoke_select_all_sources();
    let selected_sources = state
        .borrow()
        .selected_sources()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert!(
        selected_sources.iter().any(|target| target == "background"),
        "Ctrl+A keeps the root source in the bounded visible-row selection"
    );
    assert!(
        selected_sources
            .iter()
            .any(|target| target == "overlay-group"),
        "Ctrl+A keeps the group row in the bounded visible-row selection"
    );
    assert!(
        selected_sources
            .iter()
            .any(|target| target.starts_with("overlay-group/")),
        "Ctrl+A includes visible nested group rows through the same Rust target path"
    );
    assert!(
        selected_sources
            .iter()
            .any(|target| target == selected.as_str()),
        "Ctrl+A includes the later root row after the nested group"
    );

    // Grouping is enabled only for an unlocked same-parent selection. Add a
    // second root item so this exercises the complete callback and the same
    // atomic project command used by the context menu.
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: obs_rs_project::SceneItemSpec::for_source("pattern").expect("pattern root item"),
        }))
        .expect("add root item for grouping");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSources {
            ids: vec!["background".to_owned(), "pattern".to_owned()],
            additive: false,
        })
        .expect("select root items for grouping");
    refresh_ui(ui, state, surface);
    assert!(ui.get_can_group_sources());
    ui.invoke_group_sources();
    assert_eq!(
        state.borrow().selected_source(),
        Some("group"),
        "grouping selects the new group"
    );
    let grouped_ids = {
        let state = state.borrow();
        let grouped = state
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("preview"))
            .and_then(|scene| scene.item("group"))
            .and_then(obs_rs_project::SceneItemSpec::group)
            .expect("group callback creates a root group");
        grouped
            .items()
            .iter()
            .map(|item| item.id().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        grouped_ids,
        vec!["background".to_owned(), "pattern".to_owned()]
    );
    assert!(!ui.get_can_group_sources());
    ui.invoke_undo_edit();
    ui.invoke_undo_edit();
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .is_some_and(|scene| scene.item("group").is_none() && scene.item("pattern").is_none()));

    let mut ungroup_target =
        obs_rs_project::SceneItemSpec::for_group("ungroup-target", "Ungroup target")
            .expect("ungroup target");
    ungroup_target
        .group_mut()
        .expect("ungroup target group")
        .add_item(
            obs_rs_project::SceneItemSpec::new("ungroup-child-a", "background").expect("child a"),
        )
        .expect("add child a");
    ungroup_target
        .group_mut()
        .expect("ungroup target group")
        .add_item(
            obs_rs_project::SceneItemSpec::new("ungroup-child-b", "background").expect("child b"),
        )
        .expect("add child b");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "preview".to_owned(),
            item: ungroup_target,
        }))
        .expect("add ungroup target");
    refresh_ui(ui, state, surface);
    ui.invoke_select_source("ungroup-target".into());
    ui.invoke_ungroup_source("ungroup-target".into());
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec!["ungroup-child-a", "ungroup-child-b"],
        "ungroup selects the exposed root children"
    );
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .is_some_and(|scene| {
            scene.item("ungroup-target").is_none()
                && scene.item("ungroup-child-a").is_some()
                && scene.item("ungroup-child-b").is_some()
        }));
    ui.invoke_undo_edit();
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .is_some_and(|scene| scene.item("ungroup-target").is_some()));

    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSources {
            ids: vec![
                "ungroup-target/ungroup-child-a".to_owned(),
                "ungroup-target/ungroup-child-b".to_owned(),
            ],
            additive: false,
        })
        .expect("select nested siblings for grouping");
    refresh_ui(ui, state, surface);
    assert!(ui.get_can_group_sources());
    ui.invoke_group_sources();
    assert_eq!(
        state.borrow().selected_source(),
        Some("ungroup-target/group"),
        "nested grouping selects the new path-addressed group"
    );
    let nested_grouped_ids = {
        let state = state.borrow();
        let nested_grouped_ids = state
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("preview"))
            .and_then(|scene| scene.item("ungroup-target"))
            .and_then(obs_rs_project::SceneItemSpec::group)
            .and_then(|group| group.items().first())
            .and_then(obs_rs_project::SceneItemSpec::group)
            .map(|group| {
                group
                    .items()
                    .iter()
                    .map(|item| item.id().to_string())
                    .collect::<Vec<_>>()
            })
            .expect("nested grouping creates a child group");
        nested_grouped_ids
    };
    assert_eq!(
        nested_grouped_ids,
        vec!["ungroup-child-a".to_owned(), "ungroup-child-b".to_owned()]
    );
    ui.invoke_ungroup_source("ungroup-target/group".into());
    assert_eq!(
        state.borrow().selected_sources().collect::<Vec<_>>(),
        vec![
            "ungroup-target/ungroup-child-a",
            "ungroup-target/ungroup-child-b"
        ],
        "nested ungroup selects the exposed child paths"
    );
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item("ungroup-target"))
        .and_then(obs_rs_project::SceneItemSpec::group)
        .is_some_and(|group| {
            group.items().iter().all(|item| item.group().is_none()) && group.items().len() == 2
        }));
    ui.invoke_undo_edit();
    assert!(state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item("ungroup-target"))
        .and_then(obs_rs_project::SceneItemSpec::group)
        .is_some_and(|group| group.items().len() == 1 && group.items()[0].is_group()));

    ui_scene_reference::exercise_scene_reference_transform_dialog(ui, state, surface);
}

/// Opens the File menu through its actual pointer target and proves its popup
/// participates in hit testing outside the navbar's 26px bounds.
pub(super) fn exercise_navbar_popup(ui: &MainWindow) {
    let file_button = ElementHandle::find_by_element_id(ui, "AppNavbar::file-button")
        .next()
        .expect("File menu button is discoverable");
    file_button.mock_single_click(PointerEventButton::Left);

    let entries = ElementHandle::find_by_element_type_name(ui, "MenuEntry").collect::<Vec<_>>();
    assert_eq!(entries.len(), 8, "the complete File popup is visible");
    entries[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(
        ElementHandle::find_by_element_type_name(ui, "MenuEntry").count(),
        0,
        "selecting an entry closes the popup"
    );
}

/// Drives the menu-bar actions through the real callbacks.
///
/// The bar's previous failure mode was an entry that dispatched a string
/// nothing handled, so this asserts each action changes observable state rather
/// than only that it can be invoked.
#[allow(
    clippy::too_many_lines,
    reason = "one integration fixture exercises the complete menu and projector workflow"
)]
pub(super) fn exercise_menu_actions(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    docks: &Rc<crate::callbacks::docks::DockController>,
) {
    let projectors = crate::install_menu_callbacks(ui, state, surface, docks);

    // The exercises before this one have already edited the project, so the
    // history starts from a known-empty state rather than from their leftovers.
    ui.invoke_new_project();
    assert!(!ui.get_can_undo(), "a fresh document has nothing to undo");
    let profile = state
        .borrow()
        .project_session()
        .project()
        .active_profile()
        .to_string();
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneName {
            profile,
            scene: "preview".to_owned(),
            name: "Renamed in test".to_owned(),
        }))
        .expect("rename the preview scene");
    refresh_ui(ui, state, surface);
    assert!(ui.get_can_undo(), "an edit becomes an undoable step");

    ui.invoke_undo_edit();
    assert!(
        !ui.get_can_undo() && ui.get_can_redo(),
        "undo consumes the step and offers it back as a redo"
    );
    ui.invoke_redo_edit();
    assert!(ui.get_can_undo());

    // Starting another new project clears the history along with the document.
    ui.invoke_new_project();
    assert!(
        !ui.get_can_undo() && !ui.get_can_redo(),
        "undo must not reach across a new document"
    );

    // A clean restart reopens fixed-target projectors whose bounded lifecycle
    // bit was captured at shutdown. Closing it again clears that bit before
    // the ordinary toggle assertions below.
    let source_target = crate::selected_target(&state.borrow()).expect("selected source target");
    projectors.restore_geometry(&[
        ProjectorGeometry::new(ProjectorKind::Program, 24, 32, 960, 540, 1_000)
            .expect("valid projector geometry")
            .with_fullscreen(true)
            .with_open(true),
        ProjectorGeometry::new(ProjectorKind::Source, 48, 64, 960, 540, 1_000)
            .expect("valid source projector geometry")
            .with_open(true),
        ProjectorGeometry::new(ProjectorKind::Scene, 72, 96, 960, 540, 1_000)
            .expect("valid scene projector geometry")
            .with_open(true),
    ]);
    projectors.restore_targets(&[
        ProjectorTarget::Source {
            scene: source_target.scene.clone(),
            item: source_target.item.clone(),
        },
        ProjectorTarget::Scene {
            scene: "preview".to_owned(),
        },
    ]);
    projectors.reopen_persisted(ui, state);
    assert!(
        projectors.is_open(true),
        "the persisted program projector reopened"
    );
    assert!(
        projectors.is_source_open() && projectors.is_scene_open(),
        "persisted source and scene projectors reopened"
    );
    ui.invoke_open_projector(true);
    ui.invoke_open_source_projector();
    ui.invoke_open_scene_projector("preview".into());
    assert!(
        !projectors.is_open(true),
        "closing clears the persisted open state"
    );

    // A projector is a toggle, not a way to stack duplicate windows.
    assert!(!projectors.is_open(true));
    ui.invoke_open_projector(true);
    assert!(projectors.is_open(true), "the program projector opened");
    assert!(
        projectors.is_fullscreen(true),
        "the program projector uses fullscreen geometry"
    );
    assert!(!projectors.is_open(false), "only one feed was requested");
    ui.invoke_open_projector(true);
    assert!(!projectors.is_open(true), "selecting it again closed it");

    assert!(!projectors.is_multiview_open());
    ui.invoke_open_multiview_projector();
    assert!(
        projectors.is_multiview_open(),
        "the multiview projector opened"
    );
    assert!(
        projectors.is_multiview_fullscreen(),
        "the multiview projector uses fullscreen geometry"
    );
    ui.invoke_open_multiview_projector();
    assert!(
        !projectors.is_multiview_open(),
        "selecting multiview again closed it"
    );

    // A source projector captures the selected scene item, not the current
    // selection after the window has opened.
    refresh_ui(ui, state, surface);
    assert!(!projectors.is_source_open());
    ui.invoke_open_source_projector();
    assert!(
        projectors.is_source_open(),
        "the selected source projector opened"
    );
    ui.invoke_open_source_projector();
    assert!(!projectors.is_source_open(), "selecting it again closed it");

    // A scene projector keeps the scene row's stable ID, independent of the
    // currently selected preview scene.
    assert!(!projectors.is_scene_open());
    ui.invoke_open_scene_projector("preview".into());
    assert!(projectors.is_scene_open(), "the scene projector opened");
    ui.invoke_open_scene_projector("preview".into());
    assert!(!projectors.is_scene_open(), "selecting it again closed it");

    // Resetting the layout restores the shipped arrangement whatever the row
    // was dragged into.
    let reversed = vec![4, 3, 2, 1, 0, 5];
    ui.set_panel_order(ModelRc::new(VecModel::from(reversed.clone())));
    ui.set_show_mixer(false);
    ui.invoke_reset_dock_layout();
    assert_ne!(read_order(ui), reversed, "the reset changed the row");
    assert_eq!(read_order(ui), AppSettings::default().layout.panel_order);
    assert!(
        ui.get_show_mixer(),
        "a hidden dock comes back with the reset"
    );

    // The menu models the About and Scene Collection entries read are populated.
    assert!(!ui.get_app_version().is_empty());
    assert!(!ui.get_app_platform().is_empty());
    assert!(
        ui.get_collection_rows().row_count() >= 1,
        "the open document is always listed as a collection"
    );
}

/// Drives the Scenes dock through its real focus and keyboard boundary. The
/// callback remains the same one used by the floating dock, so this verifies
/// that the list does not acquire a second scene-order owner in Slint.
fn exercise_scene_keyboard_navigation(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    ui.invoke_select_preview("preview".into());
    let row = ElementHandle::find_by_accessible_label(ui, "preview")
        .find(|row| row.size().height > 30.0)
        .expect("preview scene row");
    let target = row
        .query_descendants()
        .match_inherits("TouchArea")
        .find_first()
        .expect("preview scene row focus target");
    target.mock_single_click(PointerEventButton::Left);

    for (key, expected) in [
        (Key::DownArrow, "program"),
        (Key::End, "program"),
        (Key::UpArrow, "preview"),
        (Key::Home, "intermission"),
    ] {
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
        assert_eq!(state.borrow().preview_scene(), Some(expected));
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the integration fixture exercises one complete context-menu workflow"
)]
pub(super) fn exercise_context_menus(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let output = Rc::new(RefCell::new(OutputRuntime::new(surface.borrow().format)));
    crate::callbacks::install_callbacks(ui, state, surface, &output);
    ui.invoke_select_preview("preview".into());
    ui.invoke_navigate_preview_scene(1);
    assert_eq!(state.borrow().preview_scene(), Some("program"));
    ui.invoke_navigate_preview_scene(-1);
    assert_eq!(state.borrow().preview_scene(), Some("preview"));
    ui.invoke_new_project();
    ui.invoke_add_scene("intro".into(), "Intro".into());
    assert_eq!(state.borrow().preview_scene(), Some("intro"));
    ui.invoke_duplicate_scene("intro".into());
    assert_eq!(state.borrow().preview_scene(), Some("intro_copy"));
    ui.invoke_select_preview("preview".into());
    let profile = "live".to_owned();
    for id in ["middle", "foreground"] {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile: profile.clone(),
                scene: "preview".to_owned(),
                source: SourceSpec::new(
                    id,
                    "color_source",
                    id,
                    source_settings("color_source").expect("source defaults"),
                )
                .expect("source"),
            }))
            .expect("add source");
    }
    refresh_ui(ui, state, surface);

    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
            profile: profile.clone(),
            scene: "preview".to_owned(),
            item: "foreground".to_owned(),
            transform: FrameTransform::new(500, 250, 100, 50, false, false, 255)
                .expect("source transform"),
        }))
        .expect("position source for transform command");
    refresh_ui(ui, state, surface);
    ui.invoke_transform_source("foreground".into(), "center-screen".into());
    let centered = state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene("preview"))
        .and_then(|scene| scene.item("foreground"))
        .expect("centered source")
        .transform();
    assert_eq!((centered.translate_x(), centered.translate_y()), (320, 270));

    let rows = ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
        .filter(|row| row.size().height > 30.0)
        .collect::<Vec<_>>();
    println!(
        "source rows model={} handles={:?}",
        ui.get_source_rows().row_count(),
        ElementHandle::find_by_element_type_name(ui, "SourceContextMenuArea")
            .map(|row| (row.size(), row.absolute_position(), row.id()))
            .collect::<Vec<_>>()
    );
    assert_eq!(rows.len(), 3);
    let row_target = rows[1]
        .query_descendants()
        .match_inherits("TouchArea")
        .match_predicate(|target| target.size().width > 120.0 && target.size().height > 30.0)
        .find_first()
        .expect("source row hit target");
    println!(
        "right click target={:?} id={:?} selected-before={:?}",
        row_target.size(),
        row_target.id(),
        state.borrow().selected_source()
    );
    let position = row_target.absolute_position();
    let size = row_target.size();
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: LogicalPosition::new(
            position.x + size.width / 2.0,
            position.y + size.height / 2.0,
        ),
        button: PointerEventButton::Right,
    });
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    for _ in 0..6 {
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::UpArrow.into(),
        });
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::UpArrow.into(),
        });
    }
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    println!(
        "after keyboard duplicate sources={:?}",
        state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene("preview"))
            .map(|scene| scene
                .sources()
                .iter()
                .map(|source| source.id().to_string())
                .collect::<Vec<_>>())
    );
    println!("selected-after={:?}", state.borrow().selected_source());
    let entries = ElementHandle::find_by_element_type_name(ui, "MenuEntry").collect::<Vec<_>>();
    println!(
        "source menu entries: {:?}",
        entries
            .iter()
            .map(|entry| {
                (
                    entry.type_name().map(|value| value.to_string()),
                    entry.id().map(|value| value.to_string()),
                    entry.size(),
                    entry.absolute_position(),
                    entry.computed_opacity(),
                    entry.accessible_label().map(|value| value.to_string()),
                    entry.accessible_enabled(),
                    entry.accessible_checked(),
                )
            })
            .collect::<Vec<_>>()
    );
    for type_name in [
        "MenuEntry",
        "MenuItem",
        "MenuItemBase",
        "MenuFrame",
        "ContextMenuInternal",
    ] {
        println!(
            "{} count={}",
            type_name,
            ElementHandle::find_by_element_type_name(ui, type_name).count()
        );
    }
    for type_name in [
        "PopupMenuImpl",
        "FocusScope",
        "MenuFrameBase",
        "Text",
        "TouchArea",
        "Window",
    ] {
        println!(
            "{} count={}",
            type_name,
            ElementHandle::find_by_element_type_name(ui, type_name).count()
        );
    }
    println!(
        "context ids={:?} context types={:?}",
        ElementHandle::find_by_element_id(ui, "SourceContextMenuArea::context-menu")
            .map(|element| (element.type_name(), element.id(), element.size()))
            .collect::<Vec<_>>(),
        ElementHandle::find_by_element_type_name(ui, "ContextMenuArea")
            .map(|element| (element.type_name(), element.id(), element.size()))
            .collect::<Vec<_>>()
    );
    println!(
        "compact buttons={:?}",
        ElementHandle::find_by_element_type_name(ui, "CompactButton")
            .map(|button| (
                button.size(),
                button.absolute_position(),
                button.accessible_label()
            ))
            .collect::<Vec<_>>()
    );
    let more = ElementHandle::find_by_element_type_name(ui, "CompactButton")
        .find(|button| {
            let position = button.absolute_position();
            position.x < 180.0 && position.y > 800.0
        })
        .expect("source more button");
    more.mock_single_click(PointerEventButton::Left);
    println!(
        "after more menu entries={:?}",
        ElementHandle::find_by_element_type_name(ui, "MenuEntry")
            .map(|entry| (
                entry.type_name(),
                entry.id(),
                entry.size(),
                entry.absolute_position()
            ))
            .collect::<Vec<_>>()
    );
    let context = ElementHandle::find_by_element_type_name(ui, "ContextMenuArea")
        .find(|element| {
            let position = element.absolute_position();
            element.size().height > 30.0 && position.y > 680.0 && position.y < 720.0
        })
        .expect("source context area");
    context.mock_single_click(PointerEventButton::Right);
    println!(
        "after context right menu entries={:?}",
        ElementHandle::find_by_element_type_name(ui, "MenuEntry")
            .map(|entry| (
                entry.type_name(),
                entry.id(),
                entry.size(),
                entry.absolute_position()
            ))
            .collect::<Vec<_>>()
    );
}
