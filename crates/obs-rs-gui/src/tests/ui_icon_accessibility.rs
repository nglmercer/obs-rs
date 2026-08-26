use super::*;

/// Verifies that icon-only controls expose their existing visible action hint
/// as an accessible button name.
pub(super) fn exercise_compact_button_accessibility(ui: &MainWindow) {
    for label in ["Zoom out", "Zoom in"] {
        ElementHandle::find_by_accessible_label(ui, label)
            .find(|button| button.size().width > 0.0 && button.size().height > 0.0)
            .unwrap_or_else(|| panic!("visible compact button with accessible label {label:?}"));
    }
}
