//! Toolkit-neutral desktop state and commands for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

pub const MAX_UI_NOTICES: usize = 256;
pub const MAX_SHORTCUT_KEY_BYTES: usize = 32;
pub const MAX_CONSOLE_COMMAND_BYTES: usize = 256;
pub const MAX_WEB_REQUEST_BYTES: usize = 64 * 1024;

mod commands;
mod console;
mod error;
mod helpers;
mod snapshot;
mod state;
mod types;
mod web;

#[cfg(test)]
mod tests;

pub use console::{parse_console_command, ConsoleCommand, ConsoleCommandError};
pub use error::UiError;
pub use state::DesktopState;
pub use types::{MixerChannel, SceneView, Shortcut, UiAction, UiCommand, UiLocale, UiNotice};
pub use web::{parse_web_request, WebRequestError, WebRoute};
