use super::*;
use i_slint_backend_testing::AccessibleRole;

/// Verifies that icon-only controls expose their existing visible action hint
/// and default action.
pub(super) fn exercise_compact_button_accessibility(ui: &MainWindow) {
    let zoom_out = find_visible_button(ui, "Zoom out");
    let zoom_in = find_visible_button(ui, "Zoom in");
    let unavailable = find_visible_button(ui, "Virtual camera options");
    assert_eq!(zoom_out.accessible_enabled(), Some(true));
    assert_eq!(zoom_in.accessible_enabled(), Some(true));
    assert_eq!(
        unavailable.accessible_enabled(),
        Some(false),
        "a disabled compact control must expose its unavailable state"
    );

    assert_eq!(
        ui.get_canvas_zoom(),
        0,
        "the fixture starts at Fit to Window"
    );
    zoom_in.invoke_accessible_default_action();
    assert_eq!(
        ui.get_canvas_zoom(),
        25,
        "the accessible action activates zoom"
    );
    ui.invoke_canvas_zoom_changed(0);
}

/// Verifies that dock-header actions expose the dock they operate on rather
/// than leaving the icon-only controls anonymous.
pub(super) fn exercise_dock_header_accessibility(ui: &MainWindow) {
    for label in ["Float panel: Scenes", "Close panel: Scenes"] {
        ElementHandle::find_by_accessible_label(ui, label)
            .find(|button| {
                button.accessible_role() == Some(AccessibleRole::Button)
                    && button.accessible_enabled() == Some(true)
                    && button.size().width > 0.0
                    && button.size().height > 0.0
            })
            .unwrap_or_else(|| panic!("visible dock action with accessible label {label:?}"));
    }
}

fn find_visible_button(ui: &MainWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label)
        .find(|button| {
            button.accessible_role() == Some(AccessibleRole::Button)
                && button.size().width > 0.0
                && button.size().height > 0.0
        })
        .unwrap_or_else(|| panic!("visible compact button with accessible label {label:?}"))
}

/// Verifies that the custom menu bar and popup entries expose their labels,
/// roles, enabled state, and default accessibility actions.
pub(super) fn exercise_navigation_accessibility(ui: &MainWindow) {
    let file_button = ElementHandle::find_by_element_id(ui, "AppNavbar::file-button")
        .next()
        .expect("File menu button is discoverable");
    assert_eq!(
        file_button.accessible_role(),
        Some(AccessibleRole::Button),
        "the menu-bar entry has button semantics"
    );
    assert_eq!(
        file_button.accessible_label().as_deref(),
        Some("File"),
        "the menu-bar entry exposes its visible label"
    );
    // Keep the existing pointer coverage for the popup boundary, then use
    // the accessible action on one entry so the document is reset only once.
    file_button.mock_single_click(PointerEventButton::Left);

    let entries = ElementHandle::find_by_element_type_name(ui, "MenuEntry").collect::<Vec<_>>();
    assert_eq!(entries.len(), 9, "the complete File popup is visible");
    assert_eq!(
        entries[0].accessible_role(),
        Some(AccessibleRole::Button),
        "the popup entry has button semantics"
    );
    assert_eq!(
        entries[0].accessible_label().as_deref(),
        Some("New project"),
        "the popup entry exposes its visible label"
    );
    assert_eq!(entries[0].accessible_enabled(), Some(true));
    entries[0].invoke_accessible_default_action();
    assert_eq!(
        ElementHandle::find_by_element_type_name(ui, "MenuEntry").count(),
        0,
        "the accessible action closes the popup"
    );
}

/// Verifies that scene and source rows expose list-item state while retaining
/// their stable project targets and selecting through the existing callbacks.
pub(super) fn exercise_row_accessibility(ui: &MainWindow) {
    let scene = find_accessible_row(ui, "preview");
    let scene_count = ui.get_scene_rows().row_count();
    assert_eq!(scene.accessible_description().as_deref(), Some("Preview"));
    assert_eq!(scene.accessible_enabled(), Some(true));
    assert_eq!(scene.accessible_item_selectable(), Some(true));
    assert_eq!(scene.accessible_item_selected(), Some(true));
    assert_eq!(scene.accessible_item_count(), Some(scene_count));
    assert!(scene
        .accessible_item_index()
        .is_some_and(|index| index < scene_count));

    let program = find_accessible_row(ui, "program");
    program.invoke_accessible_default_action();
    assert_eq!(ui.get_preview_scene().as_str(), "program");

    let source = find_accessible_row(ui, "background_program");
    assert_eq!(
        source.accessible_description().as_deref(),
        Some("Background")
    );
    assert_eq!(source.accessible_enabled(), Some(true));
    assert_eq!(source.accessible_item_selectable(), Some(true));
    assert_eq!(source.accessible_item_selected(), Some(true));
    assert_eq!(source.accessible_item_count(), Some(1));
    source.invoke_accessible_default_action();
    assert_eq!(ui.get_selected_source().as_str(), "background_program");

    // Leave the integrated fixture in the same scene/source state for the
    // following keyboard, properties, and projector workflows.
    ui.invoke_select_preview("preview".into());
}

/// Verifies that Multiview tiles expose the scene projection and use the
/// existing preview-selection callback for their default action.
pub(super) fn exercise_multiview_accessibility(ui: &MainWindow) {
    let previous_mode = ui.get_view_mode();
    ui.set_view_mode(2);

    let tile = find_accessible_multiview_tile(ui, "intermission");
    let multiview_count = ui.get_multiview_scenes().row_count();
    assert_eq!(
        tile.accessible_description().as_deref(),
        Some("Intermission")
    );
    assert_eq!(tile.accessible_enabled(), Some(true));
    assert_eq!(tile.accessible_item_selectable(), Some(true));
    assert_eq!(tile.accessible_item_selected(), Some(false));
    assert_eq!(tile.accessible_item_count(), Some(multiview_count));
    assert!(tile
        .accessible_item_index()
        .is_some_and(|index| index < multiview_count));

    tile.invoke_accessible_default_action();
    assert_eq!(ui.get_preview_scene().as_str(), "intermission");

    ui.invoke_select_preview("preview".into());
    ui.set_view_mode(previous_mode);
}

fn find_accessible_multiview_tile(ui: &MainWindow, target: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, target)
        .find(|tile| {
            tile.accessible_role() == Some(AccessibleRole::ListItem)
                && tile.size().width > 100.0
                && tile.size().height > 100.0
        })
        .unwrap_or_else(|| panic!("visible accessible multiview tile for {target:?}"))
}

fn find_accessible_row(ui: &MainWindow, target: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, target)
        .find(|row| {
            row.accessible_role() == Some(AccessibleRole::ListItem) && row.size().height > 30.0
        })
        .unwrap_or_else(|| panic!("visible accessible list row for {target:?}"))
}
