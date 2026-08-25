//! Toolkit-neutral desktop state and commands for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

pub const MAX_UI_NOTICES: usize = 256;
pub const MAX_SHORTCUT_KEY_BYTES: usize = 32;
pub const MAX_SHORTCUT_TEXT_BYTES: usize = 64;
pub const MAX_SHORTCUT_BINDINGS: usize = 64;
pub const MAX_CONSOLE_COMMAND_BYTES: usize = 256;
pub const MAX_WEB_REQUEST_BYTES: usize = 64 * 1024;
pub use obs_rs_media::{MAX_TRANSITION_DURATION_MILLIS, MIN_TRANSITION_DURATION_MILLIS};
/// Default scene-transition duration in milliseconds.
pub const DEFAULT_TRANSITION_DURATION_MILLIS: u32 =
    obs_rs_media::DEFAULT_TRANSITION_DURATION_MILLIS;
/// Maximum number of scene items a desktop canvas selection retains.
///
/// Selection is transient UI state, but it still crosses frontend boundaries
/// and must not become an unbounded allocation sink when a project contains a
/// very large scene.
pub const MAX_CANVAS_SELECTIONS: usize = 256;

mod commands;
mod console;
mod error;
mod helpers;
mod snapshot;
mod state;
mod stinger_loader;
mod types;
mod web;

#[cfg(test)]
mod tests;

pub use console::{parse_console_command, ConsoleCommand, ConsoleCommandError};
pub use error::UiError;
pub use state::DesktopState;
pub use stinger_loader::{StingerLoadSession, StingerLoadState};
pub use types::{
    MixerChannel, ProjectSceneSelection, SceneView, Shortcut, StingerSnapshot, TransitionSnapshot,
    UiAction, UiCommand, UiLocale, UiNotice,
};
pub use web::{parse_web_request, WebRequestError, WebRoute};
