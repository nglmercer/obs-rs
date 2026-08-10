//! Portable render-backend contracts with a deterministic CPU fallback.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod backend;
mod cpu;
mod error;
mod types;

#[cfg(test)]
mod tests;

pub use backend::RenderBackend;
pub use cpu::CpuRenderBackend;
pub use error::RenderError;
pub use types::{RenderCapabilities, RenderMetrics, RenderState, TextureId};

pub const DEFAULT_MAX_TEXTURE_BYTES: usize = 512 * 1024 * 1024;
