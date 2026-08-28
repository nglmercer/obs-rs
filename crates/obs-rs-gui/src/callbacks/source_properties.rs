//! Controller for the standalone source properties window.
//!
//! The window edits only source-specific settings. Scene-item transforms and
//! source filters have their own standalone windows and command paths.

use std::{cell::RefCell, rc::Rc};

use obs_rs_capture::CaptureKind;
use obs_rs_config::Config;
use obs_rs_ui::{DesktopState, UiLocale};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::callbacks::monitor::MonitorController;
use crate::{
    apply_source_settings_to, kind_selects_monitor, properties, source_settings_for_canvas,
    source_target, target_settings_document, I18n, MainWindow, Palette, PreviewSurface,
    SourcePropertiesWindow, SourceTarget,
};

/// Owns the properties window.
pub(crate) struct SourcePropertiesController {
    window: SourcePropertiesWindow,
    /// The source this dialog was opened for.
    ///
    /// The studio window stays usable while this dialog is open, so the source
    /// it writes to is fixed when it opens. Resolving the selection at OK would
    /// write a camera's device ID onto a screen capture the user clicked in the
    /// meantime.
    target: RefCell<Option<SourceTarget>>,
    /// The shared monitor picker, when this window is installed by the real
    /// desktop. Tests may install the properties window in isolation.
    monitor: Option<Rc<MonitorController>>,
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
#[cfg(test)]
pub(crate) fn install_source_properties_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) -> Result<Rc<SourcePropertiesController>, slint::PlatformError> {
    install_source_properties_window_with_monitor(ui, state, surface, None)
}

/// Creates the properties window with an explicit monitor-picker controller.
///
/// The extra boundary lets a nested screen source open the picker for its
/// stable path even after the main canvas selection changes. The legacy helper
/// above remains useful for isolated property-window fixtures.
pub(crate) fn install_source_properties_window_with_monitor(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    monitor: Option<&Rc<MonitorController>>,
) -> Result<Rc<SourcePropertiesController>, slint::PlatformError> {
    let controller = Rc::new(SourcePropertiesController {
        window: SourcePropertiesWindow::new()?,
        target: RefCell::new(None),
        monitor: monitor.cloned(),
    });

    super::stinger_picker::install_source_image_picker(&controller.window);
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
    let window_state = Rc::clone(state);
    let window_controller = Rc::clone(controller);
    ui.on_open_source_properties_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let target = ui.get_selected_source().to_string();
        open_for_target(&ui, &window_state, &window_controller, &target);
    });

    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let controller = Rc::clone(controller);
    ui.on_open_source_properties_for(move |target| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        open_for_target(&ui, &state, &controller, target.as_str());
    });
}

fn open_for_target(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &SourcePropertiesController,
    item: &str,
) {
    let Some(target) = source_target(&state.borrow(), item) else {
        ui.set_status_message("Source settings failed: the target is not a source".into());
        return;
    };
    let locale = state.borrow().locale();
    let kind = crate::project_migration::host_source_kind(&source_kind(state, &target)).to_owned();
    let name = source_name(state, &target);
    let settings = target_settings_document(state, &target).unwrap_or_default();
    controller.target.replace(Some(target.clone()));
    let window = &controller.window;
    window
        .global::<I18n>()
        .set_text(crate::i18n::catalog(locale));
    controller.set_tokens(ui.global::<Palette>().get_tokens());
    window.set_source_name(name.into());
    window.set_source_kind(kind.as_str().into());
    window.set_source_kind_label(kind_label(&kind, locale));
    window.set_capture_capabilities(ui.get_capture_capabilities());
    window.set_source_settings(settings.into());
    // A nested target shares source settings, but its monitor picker must not
    // use the main window's selected-item callback. Keep that picker available
    // only when this dialog targets the selected top-level item.
    window.set_monitor_visible(
        kind_selects_monitor(&kind)
            && (controller.monitor.is_some() || ui.get_selected_source().as_str() == target.item),
    );
    controller.refresh_rows(locale);
    match window.show() {
        Ok(()) => window.invoke_focus_keyboard_boundary(),
        Err(error) => ui.set_status_message(format!("Properties window: {error}").into()),
    }
}

fn install_editing(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &Rc<SourcePropertiesController>,
) {
    let weak = ui.as_weak();
    controller.window.on_picker_status(move |message| {
        if let Some(ui) = weak.upgrade() {
            ui.set_status_message(message);
        }
    });

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

    let refresh_state = Rc::clone(state);
    let refresh_controller = Rc::clone(controller);
    controller.window.on_refresh_properties(move || {
        let locale = refresh_state.borrow().locale();
        let kind = refresh_controller.window.get_source_kind().to_string();
        let document = refresh_controller.window.get_source_settings().to_string();
        invalidate_capture_cache(&kind, &document);
        refresh_controller.refresh_rows(locale);
    });

    // The picker edits the project directly, so the properties window hands the
    // request to the studio and closes its own draft to avoid two writers.
    let weak = ui.as_weak();
    let monitor_controller = Rc::clone(controller);
    let monitor_state = Rc::clone(state);
    controller.window.on_open_monitor_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let _ = monitor_controller.window.hide();
        if let Some(monitor) = monitor_controller.monitor.as_ref() {
            let target = monitor_controller.target.borrow().clone();
            if let Some(target) = target {
                monitor.open_for_target(&ui, &monitor_state, &target);
            } else {
                ui.set_status_message("Display selection failed: the source is gone".into());
            }
        } else {
            ui.invoke_open_monitor_window();
        }
    });

    let defaults_state = Rc::clone(state);
    let defaults_controller = Rc::clone(controller);
    controller.window.on_restore_defaults(move || {
        let window = &defaults_controller.window;
        let kind = window.get_source_kind().to_string();
        let (canvas_width, canvas_height) = active_canvas_size(&defaults_state);
        if let Ok(defaults) = source_settings_for_canvas(&kind, canvas_width, canvas_height) {
            window.set_source_settings(defaults.serialize().into());
            defaults_controller.refresh_rows(defaults_state.borrow().locale());
        }
    });
}

fn invalidate_capture_cache(kind: &str, document: &str) {
    let capture_kind = match crate::project_migration::host_source_kind(kind) {
        "screen_capture" | "x11_screen_capture" => Some(CaptureKind::Screen),
        "window_capture" | "x11_window_capture" => Some(CaptureKind::Window),
        "camera_capture" => Some(CaptureKind::Camera),
        _ => None,
    };
    let camera_id = if capture_kind == Some(CaptureKind::Camera) {
        Config::parse(document)
            .ok()
            .and_then(|settings| settings.get("device_id").map(str::to_owned))
    } else {
        None
    };
    if let Some(capture_kind) = capture_kind {
        crate::fixtures::invalidate_capture_cache(capture_kind, camera_id.as_deref());
    }
}

fn active_canvas_size(state: &Rc<RefCell<DesktopState>>) -> (u32, u32) {
    state
        .borrow()
        .project_session()
        .project()
        .active_profile_spec()
        .map_or((1_280, 720), |profile| {
            let format = profile.video_format();
            (format.width(), format.height())
        })
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
        let Some(target) = accept_controller.target.borrow().clone() else {
            ui.set_status_message("Source settings failed: the source is gone".into());
            let _ = window.hide();
            return;
        };
        // Keep the studio's selected-item draft in step, then commit only
        // source settings. A nested target deliberately does not replace the
        // unrelated top-level editor draft.
        if ui.get_selected_source().as_str() == target.item {
            ui.set_source_settings(window.get_source_settings());
        }
        apply_source_settings_to(
            &ui,
            &state,
            &surface,
            &target,
            window.get_source_settings().as_str(),
        );
        let _ = window.hide();
    });

    let cancel_controller = Rc::clone(controller);
    controller.window.on_cancel_properties(move || {
        let _ = cancel_controller.window.hide();
    });

    // Treat the native window-manager close exactly like Cancel so a staged
    // source-settings edit cannot escape the dialog without an explicit
    // commit. The controller remains alive and can reopen a fresh draft.
    let close_controller = Rc::clone(controller);
    controller.window.window().on_close_requested(move || {
        let _ = close_controller.window.hide();
        slint::CloseRequestResponse::HideWindow
    });
}

/// Describes the display a screen source is pointed at, for the picker button.
fn monitor_summary(document: &str, locale: UiLocale) -> SharedString {
    let monitor = Config::parse(document)
        .ok()
        .and_then(|settings| settings.get("monitor").map(str::to_owned))
        .unwrap_or_default();
    let monitor = monitor.trim();
    if monitor.is_empty() || (cfg!(target_os = "windows") && monitor == "wgc-screen-picker") {
        crate::i18n::with_catalog(locale, |text| {
            if cfg!(target_os = "windows") {
                text.monitor_ui.automatic_display.clone()
            } else {
                text.monitor_ui.whole_desktop.clone()
            }
        })
    } else {
        monitor.into()
    }
}

/// Returns the translated name of a source kind, falling back to its id.
fn kind_label(kind: &str, locale: UiLocale) -> SharedString {
    crate::i18n::with_catalog(locale, |text| {
        crate::callbacks::add_source::kind_label(&text.add_source_ui, kind)
    })
}

/// Looks up the kind of a stable source target.
fn source_kind(state: &Rc<RefCell<DesktopState>>, target: &SourceTarget) -> String {
    let state = state.borrow();
    state
        .project_session()
        .project()
        .profile(target.profile.as_str())
        .and_then(|profile| profile.source(target.source.as_str()))
        .map_or_else(String::new, |source| source.kind().as_str().to_owned())
}

/// Looks up the display name for a stable source target.
fn source_name(state: &Rc<RefCell<DesktopState>>, target: &SourceTarget) -> String {
    let state = state.borrow();
    state
        .project_session()
        .project()
        .profile(target.profile.as_str())
        .and_then(|profile| profile.source(target.source.as_str()))
        .map_or_else(|| target.source.clone(), |source| source.name().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_monitor_summary_uses_the_platform_label() {
        let document = "device_id = \"wgc-screen-picker\"\nmonitor = \"\"\n";
        let summary = monitor_summary(document, UiLocale::English);

        #[cfg(target_os = "windows")]
        assert_eq!(summary, "Primary display (automatic)");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(summary, "Capture the whole desktop instead of one display");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn legacy_windows_monitor_sentinel_uses_the_platform_label() {
        let document = "monitor = \"wgc-screen-picker\"\n";

        assert_eq!(
            monitor_summary(document, UiLocale::English),
            "Primary display (automatic)"
        );
    }
}
