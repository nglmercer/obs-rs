use std::fmt;

use super::types::DiagnosticFileState;
/// Errors raised while building, decoding, or committing a diagnostics bundle.
#[derive(Debug, Eq, PartialEq)]
pub enum DiagnosticError {
    /// A section name is empty, oversized, or contains unsupported characters.
    InvalidSectionName,
    /// A section name already exists in the bundle.
    DuplicateSection(String),
    /// A section exceeds [`crate::MAX_SECTION_BYTES`].
    SectionTooLarge { bytes: usize },
    /// The complete bundle exceeds [`crate::MAX_BUNDLE_BYTES`].
    BundleTooLarge { bytes: usize },
    /// The serialized input has an invalid header.
    InvalidHeader,
    /// The serialized input ended before a declared field or payload.
    Truncated,
    /// The serialized input contains bytes after its final section.
    TrailingBytes,
    /// A serialized section name is not valid UTF-8 or violates name rules.
    InvalidEncodedName,
    /// The file writer received an empty or aliased path pair.
    InvalidPaths,
    /// The file writer was used after finalization or abort.
    InvalidState {
        /// Operation attempted against a closed writer.
        operation: &'static str,
        /// Current writer state.
        state: DiagnosticFileState,
    },
    /// A filesystem operation failed.
    Io { operation: String, message: String },
}

impl fmt::Display for DiagnosticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSectionName => formatter.write_str("invalid diagnostics section name"),
            Self::DuplicateSection(name) => {
                write!(formatter, "diagnostics section {name} is duplicated")
            }
            Self::SectionTooLarge { bytes } => {
                write!(formatter, "diagnostics section is too large: {bytes} bytes")
            }
            Self::BundleTooLarge { bytes } => {
                write!(formatter, "diagnostics bundle is too large: {bytes} bytes")
            }
            Self::InvalidHeader => formatter.write_str("invalid diagnostics bundle header"),
            Self::Truncated => formatter.write_str("truncated diagnostics bundle"),
            Self::TrailingBytes => formatter.write_str("diagnostics bundle has trailing bytes"),
            Self::InvalidEncodedName => {
                formatter.write_str("diagnostics bundle contains an invalid section name")
            }
            Self::InvalidPaths => {
                formatter.write_str("diagnostics paths must be non-empty and distinct")
            }
            Self::InvalidState { operation, state } => {
                write!(
                    formatter,
                    "cannot {operation} diagnostics writer in state {state:?}"
                )
            }
            Self::Io { operation, message } => {
                write!(formatter, "diagnostics {operation}: {message}")
            }
        }
    }
}

impl std::error::Error for DiagnosticError {}
