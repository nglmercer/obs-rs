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

fn find_visible_button(ui: &MainWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label)
        .find(|button| {
            button.accessible_role() == Some(AccessibleRole::Button)
                && button.size().width > 0.0
                && button.size().height > 0.0
        })
        .unwrap_or_else(|| panic!("visible compact button with accessible label {label:?}"))
}
