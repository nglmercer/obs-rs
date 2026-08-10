//! Controller for the standalone source properties window.
//!
//! The window edits copies of the selected source's settings, transform, and
//! filter documents; the three existing apply paths run only on OK.

use std::{cell::RefCell, rc::Rc};

use obs_rs_ui::DesktopState;
use slint::ComponentHandle;

use crate::{
    apply_source_filters_and_refresh, apply_source_settings_and_refresh,
    apply_source_transform_and_refresh, source_settings, I18n, MainWindow, Palette,
    PreviewRenderer, SourcePropertiesWindow,
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

    let weak = ui.as_weak();
    let open_state = Rc::clone(state);
    let open_controller = Rc::clone(&controller);
    ui.on_open_source_properties_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let selected = ui.get_selected_source().to_string();
        if selected.is_empty() {
            return;
        }
        let window = &open_controller.window;
        window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(open_state.borrow().locale()));
        open_controller.set_tokens(ui.global::<Palette>().get_tokens());
        window.set_source_name(selected.as_str().into());
        window.set_source_kind(source_kind(&open_state, &selected).into());
        window.set_capture_capabilities(ui.get_capture_capabilities());
        // Start from what the studio last synced from the project.
        window.set_source_settings(ui.get_source_settings());
        window.set_source_transform(ui.get_source_transform());
        window.set_source_filters(ui.get_source_filters());
        if let Err(error) = window.show() {
            ui.set_status_message(format!("Properties window: {error}").into());
        }
    });

    let weak = ui.as_weak();
    let accept_state = Rc::clone(state);
    let accept_renderer = Rc::clone(renderer);
    let accept_controller = Rc::clone(&controller);
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
            &accept_state,
            &accept_renderer,
            window.get_source_settings().as_str(),
        );
        apply_source_transform_and_refresh(
            &ui,
            &accept_state,
            &accept_renderer,
            window.get_source_transform().as_str(),
        );
        apply_source_filters_and_refresh(
            &ui,
            &accept_state,
            &accept_renderer,
            window.get_source_filters().as_str(),
        );
        let _ = window.hide();
    });

    let cancel_controller = Rc::clone(&controller);
    controller.window.on_cancel_properties(move || {
        let _ = cancel_controller.window.hide();
    });

    let defaults_controller = Rc::clone(&controller);
    controller.window.on_restore_defaults(move || {
        let window = &defaults_controller.window;
        let kind = window.get_source_kind().to_string();
        if let Ok(defaults) = source_settings(&kind) {
            window.set_source_settings(defaults.serialize().into());
        }
        // The identity transform and an empty filter chain are the documented
        // defaults for a freshly created source.
        window.set_source_transform("1000,1000,0,0,0,0,255".into());
        window.set_source_filters(String::new().into());
    });

    Ok(controller)
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
        .profiles()
        .find(|profile| profile.id() == project.active_profile())
        .and_then(|profile| {
            profile
                .scenes()
                .find(|scene| scene.id().as_str() == scene_id)
        })
        .and_then(|scene| {
            scene
                .sources()
                .iter()
                .find(|source| source.id().as_str() == source_id)
        })
        .map(|source| source.kind().as_str().to_owned())
        .unwrap_or_default();
    kind
}
