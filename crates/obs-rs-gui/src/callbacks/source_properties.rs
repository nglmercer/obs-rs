//! Controller for the standalone source properties window.
//!
//! The window edits copies of the selected source's settings, transform, and
//! filter documents; the three existing apply paths run only on OK. Editing is
//! done through the typed form in [`crate::properties`], with the raw document
//! kept behind the dialog's advanced section as the escape hatch for anything
//! the form does not model.

use std::{cell::RefCell, rc::Rc};

use obs_rs_config::Config;
use obs_rs_ui::{DesktopState, UiLocale};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    apply_source_filters_and_refresh, apply_source_settings_and_refresh,
    apply_source_transform_and_refresh, kind_selects_monitor, properties, source_settings, I18n,
    MainWindow, Palette, PreviewRenderer, SourcePropertiesWindow,
};

/// Owns the properties window.
pub(crate) struct SourcePropertiesController {
    window: SourcePropertiesWindow,
}

impl SourcePropertiesController {
    /// Repaints this window when the studio's theme changes.
    pub(crate) fn set_tokens(&self, tokens: crate::ThemeTokens) {
        self.window.global::<Palette>().set_tokens(tokens);
    }

    #[cfg(test)]
    pub(crate) fn window(&self) -> &SourcePropertiesWindow {
        &self.window
    }

    /// Rebuilds the typed rows from the window's current settings draft.
    fn refresh_rows(&self, locale: UiLocale) {
        let window = &self.window;
        let kind = window.get_source_kind().to_string();
        let document = window.get_source_settings().to_string();
        window.set_property_rows(ModelRc::new(VecModel::from(properties::rows(
            &kind, &document, locale,
        ))));
        window.set_monitor_summary(monitor_summary(&document, locale));
    }
}

/// Creates the properties window and wires it to the studio window.
///
/// The returned controller must outlive the event loop; dropping it closes the
/// window.
pub(crate) fn install_source_properties_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) -> Result<Rc<SourcePropertiesController>, slint::PlatformError> {
    let controller = Rc::new(SourcePropertiesController {
        window: SourcePropertiesWindow::new()?,
    });

    install_open(ui, state, &controller);
    install_editing(ui, state, &controller);
    install_commit(ui, state, renderer, &controller);
    Ok(controller)
}

fn install_open(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &Rc<SourcePropertiesController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let controller = Rc::clone(controller);
    ui.on_open_source_properties_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let selected = ui.get_selected_source().to_string();
        if selected.is_empty() {
            return;
        }
        let locale = state.borrow().locale();
        let window = &controller.window;
        window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(locale));
        controller.set_tokens(ui.global::<Palette>().get_tokens());
        let kind = source_kind(&state, &selected);
        window.set_source_name(selected.as_str().into());
        window.set_source_kind(kind.as_str().into());
        window.set_source_kind_label(kind_label(&kind, locale));
        window.set_capture_capabilities(ui.get_capture_capabilities());
        // Start from what the studio last synced from the project.
        window.set_source_settings(ui.get_source_settings());
        window.set_monitor_visible(kind_selects_monitor(&kind));
        window.set_source_transform(ui.get_source_transform());
        sync_transform_fields(window);
        window.set_source_filters(ui.get_source_filters());
        controller.refresh_rows(locale);
        if let Err(error) = window.show() {
            ui.set_status_message(format!("Properties window: {error}").into());
        }
    });
}

fn install_editing(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &Rc<SourcePropertiesController>,
) {
    let edit_state = Rc::clone(state);
    let edit_controller = Rc::clone(controller);
    controller.window.on_edit_property(move |key, value| {
        let window = &edit_controller.window;
        let kind = window.get_source_kind().to_string();
        let document = window.get_source_settings().to_string();
        // An edit the schema cannot represent leaves the draft untouched
        // instead of replacing it with a partial document.
        if let Some(updated) = properties::apply(&kind, &document, key.as_str(), value.as_str()) {
            window.set_source_settings(updated.into());
            edit_controller.refresh_rows(edit_state.borrow().locale());
        }
    });

    let transform_controller = Rc::clone(controller);
    controller.window.on_edit_transform(move |key, value| {
        edit_transform_draft(&transform_controller.window, key.as_str(), value.as_str());
    });

    // The picker edits the project directly, so the properties window hands the
    // request to the studio and closes its own draft to avoid two writers.
    let weak = ui.as_weak();
    let monitor_controller = Rc::clone(controller);
    controller.window.on_open_monitor_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let _ = monitor_controller.window.hide();
        ui.invoke_open_monitor_window();
    });

    let defaults_state = Rc::clone(state);
    let defaults_controller = Rc::clone(controller);
    controller.window.on_restore_defaults(move || {
        let window = &defaults_controller.window;
        let kind = window.get_source_kind().to_string();
        if let Ok(defaults) = source_settings(&kind) {
            window.set_source_settings(defaults.serialize().into());
            defaults_controller.refresh_rows(defaults_state.borrow().locale());
        }
        // The identity transform and an empty filter chain are the documented
        // defaults for a freshly created source.
        window.set_source_transform("1000,1000,0,0,0,0,255,0,0,0,0".into());
        sync_transform_fields(window);
        window.set_source_filters(String::new().into());
    });
}

/// Copies the serialized transform draft into the typed controls.
fn sync_transform_fields(window: &SourcePropertiesWindow) {
    let values = normalized_transform(window.get_source_transform().as_str());
    let number = |index: usize| values[index].parse::<i32>().unwrap_or(0);
    window.set_item_scale_x(number(0));
    window.set_item_scale_y(number(1));
    window.set_item_x(number(2));
    window.set_item_y(number(3));
    window.set_item_flip_x(values[4] == "1" || values[4] == "true");
    window.set_item_flip_y(values[5] == "1" || values[5] == "true");
    window.set_item_opacity(number(6));
    window.set_crop_left(number(7));
    window.set_crop_top(number(8));
    window.set_crop_right(number(9));
    window.set_crop_bottom(number(10));
}

/// Rewrites one typed transform field while preserving every other field.
fn edit_transform_draft(window: &SourcePropertiesWindow, key: &str, value: &str) {
    let mut values = normalized_transform(window.get_source_transform().as_str());
    let index = match key {
        "scale-x" => 0,
        "scale-y" => 1,
        "x" => 2,
        "y" => 3,
        "flip-x" => 4,
        "flip-y" => 5,
        "opacity" => 6,
        "crop-left" => 7,
        "crop-top" => 8,
        "crop-right" => 9,
        "crop-bottom" => 10,
        _ => return,
    };
    values[index] = value.trim().to_owned();
    window.set_source_transform(values.join(",").into());
    sync_transform_fields(window);
}

/// Expands the legacy seven-field shape with zero crop values.
fn normalized_transform(document: &str) -> Vec<String> {
    let mut values = document
        .split(',')
        .map(|value| value.trim().to_owned())
        .collect::<Vec<_>>();
    if values.len() == 7 {
        values.extend(["0", "0", "0", "0"].map(str::to_owned));
    }
    if values.len() != 11 {
        return vec![
            "1000".to_owned(),
            "1000".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "255".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
            "0".to_owned(),
        ];
    }
    values
}

fn install_commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    controller: &Rc<SourcePropertiesController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    let accept_controller = Rc::clone(controller);
    controller.window.on_accept_properties(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let window = &accept_controller.window;
        // Keep the studio's copies in step, then run the same apply paths the
        // in-window editor used.
        ui.set_source_settings(window.get_source_settings());
        ui.set_source_transform(window.get_source_transform());
        ui.set_source_filters(window.get_source_filters());
        apply_source_settings_and_refresh(
            &ui,
            &state,
            &renderer,
            window.get_source_settings().as_str(),
        );
        apply_source_transform_and_refresh(
            &ui,
            &state,
            &renderer,
            window.get_source_transform().as_str(),
        );
        apply_source_filters_and_refresh(
            &ui,
            &state,
            &renderer,
            window.get_source_filters().as_str(),
        );
        let _ = window.hide();
    });

    let cancel_controller = Rc::clone(controller);
    controller.window.on_cancel_properties(move || {
        let _ = cancel_controller.window.hide();
    });
}

/// Describes the display a screen source is pointed at, for the picker button.
fn monitor_summary(document: &str, locale: UiLocale) -> SharedString {
    let monitor = Config::parse(document)
        .ok()
        .and_then(|settings| settings.get("monitor").map(str::to_owned))
        .unwrap_or_default();
    if monitor.trim().is_empty() {
        crate::i18n::with_catalog(locale, |text| text.monitor_ui.whole_desktop.clone())
    } else {
        monitor.as_str().into()
    }
}

/// Returns the translated name of a source kind, falling back to its id.
fn kind_label(kind: &str, locale: UiLocale) -> SharedString {
    crate::i18n::with_catalog(locale, |text| {
        crate::callbacks::add_source::kind_label(&text.add_source_ui, kind)
    })
}

/// Looks up the kind of the selected source in the preview scene.
fn source_kind(state: &Rc<RefCell<DesktopState>>, source_id: &str) -> String {
    let state = state.borrow();
    let Some(scene_id) = state.preview_scene().map(str::to_owned) else {
        return String::new();
    };
    let session = state.project_session();
    let project = session.project();
    let kind = project
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id.as_str()))
        .and_then(|scene| scene.source(source_id))
        .map(|source| source.kind().as_str().to_owned())
        .unwrap_or_default();
    kind
}
