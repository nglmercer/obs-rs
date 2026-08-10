/// Magic and format version for an OBS-RS diagnostics bundle.
pub const DIAGNOSTIC_MAGIC: &[u8; 8] = b"OBSRDG01";
/// Maximum number of named sections in one bundle.
pub const MAX_SECTIONS: usize = 128;
/// Maximum UTF-8 byte length of one section name.
pub const MAX_SECTION_NAME_BYTES: usize = 64;
/// Maximum payload size of one section.
pub const MAX_SECTION_BYTES: usize = 1 << 20;
/// Maximum encoded size of a complete bundle.
pub const MAX_BUNDLE_BYTES: usize = 4 << 20;

/// Lifecycle state of an atomic diagnostics file writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticFileState {
    /// The writer accepts one finalization or abort operation.
    Open,
    /// The encoded bundle was synchronized and renamed into place.
    Finalized,
    /// The writer was cancelled and its temporary artifact was removed.
    Aborted,
}
