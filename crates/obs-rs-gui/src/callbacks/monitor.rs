//! Controller for the display picker.
//!
//! A multi-head desktop is a single X11 screen, so "screen capture" without a
//! chosen display grabs every monitor at once. This window is what turns that
//! into an explicit choice; the selection is stored in the source's `monitor`
//! setting and therefore travels with the project file.

use std::{cell::RefCell, rc::Rc};

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

use obs_rs_config::Config;
use obs_rs_ui::DesktopState;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    apply_source_settings_and_refresh,
    fixtures::{kind_selects_monitor, screen_monitors, MonitorChoice},
    I18n, MainWindow, MonitorRow, MonitorWindow, Palette, PreviewRenderer,
};

/// Only one compositor picker may be active at a time. A portal request is a
/// separate D-Bus session, so repeated UI callbacks otherwise create several
/// identical dialogs stacked on top of each other.
#[cfg(target_os = "linux")]
static PORTAL_REQUEST_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "linux")]
struct PortalRequestGuard;

#[cfg(target_os = "linux")]
impl Drop for PortalRequestGuard {
    fn drop(&mut self) {
        PORTAL_REQUEST_IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// Owns the picker window and the draft selection it edits.
pub(crate) struct MonitorController {
    window: MonitorWindow,
    monitors: RefCell<Vec<MonitorChoice>>,
    /// The display the user has highlighted; empty means none is highlighted.
    selected: RefCell<String>,
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

/// The rectangle covering every monitor, used to normalize the layout map.
struct DesktopBounds {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

fn desktop_bounds(monitors: &[MonitorChoice]) -> DesktopBounds {
    let left = monitors.iter().map(|monitor| monitor.x).min().unwrap_or(0);
    let top = monitors.iter().map(|monitor| monitor.y).min().unwrap_or(0);
    let right = monitors
        .iter()
        .map(|monitor| {
            monitor
                .x
                .saturating_add(i32::try_from(monitor.width).unwrap_or(i32::MAX))
        })
        .max()
        .unwrap_or(1);
    let bottom = monitors
        .iter()
        .map(|monitor| {
            monitor
                .y
                .saturating_add(i32::try_from(monitor.height).unwrap_or(i32::MAX))
        })
        .max()
        .unwrap_or(1);
    DesktopBounds {
        left,
        top,
        // A zero extent would divide by zero in the map; one pixel is the
        // smallest value that keeps every rectangle finite.
        width: (right - left).max(1),
        height: (bottom - top).max(1),
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
    renderer: &Rc<RefCell<PreviewRenderer>>,
) -> Result<Rc<MonitorController>, slint::PlatformError> {
    let controller = Rc::new(MonitorController {
        window: MonitorWindow::new()?,
        monitors: RefCell::new(Vec::new()),
        selected: RefCell::new(String::new()),
    });

    install_open(ui, state, &controller);
    #[cfg(target_os = "linux")]
    install_portal_token(ui, state, renderer);
    install_selection(state, &controller);
    install_commit(ui, state, renderer, &controller);
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
        let locale = state.borrow().locale();
        let selected = ui.get_selected_source().to_string();
        let kind = selected_source_kind(&state, &selected);
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
            share_through_portal(&ui, &state, &selected);
            return;
        }
        let window = &controller.window;
        window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(locale));
        controller.set_tokens(ui.global::<Palette>().get_tokens());
        controller.reload(&selected, current_monitor(&state, &selected).as_deref());
        window.set_status(status_line(&controller.monitors.borrow()).into());
        if let Err(error) = window.show() {
            ui.set_status_message(format!("Display picker: {error}").into());
        }
    });
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
        let source = window.get_source_name().to_string();
        let current = current_monitor(&refresh_state, &source);
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
    renderer: &Rc<RefCell<PreviewRenderer>>,
    controller: &Rc<MonitorController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    let accept_controller = Rc::clone(controller);
    controller.window.on_accept_monitor(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let window = &accept_controller.window;
        let source = window.get_source_name().to_string();
        let monitor = if window.get_capture_whole_desktop() {
            String::new()
        } else {
            accept_controller.selected.borrow().clone()
        };
        let Some(document) = monitor_document(&state, &source, &monitor) else {
            ui.set_status_message("Display selection failed: source settings are invalid".into());
            return;
        };
        // The shared apply path validates the document, writes it through the
        // project command, and rebuilds the renderer, so a display change is
        // recorded as an ordinary undoable project edit.
        apply_source_settings_and_refresh(&ui, &state, &renderer, &document);
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
fn share_through_portal(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>, source: &str) {
    use obs_rs_capture::{open_screencast, CursorMode};

    if PORTAL_REQUEST_IN_FLIGHT.swap(true, Ordering::AcqRel) {
        let waiting = crate::i18n::with_catalog(state.borrow().locale(), |text| {
            text.monitor_ui.portal_waiting.clone()
        });
        ui.set_status_message(waiting);
        return;
    }

    let source = source.to_owned();
    let cursor = source_settings_document(state, &source)
        .as_deref()
        .and_then(|document| Config::parse(document).ok())
        .and_then(|settings| settings.get("capture_cursor").map(str::to_owned))
        .is_none_or(|value| value.trim() != "false");
    let waiting = crate::i18n::with_catalog(state.borrow().locale(), |text| {
        text.monitor_ui.portal_waiting.clone()
    });
    ui.set_status_message(waiting);

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
                // The token outlives this session, so it is read before the
                // session is dropped and the compositor's stream stops.
                Ok(session) => session.restore_token().map_or_else(
                    || Err("the compositor shared a screen but would not remember it".to_owned()),
                    |token| Ok(token.to_owned()),
                ),
                Err(error) => Err(error.to_string()),
            };
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                match outcome {
                    Ok(token) => ui.invoke_apply_portal_token(source.into(), token.into()),
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

/// Stores a portal token on the selected source, on the UI thread.
#[cfg(target_os = "linux")]
fn install_portal_token(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    ui.on_apply_portal_token(move |source, token| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let source = source.to_string();
        let document = source_settings_document(&state, &source)
            .as_deref()
            .and_then(|document| Config::parse(document).ok());
        let Some(mut settings) = document else {
            ui.set_status_message("Screen sharing failed: source settings are invalid".into());
            return;
        };
        if settings.set("restore_token", token.as_str()).is_err() {
            ui.set_status_message("Screen sharing failed: the token could not be stored".into());
            return;
        }
        apply_source_settings_and_refresh(&ui, &state, &renderer, &settings.serialize());
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
    source: &str,
    monitor: &str,
) -> Option<String> {
    let mut settings = source_settings_document(state, source)
        .as_deref()
        .and_then(|document| Config::parse(document).ok())?;
    if let Ok(display) = std::env::var("DISPLAY") {
        let _ = settings.set("display", &display);
    }
    settings.set("monitor", monitor).ok()?;
    Some(settings.serialize())
}

/// Returns the selected source's settings document from the live project.
fn source_settings_document(state: &Rc<RefCell<DesktopState>>, source: &str) -> Option<String> {
    let state = state.borrow();
    let scene = state.preview_scene()?.to_owned();
    let session = state.project_session();
    let project = session.project();
    let profile = project.active_profile_spec()?;
    let item = profile.scene(scene.as_str())?.item(source)?;
    Some(profile.source(item.source_id())?.settings().serialize())
}

/// Reads the display a source is currently pointed at.
///
/// `Some("")` means the source explicitly captures the whole desktop, while
/// `None` means it has never been configured.
pub(crate) fn current_monitor(state: &Rc<RefCell<DesktopState>>, source: &str) -> Option<String> {
    let document = source_settings_document(state, source)?;
    let settings = Config::parse(&document).ok()?;
    settings.get("monitor").map(str::to_owned)
}

/// Returns the kind of the selected source in the preview scene.
pub(crate) fn selected_source_kind(state: &Rc<RefCell<DesktopState>>, source: &str) -> String {
    let state = state.borrow();
    let Some(scene) = state.preview_scene().map(str::to_owned) else {
        return String::new();
    };
    let session = state.project_session();
    let project = session.project();
    let Some(profile) = project.active_profile_spec() else {
        return String::new();
    };
    let Some(item) = profile
        .scene(scene.as_str())
        .and_then(|scene| scene.item(source))
    else {
        return String::new();
    };
    profile
        .source(item.source_id())
        .map_or_else(String::new, |source| source.kind().as_str().to_owned())
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
