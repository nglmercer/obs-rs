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
