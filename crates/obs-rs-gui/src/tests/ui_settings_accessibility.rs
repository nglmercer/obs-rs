use super::*;
use i_slint_backend_testing::AccessibleRole;

/// Verifies that the settings sidebar exposes its mutually exclusive pages as
/// tabs while keeping page selection in the `SettingsWindow` property.
pub(super) fn exercise_settings_category_accessibility(window: &SettingsWindow) {
    let general = ElementHandle::find_by_accessible_label(window, "General")
        .find(|tab| {
            tab.accessible_role() == Some(AccessibleRole::Tab)
                && tab.size().width > 100.0
                && tab.size().height > 20.0
        })
        .expect("the General settings category is accessible");
    assert_eq!(general.accessible_description().as_deref(), Some("General"));
    assert_eq!(general.accessible_enabled(), Some(true));
    assert_eq!(general.accessible_item_selectable(), Some(true));
    assert_eq!(general.accessible_item_selected(), Some(true));
    assert_eq!(general.accessible_item_index(), Some(0));
    assert_eq!(general.accessible_item_count(), Some(9));

    let appearance = ElementHandle::find_by_accessible_label(window, "Appearance")
        .find(|tab| {
            tab.accessible_role() == Some(AccessibleRole::Tab)
                && tab.size().width > 100.0
                && tab.size().height > 20.0
        })
        .expect("the Appearance settings category is accessible");
    assert_eq!(appearance.accessible_item_selected(), Some(false));
    assert_eq!(appearance.accessible_item_index(), Some(1));
    assert_eq!(appearance.accessible_item_count(), Some(9));

    appearance.invoke_accessible_default_action();
    assert_eq!(window.get_category(), 1);
    assert_eq!(general.accessible_item_selected(), Some(false));
    assert_eq!(appearance.accessible_item_selected(), Some(true));

    general.invoke_accessible_default_action();
    assert_eq!(window.get_category(), 0);
    assert_eq!(general.accessible_item_selected(), Some(true));
    assert_eq!(appearance.accessible_item_selected(), Some(false));
}

/// Verifies that the General page connects its visual labels to the native
/// language and snap-distance controls without adding page-side state.
pub(super) fn exercise_general_settings_control_accessibility(window: &SettingsWindow) {
    window.set_category(0);
    let language = ElementHandle::find_by_accessible_label(window, "Language")
        .find(|control| {
            control.accessible_role() == Some(AccessibleRole::Combobox)
                && control.accessible_enabled() == Some(true)
                && control.size().width > 100.0
                && control.size().height > 20.0
        })
        .expect("the General language control is accessible");
    assert_eq!(language.accessible_label().as_deref(), Some("Language"));

    let snap = ElementHandle::find_by_accessible_label(window, "Snap distance (canvas px)")
        .find(|control| {
            control.accessible_role() == Some(AccessibleRole::Spinbox)
                && control.accessible_enabled() == Some(true)
                && control.size().width > 100.0
                && control.size().height > 20.0
        })
        .expect("the General snap-distance control is accessible");
    assert_eq!(
        snap.accessible_description().as_deref(),
        Some("Sources snap to canvas and other visible sources within this distance. Ctrl temporarily disables snapping during a gesture.")
    );
}

/// Verifies that the Appearance page connects its visual Theme and Style
/// labels to the native combo boxes and the Font size label to both controls
/// in its paired `SpinBox`/`Slider`, without adding page-side state.
pub(super) fn exercise_appearance_settings_control_accessibility(window: &SettingsWindow) {
    window.set_category(1);
    for label in ["Theme", "Style"] {
        let control = ElementHandle::find_by_accessible_label(window, label)
            .find(|control| {
                control.accessible_role() == Some(AccessibleRole::Combobox)
                    && control.accessible_enabled() == Some(true)
                    && control.size().width > 100.0
                    && control.size().height > 20.0
            })
            .unwrap_or_else(|| panic!("the Appearance {label} control is accessible"));
        assert_eq!(control.accessible_label().as_deref(), Some(label));
    }

    let font_size_field = ElementHandle::find_by_accessible_label(window, "Font size")
        .find(|control| {
            control.accessible_role() == Some(AccessibleRole::Spinbox)
                && control.accessible_enabled() == Some(true)
                && control.size().width > 60.0
                && control.size().height > 20.0
        })
        .expect("the Appearance font-size field is accessible");
    assert_eq!(
        font_size_field.accessible_label().as_deref(),
        Some("Font size")
    );

    let font_size_slider = ElementHandle::find_by_accessible_label(window, "Font size")
        .find(|control| {
            control.accessible_role() == Some(AccessibleRole::Slider)
                && control.accessible_enabled() == Some(true)
                && control.size().width > 100.0
                && control.size().height > 20.0
        })
        .expect("the Appearance font-size slider is accessible");
    assert_eq!(
        font_size_slider.accessible_label().as_deref(),
        Some("Font size")
    );
    window.set_category(0);
}

/// Verifies the shared Appearance density selector as a bounded radio group
/// while retaining the `SettingsWindow` draft callback as its state owner.
pub(super) fn exercise_settings_density_accessibility(window: &SettingsWindow) {
    window.set_category(1);
    let normal = ElementHandle::find_by_accessible_label(window, "Normal")
        .find(|option| {
            option.accessible_role() == Some(AccessibleRole::RadioButton)
                && option.size().width > 40.0
                && option.size().height > 20.0
        })
        .expect("the Normal density option is accessible");
    assert_eq!(normal.accessible_description().as_deref(), Some("Normal"));
    assert_eq!(normal.accessible_enabled(), Some(true));
    assert_eq!(normal.accessible_checkable(), Some(true));
    assert_eq!(normal.accessible_checked(), Some(true));
    assert_eq!(normal.accessible_item_index(), Some(2));
    assert_eq!(normal.accessible_item_count(), Some(4));

    let compact = ElementHandle::find_by_accessible_label(window, "Compact")
        .find(|option| {
            option.accessible_role() == Some(AccessibleRole::RadioButton)
                && option.size().width > 40.0
                && option.size().height > 20.0
        })
        .expect("the Compact density option is accessible");
    assert_eq!(compact.accessible_checked(), Some(false));
    assert_eq!(compact.accessible_item_index(), Some(1));
    assert_eq!(compact.accessible_item_count(), Some(4));

    compact.invoke_accessible_default_action();
    assert_eq!(window.get_density_index(), 1);
    assert_eq!(compact.accessible_checked(), Some(true));
    assert_eq!(normal.accessible_checked(), Some(false));

    normal.invoke_accessible_default_action();
    assert_eq!(window.get_density_index(), 2);
    assert_eq!(normal.accessible_checked(), Some(true));
    window.set_category(0);
}
