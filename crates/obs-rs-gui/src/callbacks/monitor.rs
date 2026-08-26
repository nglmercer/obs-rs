//! Controller for the display picker.
//!
//! A multi-head desktop is a single X11 screen, so "screen capture" without a
//! chosen display grabs every monitor at once. This window is what turns that
//! into an explicit choice; the selection is stored in the source's `monitor`
//! setting and therefore travels with the project file.

use std::{cell::RefCell, rc::Rc};

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use obs_rs_config::Config;
use obs_rs_ui::DesktopState;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    apply_source_settings_to,
    fixtures::{desktop_bounds, kind_selects_monitor, screen_monitors, MonitorChoice},
    source_target, target_settings_document, I18n, MainWindow, MonitorRow, MonitorWindow, Palette,
    PreviewSurface, SourceTarget,
};

/// Only one compositor picker may be active at a time. A portal request is a
/// separate D-Bus session, so repeated UI callbacks otherwise create several
/// identical dialogs stacked on top of each other.
#[cfg(target_os = "linux")]
static PORTAL_REQUEST_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "linux")]
static PORTAL_HANDOFF_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "linux")]
struct PortalRequestGuard;

#[cfg(target_os = "linux")]
impl Drop for PortalRequestGuard {
    fn drop(&mut self) {
        PORTAL_REQUEST_IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// Owns the picker window and the draft selection it edits.
///
/// Both display-selection paths outlive the click that opened them — the X11
/// picker is a window the user can leave open, and the Wayland portal is the
/// compositor's own dialog — while the studio window behind them stays
/// clickable. Both therefore pin the source they are choosing a display for.
/// Resolving "the selected source" on the way back writes a screen capture's
/// display or portal token onto whatever the user clicked in the meantime,
/// which is exactly how a working camera and a working screen share become one
/// of each.
pub(crate) struct MonitorController {
    window: MonitorWindow,
    monitors: RefCell<Vec<MonitorChoice>>,
    /// The display the user has highlighted; empty means none is highlighted.
    selected: RefCell<String>,
    /// The source the open picker window is choosing a display for.
    target: RefCell<Option<SourceTarget>>,
    /// The source whose portal handshake is in flight, if one is.
    ///
    /// Kept apart from `target` because a handshake can still be waiting on the
    /// compositor when the picker is opened again for something else.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pending_portal: RefCell<Option<SourceTarget>>,
}

impl MonitorController {
    /// Repaints this window when the studio's theme changes.
    pub(crate) fn set_tokens(&self, tokens: crate::ThemeTokens) {
        self.window.global::<Palette>().set_tokens(tokens);
    }

    #[cfg(test)]
    pub(crate) fn window(&self) -> &MonitorWindow {
        &self.window
    }

    /// Opens the picker for the currently selected scene-item.
    pub(crate) fn open_for_item(
        &self,
        ui: &MainWindow,
        state: &Rc<RefCell<DesktopState>>,
        item: &str,
    ) {
        let Some(target) = source_target(&state.borrow(), item) else {
            ui.set_status_message("Display selection failed: the source is gone".into());
            return;
        };
        self.open_for_target(ui, state, &target);
    }

    /// Opens the picker for an explicit scene-item target.
    ///
    /// Source Properties can remain open while the canvas selection changes,
    /// so resolving the selected row here could send a nested screen choice to
    /// an unrelated source. The target is resolved once at this boundary.
    pub(crate) fn open_for_target(
        &self,
        ui: &MainWindow,
        state: &Rc<RefCell<DesktopState>>,
        target: &SourceTarget,
    ) {
        open_for_target(ui, state, self, target);
    }

    /// Fills the window from the live display list for `source`.
    fn reload(&self, source_name: &str, current: Option<&str>) {
        let monitors = screen_monitors();
        let selected = current
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                monitors
                    .iter()
                    .find(|monitor| monitor.primary)
                    .or_else(|| monitors.first())
                    .map(|monitor| monitor.id.clone())
            })
            .unwrap_or_default();
        self.window
            .set_capture_whole_desktop(current.is_some_and(|value| value.trim().is_empty()));
        self.window.set_source_name(source_name.into());
        *self.selected.borrow_mut() = selected;
        self.monitors.replace(monitors);
        self.refresh_rows();
    }

    /// Rebuilds the row model and the normalized layout map.
    fn refresh_rows(&self) {
        let monitors = self.monitors.borrow();
        let selected = self.selected.borrow();
        let bounds = desktop_bounds(&monitors);
        let rows = monitors
            .iter()
            .map(|monitor| MonitorRow {
                id: monitor.id.as_str().into(),
                name: monitor.name.as_str().into(),
                geometry: monitor.geometry().into(),
                primary: monitor.primary,
                selected: monitor.id == *selected,
                normalized_x: normalized(monitor.x - bounds.left, bounds.width),
                normalized_y: normalized(monitor.y - bounds.top, bounds.height),
                normalized_width: normalized(
                    i32::try_from(monitor.width).unwrap_or(i32::MAX),
                    bounds.width,
                ),
                normalized_height: normalized(
                    i32::try_from(monitor.height).unwrap_or(i32::MAX),
                    bounds.height,
                ),
            })
            .collect::<Vec<_>>();
        self.window.set_selected_id(selected.as_str().into());
        self.window
            .set_monitor_rows(ModelRc::new(VecModel::from(rows)));
    }
}

/// Converts a pixel extent into the 0..1 fraction the map draws with.
fn normalized(value: i32, extent: i32) -> f32 {
    #[allow(
        clippy::cast_precision_loss,
        reason = "desktop geometry is far below f32's exact integer range"
    )]
    let fraction = value as f32 / extent as f32;
    fraction.clamp(0.0, 1.0)
}

/// Creates the display picker and wires it to the studio window.
///
/// The returned controller must outlive the event loop; dropping it closes the
/// window.
pub(crate) fn install_monitor_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) -> Result<Rc<MonitorController>, slint::PlatformError> {
    let controller = Rc::new(MonitorController {
        window: MonitorWindow::new()?,
        monitors: RefCell::new(Vec::new()),
        selected: RefCell::new(String::new()),
        target: RefCell::new(None),
        pending_portal: RefCell::new(None),
    });

    install_open(ui, state, &controller);
    #[cfg(target_os = "linux")]
    install_portal_token(ui, state, surface, &controller);
    install_selection(state, &controller);
    install_commit(ui, state, surface, &controller);
    Ok(controller)
}

fn install_open(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &Rc<MonitorController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let controller = Rc::clone(controller);
    ui.on_open_monitor_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let selected = ui.get_selected_source().to_string();
        controller.open_for_item(&ui, &state, &selected);
    });
}

fn open_for_target(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &MonitorController,
    target: &SourceTarget,
) {
    let kind = source_kind_for_target(state, target);
    if kind.is_empty() {
        ui.set_status_message("Display selection failed: the source is gone".into());
        return;
    }
    let locale = state.borrow().locale();
    controller.target.replace(Some(target.clone()));
    if !kind_selects_monitor(&kind) {
        ui.set_status_message(crate::i18n::with_catalog(locale, |text| {
            text.monitor_ui.not_a_screen_source.clone()
        }));
        return;
    }
    // On Wayland the compositor owns the picker: OBS-RS asks the portal and
    // stores the token it hands back, rather than showing a list of screens
    // it is not allowed to enumerate.
    if crate::kind_uses_portal(&kind) {
        share_through_portal(ui, state, controller, target.clone());
        return;
    }
    let window = &controller.window;
    window
        .global::<I18n>()
        .set_text(crate::i18n::catalog(locale));
    controller.set_tokens(ui.global::<Palette>().get_tokens());
    let source_name = source_name_for_target(state, target);
    controller.reload(
        &source_name,
        current_monitor_for_target(state, target).as_deref(),
    );
    window.set_status(status_line(&controller.monitors.borrow()).into());
    match window.show() {
        Ok(()) => window.invoke_focus_keyboard_boundary(),
        Err(error) => ui.set_status_message(format!("Display picker: {error}").into()),
    }
}

fn install_selection(state: &Rc<RefCell<DesktopState>>, controller: &Rc<MonitorController>) {
    let select_controller = Rc::clone(controller);
    controller.window.on_select_monitor(move |id| {
        *select_controller.selected.borrow_mut() = id.to_string();
        // Highlighting a display is the explicit opposite of the whole-desktop
        // checkbox, so choosing one clears it.
        select_controller.window.set_capture_whole_desktop(false);
        select_controller.refresh_rows();
    });

    let refresh_state = Rc::clone(state);
    let refresh_controller = Rc::clone(controller);
    controller.window.on_refresh_monitors(move || {
        let window = &refresh_controller.window;
        let Some(target) = refresh_controller.target.borrow().clone() else {
            return;
        };
        let source = window.get_source_name().to_string();
        let current = current_monitor_for_target(&refresh_state, &target);
        refresh_controller.reload(&source, current.as_deref());
        window.set_status(status_line(&refresh_controller.monitors.borrow()).into());
    });

    let cancel_controller = Rc::clone(controller);
    controller.window.on_cancel_monitor(move || {
        let _ = cancel_controller.window.hide();
    });
}

fn install_commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    controller: &Rc<MonitorController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let surface = Rc::clone(surface);
    let accept_controller = Rc::clone(controller);
    controller.window.on_accept_monitor(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let window = &accept_controller.window;
        let Some(target) = accept_controller.target.borrow().clone() else {
            ui.set_status_message("Display selection failed: the source is gone".into());
            return;
        };
        let monitor = if window.get_capture_whole_desktop() {
            String::new()
        } else {
            accept_controller.selected.borrow().clone()
        };
        let Some(document) = monitor_document(&state, &target, &monitor) else {
            ui.set_status_message("Display selection failed: source settings are invalid".into());
            return;
        };
        // The shared apply path validates the document and writes it through the
        // project command for the source this picker was opened for, so a
        // display change is recorded as an ordinary undoable project edit.
        apply_source_settings_to(&ui, &state, &surface, &target, &document);
        let applied = crate::i18n::with_catalog(state.borrow().locale(), |text| {
            text.monitor_ui.applied.clone()
        });
        let label = if monitor.is_empty() {
            crate::i18n::with_catalog(state.borrow().locale(), |text| {
                text.monitor_ui.whole_desktop.clone()
            })
        } else {
            monitor.as_str().into()
        };
        ui.set_status_message(format!("{applied}{label}").into());
        let _ = window.hide();
    });
}

/// Asks the compositor to share a screen, off the event loop.
///
/// The portal dialog waits for a person, so running the handshake inline would
/// freeze the studio window for as long as the user takes to answer. The
/// handshake therefore runs on its own thread and reports back through
/// `apply-portal-token`, which is wired to the project on the UI thread.
#[cfg(target_os = "linux")]
fn share_through_portal(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &MonitorController,
    target: SourceTarget,
) {
    use obs_rs_capture::{
        open_screencast, publish_wayland_portal_handoff, CursorMode, WaylandCaptureDevice,
    };

    if PORTAL_REQUEST_IN_FLIGHT.swap(true, Ordering::AcqRel) {
        let waiting = crate::i18n::with_catalog(state.borrow().locale(), |text| {
            text.monitor_ui.portal_waiting.clone()
        });
        ui.set_status_message(waiting);
        return;
    }

    let source = target.item.clone();
    let handoff_id = format!(
        "{}-{}",
        std::process::id(),
        PORTAL_HANDOFF_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let callback_handoff_id = handoff_id.clone();
    let cursor = target_settings_document(state, &target)
        .as_deref()
        .and_then(|document| Config::parse(document).ok())
        .and_then(|settings| settings.get("capture_cursor").map(str::to_owned))
        .is_none_or(|value| value.trim() != "false");
    let waiting = crate::i18n::with_catalog(state.borrow().locale(), |text| {
        text.monitor_ui.portal_waiting.clone()
    });
    ui.set_status_message(waiting);
    // The answer belongs to this source, not to whatever is selected when the
    // compositor's dialog finally closes.
    controller.pending_portal.replace(Some(target));

    let weak = ui.as_weak();
    let handshake = std::thread::Builder::new()
        .name("obs-rs-screencast".to_owned())
        .spawn(move || {
            let _request_guard = PortalRequestGuard;
            let outcome = match open_screencast(
                None,
                if cursor {
                    CursorMode::Embedded
                } else {
                    CursorMode::Hidden
                },
            ) {
                // Keep this exact live session. Dropping it and reopening from
                // a token would perform a second portal handshake and can
                // show a second picker on compositors that do not restore it.
                Ok(session) => {
                    WaylandCaptureDevice::from_session("wayland-screen", "Wayland screen", session)
                        .map(|device| {
                            publish_wayland_portal_handoff(handoff_id, device);
                            callback_handoff_id
                        })
                        .map_err(|error| error.to_string())
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                match outcome {
                    Ok(handoff_id) => {
                        ui.invoke_apply_portal_token(source.into(), handoff_id.into());
                    }
                    Err(error) => {
                        ui.set_status_message(
                            format!("Screen sharing was not started: {error}").into(),
                        );
                    }
                }
            });
        });
    if let Err(error) = handshake {
        PORTAL_REQUEST_IN_FLIGHT.store(false, Ordering::Release);
        ui.set_status_message(format!("Screen sharing was not started: {error}").into());
    }
}

/// Hands the live portal session to the selected source, on the UI thread.
#[cfg(target_os = "linux")]
fn install_portal_token(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    controller: &Rc<MonitorController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let surface = Rc::clone(surface);
    let controller = Rc::clone(controller);
    ui.on_apply_portal_token(move |source, token| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // The target recorded when the handshake started wins over both the
        // current selection and the item ID the callback carries, either of
        // which may name a different source by now.
        let target = controller
            .pending_portal
            .borrow_mut()
            .take()
            .or_else(|| source_target(&state.borrow(), source.as_str()));
        let Some(target) = target else {
            ui.set_status_message("Screen sharing failed: the source is gone".into());
            return;
        };
        let document = target_settings_document(&state, &target)
            .as_deref()
            .and_then(|document| Config::parse(document).ok());
        let Some(mut settings) = document else {
            ui.set_status_message("Screen sharing failed: source settings are invalid".into());
            return;
        };
        settings.remove("restore_token");
        if settings
            .set(
                obs_rs_capture::WAYLAND_PORTAL_HANDOFF_SETTING,
                token.as_str(),
            )
            .is_err()
        {
            ui.set_status_message(
                "Screen sharing failed: the live session could not be handed off".into(),
            );
            return;
        }
        apply_source_settings_to(&ui, &state, &surface, &target, &settings.serialize());
        let applied = crate::i18n::with_catalog(state.borrow().locale(), |text| {
            text.monitor_ui.portal_applied.clone()
        });
        ui.set_status_message(applied);
    });
}

/// Builds the source's settings document with `monitor` set to the choice.
///
/// The display string is written alongside it so the document stays a complete
/// description of what the source reads.
fn monitor_document(
    state: &Rc<RefCell<DesktopState>>,
    target: &SourceTarget,
    monitor: &str,
) -> Option<String> {
    let mut settings = target_settings_document(state, target)
        .as_deref()
        .and_then(|document| Config::parse(document).ok())?;
    if let Ok(display) = std::env::var("DISPLAY") {
        let _ = settings.set("display", &display);
    }
    settings.set("monitor", monitor).ok()?;
    Some(settings.serialize())
}

/// Returns the source kind for a resolved scene-item target.
fn source_kind_for_target(state: &Rc<RefCell<DesktopState>>, target: &SourceTarget) -> String {
    let state = state.borrow();
    state
        .project_session()
        .project()
        .profile(target.profile.as_str())
        .and_then(|profile| profile.source(target.source.as_str()))
        .map_or_else(String::new, |source| source.kind().as_str().to_owned())
}

/// Returns the display name for a resolved scene-item target.
fn source_name_for_target(state: &Rc<RefCell<DesktopState>>, target: &SourceTarget) -> String {
    let state = state.borrow();
    state
        .project_session()
        .project()
        .profile(target.profile.as_str())
        .and_then(|profile| profile.source(target.source.as_str()))
        .map_or_else(|| target.item.clone(), |source| source.name().to_owned())
}

fn current_monitor_for_target(
    state: &Rc<RefCell<DesktopState>>,
    target: &SourceTarget,
) -> Option<String> {
    let document = target_settings_document(state, target)?;
    let settings = Config::parse(&document).ok()?;
    settings.get("monitor").map(str::to_owned)
}

/// Summarizes the detected displays under the list.
fn status_line(monitors: &[MonitorChoice]) -> String {
    if monitors.is_empty() {
        return String::new();
    }
    let names = monitors
        .iter()
        .map(|monitor| monitor.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!("{} detected: {names}", monitors.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(name: &str, x: i32, y: i32, width: u32, height: u32) -> MonitorChoice {
        MonitorChoice {
            id: name.to_owned(),
            name: name.to_owned(),
            x,
            y,
            width,
            height,
            primary: false,
        }
    }

    #[test]
    fn desktop_bounds_span_every_monitor() {
        let monitors = [
            monitor("DP-1", 0, 0, 1920, 1080),
            monitor("HDMI-1", -1280, 120, 1280, 1024),
        ];

        let bounds = desktop_bounds(&monitors);

        assert_eq!((bounds.left, bounds.top), (-1280, 0));
        assert_eq!((bounds.width, bounds.height), (3200, 1144));
    }

    #[test]
    fn empty_desktops_never_produce_a_zero_extent() {
        let bounds = desktop_bounds(&[]);

        assert_eq!((bounds.width, bounds.height), (1, 1));
        assert!((normalized(0, bounds.width) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn normalized_extents_stay_inside_the_map() {
        assert!((normalized(960, 1920) - 0.5).abs() < f32::EPSILON);
        assert!((normalized(-10, 1920) - 0.0).abs() < f32::EPSILON);
        assert!((normalized(4000, 1920) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn status_line_lists_the_detected_displays() {
        assert_eq!(status_line(&[]), "");
        assert_eq!(
            status_line(&[monitor("DP-1", 0, 0, 1920, 1080)]),
            "1 detected: DP-1"
        );
    }
}
