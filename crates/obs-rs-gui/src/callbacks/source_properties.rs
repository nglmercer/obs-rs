//! Controller for the standalone source properties window.
//!
//! The window edits only source-specific settings. Scene-item transforms and
//! source filters have their own standalone windows and command paths.

use std::{cell::RefCell, rc::Rc};

use obs_rs_config::Config;
use obs_rs_ui::{DesktopState, UiLocale};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    apply_source_settings_and_refresh, kind_selects_monitor, properties, source_settings, I18n,
    MainWindow, Palette, PreviewSurface, SourcePropertiesWindow,
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
    surface: &Rc<RefCell<PreviewSurface>>,
) -> Result<Rc<SourcePropertiesController>, slint::PlatformError> {
    let controller = Rc::new(SourcePropertiesController {
        window: SourcePropertiesWindow::new()?,
    });

    install_open(ui, state, &controller);
    install_editing(ui, state, &controller);
    install_commit(ui, state, surface, &controller);
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
        window.set_source_name(source_name(&state, &selected).into());
        window.set_source_kind(kind.as_str().into());
        window.set_source_kind_label(kind_label(&kind, locale));
        window.set_capture_capabilities(ui.get_capture_capabilities());
        // Start from what the studio last synced from the project.
        window.set_source_settings(ui.get_source_settings());
        window.set_monitor_visible(kind_selects_monitor(&kind));
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
    });
}

fn install_commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    controller: &Rc<SourcePropertiesController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let surface = Rc::clone(surface);
    let accept_controller = Rc::clone(controller);
    controller.window.on_accept_properties(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let window = &accept_controller.window;
        // Keep the studio's draft in step, then commit only source settings.
        ui.set_source_settings(window.get_source_settings());
        apply_source_settings_and_refresh(
            &ui,
            &state,
            &surface,
            window.get_source_settings().as_str(),
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
    let Some(profile) = project.active_profile_spec() else {
        return String::new();
    };
    let Some(item) = profile
        .scene(scene_id.as_str())
        .and_then(|scene| scene.item(source_id))
    else {
        return String::new();
    };
    profile
        .source(item.source_id())
        .map_or_else(String::new, |source| source.kind().as_str().to_owned())
}

/// Looks up the display name separately from the stable source ID used by the
/// selection model, so the dialog title matches what the user sees in Sources.
fn source_name(state: &Rc<RefCell<DesktopState>>, source_id: &str) -> String {
    let state = state.borrow();
    let Some(scene_id) = state.preview_scene().map(str::to_owned) else {
        return source_id.to_owned();
    };
    let Some(profile) = state.project_session().project().active_profile_spec() else {
        return source_id.to_owned();
    };
    let Some(item) = profile
        .scene(scene_id.as_str())
        .and_then(|scene| scene.item(source_id))
    else {
        return source_id.to_owned();
    };
    profile
        .source(item.source_id())
        .map_or_else(|| source_id.to_owned(), |source| source.name().to_owned())
}
