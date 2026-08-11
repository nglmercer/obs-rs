//! Bounded, deterministic diagnostics and recovery bundles for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

mod bundle;
mod cursor;
mod error;
mod redaction;
mod types;
mod writer;

#[cfg(test)]
mod tests;

pub use bundle::DiagnosticBundle;
pub use error::DiagnosticError;
pub use redaction::{redact_diagnostics_text, Redacted, REDACTED};
pub use types::{
    DiagnosticFileState, DIAGNOSTIC_MAGIC, MAX_BUNDLE_BYTES, MAX_SECTIONS, MAX_SECTION_BYTES,
    MAX_SECTION_NAME_BYTES,
};
pub use writer::AtomicDiagnosticFileWriter;
