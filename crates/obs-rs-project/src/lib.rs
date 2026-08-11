//! Rust-owned project state and deterministic persistence for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

pub(crate) const MAGIC: &str = "OBSRPROJECT1";
pub const MAX_PROJECT_BYTES: usize = 1_048_576;

mod codec;
mod commands;
mod error;
mod model;
mod persistence;
mod session;
mod validation;

#[cfg(test)]
mod tests;

pub use commands::ProjectCommand;
pub use error::ProjectError;
pub use model::{Profile, Project, SceneSpec, SourceSpec};
pub use persistence::ProjectFileStore;
pub use session::{ProjectSession, MAX_HISTORY_DEPTH};
