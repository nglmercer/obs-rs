//! Bounded, deterministic diagnostics and recovery bundles for OBS-RS.
//!
//! The bundle format is intentionally small and self-describing enough for a
//! support tool to validate without loading the engine. It contains only Rust-owned
//! bytes and has explicit limits so a faulty producer cannot grow a diagnostics
//! artifact without bound.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

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

/// Errors raised while building, decoding, or committing a diagnostics bundle.
#[derive(Debug, Eq, PartialEq)]
pub enum DiagnosticError {
    /// A section name is empty, oversized, or contains unsupported characters.
    InvalidSectionName,
    /// A section name already exists in the bundle.
    DuplicateSection(String),
    /// A section exceeds [`MAX_SECTION_BYTES`].
    SectionTooLarge { bytes: usize },
    /// The complete bundle exceeds [`MAX_BUNDLE_BYTES`].
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

/// A deterministic bundle of bounded named diagnostic sections.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticBundle {
    sections: BTreeMap<String, Vec<u8>>,
}

impl DiagnosticBundle {
    /// Creates an empty diagnostics bundle.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a UTF-8 section after validating its name and size.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is invalid or duplicated, or when the
    /// section or resulting bundle exceeds its configured bound.
    pub fn insert_text(&mut self, name: &str, text: &str) -> Result<(), DiagnosticError> {
        self.insert_bytes(name, text.as_bytes())
    }

    /// Inserts an opaque section after validating its name and size.
    ///
    /// Sections are emitted in bytewise name order, regardless of insertion order.
    /// A failed insertion leaves the bundle unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is invalid or duplicated, or when the
    /// section or resulting bundle exceeds its configured bound.
    pub fn insert_bytes(&mut self, name: &str, bytes: &[u8]) -> Result<(), DiagnosticError> {
        validate_name(name)?;
        if bytes.len() > MAX_SECTION_BYTES {
            return Err(DiagnosticError::SectionTooLarge { bytes: bytes.len() });
        }
        if self.sections.contains_key(name) {
            return Err(DiagnosticError::DuplicateSection(name.to_owned()));
        }

        let candidate_count = self
            .sections
            .len()
            .checked_add(1)
            .ok_or(DiagnosticError::BundleTooLarge { bytes: usize::MAX })?;
        if candidate_count > MAX_SECTIONS {
            return Err(DiagnosticError::BundleTooLarge {
                bytes: candidate_count,
            });
        }
        let candidate = encoded_size_with_extra(&self.sections, name, bytes.len())?;
        if candidate > MAX_BUNDLE_BYTES {
            return Err(DiagnosticError::BundleTooLarge { bytes: candidate });
        }

        self.sections.insert(name.to_owned(), bytes.to_vec());
        Ok(())
    }

    /// Returns the number of sections in the bundle.
    #[must_use]
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Returns one section by name without copying its payload.
    #[must_use]
    pub fn section(&self, name: &str) -> Option<&[u8]> {
        self.sections.get(name).map(Vec::as_slice)
    }

    /// Iterates sections in deterministic name order.
    pub fn sections(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.sections
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
    }

    /// Returns the exact encoded byte length.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        encoded_size(&self.sections).unwrap_or(MAX_BUNDLE_BYTES)
    }

    /// Encodes the bundle into the versioned deterministic binary format.
    ///
    /// # Errors
    ///
    /// Returns [`DiagnosticError::BundleTooLarge`] only if the bundle was built
    /// with invalid state outside this API's normal insertion path.
    pub fn encode(&self) -> Result<Vec<u8>, DiagnosticError> {
        let size = encoded_size(&self.sections)?;
        let mut output = Vec::with_capacity(size);
        output.extend_from_slice(DIAGNOSTIC_MAGIC);
        write_u32(&mut output, self.sections.len())?;
        for (name, bytes) in &self.sections {
            write_u16(
                &mut output,
                u16::try_from(name.len()).map_err(|_| DiagnosticError::InvalidSectionName)?,
            );
            write_u64(
                &mut output,
                u64::try_from(bytes.len())
                    .map_err(|_| DiagnosticError::SectionTooLarge { bytes: bytes.len() })?,
            );
            output.extend_from_slice(name.as_bytes());
            output.extend_from_slice(bytes);
        }
        Ok(output)
    }

    /// Decodes and validates a serialized diagnostics bundle.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid header, malformed or oversized section,
    /// truncation, duplicate names, or trailing bytes.
    pub fn decode(input: &[u8]) -> Result<Self, DiagnosticError> {
        if input.len() > MAX_BUNDLE_BYTES {
            return Err(DiagnosticError::BundleTooLarge { bytes: input.len() });
        }
        let mut cursor = Cursor::new(input);
        if cursor.take(DIAGNOSTIC_MAGIC.len())? != DIAGNOSTIC_MAGIC {
            return Err(DiagnosticError::InvalidHeader);
        }
        let count = usize::try_from(cursor.u32()?)
            .map_err(|_| DiagnosticError::BundleTooLarge { bytes: usize::MAX })?;
        if count > MAX_SECTIONS {
            return Err(DiagnosticError::BundleTooLarge { bytes: count });
        }

        let mut bundle = Self::new();
        for _ in 0..count {
            let name_len = usize::from(cursor.u16()?);
            if name_len == 0 || name_len > MAX_SECTION_NAME_BYTES {
                return Err(DiagnosticError::InvalidEncodedName);
            }
            let payload_len = usize::try_from(cursor.u64()?)
                .map_err(|_| DiagnosticError::SectionTooLarge { bytes: usize::MAX })?;
            if payload_len > MAX_SECTION_BYTES {
                return Err(DiagnosticError::SectionTooLarge { bytes: payload_len });
            }
            let name_bytes = cursor.take(name_len)?;
            let name =
                std::str::from_utf8(name_bytes).map_err(|_| DiagnosticError::InvalidEncodedName)?;
            validate_name(name).map_err(|_| DiagnosticError::InvalidEncodedName)?;
            let payload = cursor.take(payload_len)?;
            bundle.insert_bytes(name, payload)?;
        }
        if cursor.remaining() != 0 {
            return Err(DiagnosticError::TrailingBytes);
        }
        Ok(bundle)
    }
}

/// A crash-safe diagnostics writer using temporary-file plus rename finalization.
pub struct AtomicDiagnosticFileWriter {
    final_path: PathBuf,
    temp_path: PathBuf,
    state: DiagnosticFileState,
    committed_bytes: Option<usize>,
}

impl AtomicDiagnosticFileWriter {
    /// Creates an open writer with explicit, distinct final and temporary paths.
    ///
    /// # Errors
    ///
    /// Returns [`DiagnosticError::InvalidPaths`] when either path is empty or the
    /// paths are identical.
    pub fn new(
        final_path: impl Into<PathBuf>,
        temp_path: impl Into<PathBuf>,
    ) -> Result<Self, DiagnosticError> {
        let final_path = final_path.into();
        let temp_path = temp_path.into();
        if final_path.as_os_str().is_empty()
            || temp_path.as_os_str().is_empty()
            || final_path == temp_path
        {
            return Err(DiagnosticError::InvalidPaths);
        }
        Ok(Self {
            final_path,
            temp_path,
            state: DiagnosticFileState::Open,
            committed_bytes: None,
        })
    }

    /// Encodes, synchronizes, and atomically renames one bundle into place.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer is closed, the bundle violates its limits,
    /// or a filesystem operation fails. A failed write removes the temporary file.
    pub fn finalize(&mut self, bundle: &DiagnosticBundle) -> Result<usize, DiagnosticError> {
        self.ensure_open("finalize")?;
        let bytes = bundle.encode()?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.temp_path)
                .map_err(|error| io_error("open temporary file", &error))?;
            file.write_all(&bytes)
                .map_err(|error| io_error("write temporary file", &error))?;
            file.sync_all()
                .map_err(|error| io_error("sync temporary file", &error))?;
            fs::rename(&self.temp_path, &self.final_path)
                .map_err(|error| io_error("rename diagnostics bundle", &error))?;
            Ok::<(), DiagnosticError>(())
        })();

        if let Err(error) = result {
            let _ = fs::remove_file(&self.temp_path);
            return Err(error);
        }
        self.committed_bytes = Some(bytes.len());
        self.state = DiagnosticFileState::Finalized;
        Ok(bytes.len())
    }

    /// Aborts the writer and removes a temporary artifact if present.
    ///
    /// # Errors
    ///
    /// Returns an error when the writer is already closed or the temporary file
    /// cannot be removed.
    pub fn abort(&mut self) -> Result<(), DiagnosticError> {
        self.ensure_open("abort")?;
        match fs::remove_file(&self.temp_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("remove temporary file", &error)),
        }
        self.state = DiagnosticFileState::Aborted;
        Ok(())
    }

    /// Returns the writer lifecycle state.
    #[must_use]
    pub const fn state(&self) -> DiagnosticFileState {
        self.state
    }

    /// Returns the selected final path.
    #[must_use]
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// Returns the selected temporary path.
    #[must_use]
    pub fn temp_path(&self) -> &Path {
        &self.temp_path
    }

    /// Returns the committed encoded byte count after finalization.
    #[must_use]
    pub const fn committed_bytes(&self) -> Option<usize> {
        self.committed_bytes
    }

    fn ensure_open(&self, operation: &'static str) -> Result<(), DiagnosticError> {
        if self.state == DiagnosticFileState::Open {
            Ok(())
        } else {
            Err(DiagnosticError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}

fn validate_name(name: &str) -> Result<(), DiagnosticError> {
    if name.is_empty() || name.len() > MAX_SECTION_NAME_BYTES {
        return Err(DiagnosticError::InvalidSectionName);
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(DiagnosticError::InvalidSectionName);
    }
    Ok(())
}

fn encoded_size(sections: &BTreeMap<String, Vec<u8>>) -> Result<usize, DiagnosticError> {
    let mut size = DIAGNOSTIC_MAGIC
        .len()
        .checked_add(4)
        .ok_or(DiagnosticError::BundleTooLarge { bytes: usize::MAX })?;
    for (name, bytes) in sections {
        size = size
            .checked_add(2)
            .and_then(|value| value.checked_add(8))
            .and_then(|value| value.checked_add(name.len()))
            .and_then(|value| value.checked_add(bytes.len()))
            .ok_or(DiagnosticError::BundleTooLarge { bytes: usize::MAX })?;
    }
    if size > MAX_BUNDLE_BYTES {
        Err(DiagnosticError::BundleTooLarge { bytes: size })
    } else {
        Ok(size)
    }
}

fn encoded_size_with_extra(
    sections: &BTreeMap<String, Vec<u8>>,
    name: &str,
    bytes: usize,
) -> Result<usize, DiagnosticError> {
    let current = encoded_size(sections)?;
    current
        .checked_add(2)
        .and_then(|value| value.checked_add(8))
        .and_then(|value| value.checked_add(name.len()))
        .and_then(|value| value.checked_add(bytes))
        .ok_or(DiagnosticError::BundleTooLarge { bytes: usize::MAX })
}

fn write_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(output: &mut Vec<u8>, value: usize) -> Result<(), DiagnosticError> {
    let value =
        u32::try_from(value).map_err(|_| DiagnosticError::BundleTooLarge { bytes: value })?;
    output.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn io_error(operation: &str, error: &std::io::Error) -> DiagnosticError {
    DiagnosticError::Io {
        operation: operation.to_owned(),
        message: error.to_string(),
    }
}

struct Cursor<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], DiagnosticError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(DiagnosticError::Truncated)?;
        let bytes = self
            .input
            .get(self.offset..end)
            .ok_or(DiagnosticError::Truncated)?;
        self.offset = end;
        Ok(bytes)
    }

    fn u16(&mut self) -> Result<u16, DiagnosticError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, DiagnosticError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, DiagnosticError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn test_paths(label: &str) -> (PathBuf, PathBuf) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir();
        (
            root.join(format!(
                "obs-rs-{label}-{}-{timestamp}-{id}.diag",
                std::process::id()
            )),
            root.join(format!(
                "obs-rs-{label}-{}-{timestamp}-{id}.part",
                std::process::id()
            )),
        )
    }

    #[test]
    fn bundles_are_sorted_bounded_and_round_trip() {
        let mut bundle = DiagnosticBundle::new();
        bundle
            .insert_text("z-runtime", "rendered=3")
            .expect("section");
        bundle
            .insert_bytes("a-project", &[1, 2, 3])
            .expect("section");
        let encoded = bundle.encode().expect("encode");
        let decoded = DiagnosticBundle::decode(&encoded).expect("decode");

        assert_eq!(decoded, bundle);
        assert_eq!(decoded.section_count(), 2);
        assert_eq!(decoded.section("a-project"), Some(&[1, 2, 3][..]));
        assert_eq!(
            decoded.sections().map(|(name, _)| name).collect::<Vec<_>>(),
            vec!["a-project", "z-runtime"]
        );
        assert_eq!(encoded.len(), bundle.encoded_len());
    }

    #[test]
    fn invalid_sections_do_not_mutate_the_bundle() {
        let mut bundle = DiagnosticBundle::new();
        bundle.insert_text("valid", "one").expect("section");
        assert_eq!(
            bundle.insert_text("valid", "two"),
            Err(DiagnosticError::DuplicateSection("valid".to_owned()))
        );
        assert_eq!(
            bundle.insert_text("bad name", "three"),
            Err(DiagnosticError::InvalidSectionName)
        );
        assert_eq!(bundle.section_count(), 1);
        assert_eq!(bundle.section("valid"), Some(b"one".as_slice()));
    }

    #[test]
    fn decoder_rejects_truncation_and_trailing_bytes() {
        let mut bundle = DiagnosticBundle::new();
        bundle.insert_text("runtime", "ok").expect("section");
        let encoded = bundle.encode().expect("encode");
        assert_eq!(
            DiagnosticBundle::decode(&encoded[..encoded.len() - 1]),
            Err(DiagnosticError::Truncated)
        );
        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(
            DiagnosticBundle::decode(&trailing),
            Err(DiagnosticError::TrailingBytes)
        );
    }

    #[test]
    fn atomic_writer_commits_and_abort_removes_temporary_state() {
        let (final_path, temp_path) = test_paths("commit");
        let mut bundle = DiagnosticBundle::new();
        bundle.insert_text("runtime", "ok").expect("section");
        let mut writer = AtomicDiagnosticFileWriter::new(&final_path, &temp_path).expect("writer");
        let committed = writer.finalize(&bundle).expect("commit");
        assert_eq!(writer.state(), DiagnosticFileState::Finalized);
        assert_eq!(writer.committed_bytes(), Some(committed));
        assert_eq!(
            DiagnosticBundle::decode(&fs::read(&final_path).expect("read")),
            Ok(bundle)
        );
        assert!(!temp_path.exists());
        fs::remove_file(final_path).expect("cleanup final");

        let (final_path, temp_path) = test_paths("abort");
        let mut writer = AtomicDiagnosticFileWriter::new(&final_path, &temp_path).expect("writer");
        fs::write(&temp_path, b"uncommitted").expect("temporary fixture");
        writer.abort().expect("abort");
        assert_eq!(writer.state(), DiagnosticFileState::Aborted);
        assert!(!temp_path.exists());
        assert!(!final_path.exists());
    }
}
