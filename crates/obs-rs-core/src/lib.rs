//! The headless OBS-RS runtime and its reference scene compositor.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod compositor;
mod error;
mod ids;
mod limits;
mod metrics;
mod registry;
mod runtime;

#[cfg(test)]
mod tests;

pub use compositor::RenderedSceneLayer;
pub use error::RuntimeError;
pub use ids::SourceId;
pub use limits::{RuntimeLimits, RuntimeUsage};
pub use metrics::CompositorMetrics;
pub use runtime::Runtime;
