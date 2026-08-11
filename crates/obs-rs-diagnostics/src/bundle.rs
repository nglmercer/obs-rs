use std::collections::BTreeMap;

use super::{
    cursor::Cursor,
    error::DiagnosticError,
    types::{
        DIAGNOSTIC_MAGIC, MAX_BUNDLE_BYTES, MAX_SECTIONS, MAX_SECTION_BYTES, MAX_SECTION_NAME_BYTES,
    },
};
/// A deterministic bundle of bounded named diagnostic sections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticBundle {
    sections: BTreeMap<String, Vec<u8>>,
    /// Running encoded length, kept in step by every insertion.
    ///
    /// Sections are append-only, so the exact encoded size can be maintained
    /// incrementally instead of walking the whole map on each insert and on
    /// each size query.
    encoded_len: usize,
}

impl Default for DiagnosticBundle {
    fn default() -> Self {
        Self {
            sections: BTreeMap::new(),
            encoded_len: HEADER_BYTES,
        }
    }
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
        let candidate = self
            .encoded_len
            .checked_add(section_bytes(name.len(), bytes.len())?)
            .ok_or(DiagnosticError::BundleTooLarge { bytes: usize::MAX })?;
        if candidate > MAX_BUNDLE_BYTES {
            return Err(DiagnosticError::BundleTooLarge { bytes: candidate });
        }

        self.sections.insert(name.to_owned(), bytes.to_vec());
        self.encoded_len = candidate;
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
    ///
    /// Reads the running total rather than re-walking every section.
    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        self.encoded_len
    }

    /// Encodes the bundle into the versioned deterministic binary format.
    ///
    /// # Errors
    ///
    /// Returns [`DiagnosticError::BundleTooLarge`] only if the bundle was built
    /// with invalid state outside this API's normal insertion path.
    pub fn encode(&self) -> Result<Vec<u8>, DiagnosticError> {
        let size = self.encoded_len;
        if size > MAX_BUNDLE_BYTES {
            return Err(DiagnosticError::BundleTooLarge { bytes: size });
        }
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

/// Bytes the encoded form spends before the first section.
const HEADER_BYTES: usize = DIAGNOSTIC_MAGIC.len() + 4;

/// Encoded bytes contributed by one section: a 2-byte name length, an 8-byte
/// payload length, then the name and payload themselves.
fn section_bytes(name_len: usize, payload_len: usize) -> Result<usize, DiagnosticError> {
    2_usize
        .checked_add(8)
        .and_then(|value| value.checked_add(name_len))
        .and_then(|value| value.checked_add(payload_len))
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
