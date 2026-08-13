//! Safe, deterministic settings used by the OBS-RS control plane.
//!
//! Documents are stored as flat [TOML] tables: one `key = value` pair per line,
//! no nested tables or arrays. Values are modelled as strings regardless of how
//! they are spelled in the document, which keeps the settings surface uniform
//! for the plugin and property layers that consume it.
//!
//! The supported subset is deliberately narrow so that parsing stays bounded
//! and allocation-predictable:
//!
//! * bare keys drawn from the shared ASCII identifier alphabet;
//! * basic strings (`"…"`) with the standard TOML escapes, including `\uXXXX`;
//! * literal strings (`'…'`), which take no escapes;
//! * bare integers and booleans, retained as their literal text.
//!
//! Every document this crate writes is valid TOML and re-reads identically
//! through any conforming TOML parser. Inline tables, arrays, dates, floats,
//! multi-line strings, and `[table]` headers are rejected rather than silently
//! misread.
//!
//! [TOML]: https://toml.io

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{collections::BTreeMap, fmt};

use obs_rs_util::{validate_identifier, IdentifierError};

/// Maximum encoded size accepted by [`Config::parse`].
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;

/// Maximum encoded size accepted for one configuration value.
pub const MAX_VALUE_BYTES: usize = 4096;

/// A validated, deterministic key/value settings document.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Config {
    entries: BTreeMap<String, String>,
}

impl Config {
    /// Creates an empty settings document.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses a flat TOML document with one `key = value` entry per line.
    ///
    /// Empty lines and lines beginning with `#` are ignored, as is a trailing
    /// comment after a value. Keys use the shared ASCII identifier policy.
    /// Whitespace around the key, the `=`, and the value is insignificant.
    /// Duplicate keys are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the document is too large, a line is
    /// malformed, a key or value is invalid, or a key is repeated.
    pub fn parse(document: &str) -> Result<Self, ConfigError> {
        if document.len() > MAX_CONFIG_BYTES {
            return Err(ConfigError::InputTooLarge);
        }

        let mut config = Self::new();
        for (index, line) in document.lines().enumerate() {
            let line_number = index + 1;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // `[table]` headers are the one TOML construct likely to appear in a
            // hand-edited file, so it is named explicitly instead of failing as
            // a generic malformed line.
            if trimmed.starts_with('[') {
                return Err(ConfigError::UnsupportedConstruct { line: line_number });
            }

            let Some((key, value)) = trimmed.split_once('=') else {
                return Err(ConfigError::InvalidLine { line: line_number });
            };

            let value = parse_value(value.trim(), line_number)?;
            config.insert(key.trim(), &value, line_number)?;
        }

        Ok(config)
    }

    /// Inserts or replaces a validated setting.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when `key` violates the identifier policy or `value`
    /// contains a NUL byte or exceeds [`MAX_VALUE_BYTES`].
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), ConfigError> {
        Self::validate_entry(key, value, 0)?;
        self.entries.insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    /// Removes a setting and returns its previous value.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.entries.remove(key)
    }

    /// Returns a borrowed setting value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// Iterates over settings in sorted key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// Returns the number of stored settings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the document contains no settings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serializes settings as flat TOML in sorted key order.
    ///
    /// Values that are already canonical TOML integers or booleans are written
    /// bare; everything else is written as a basic string. The result re-parses
    /// to an identical [`Config`], so `serialize` is a fixed point after one
    /// round trip.
    #[must_use]
    pub fn serialize(&self) -> String {
        // Reserve the exact unescaped length plus the ` = ""\n` framing. Values
        // needing escapes are the rare case and cost at most one growth, which
        // is why the reservation is a lower bound rather than a second pass.
        let capacity = self.entries.iter().fold(0_usize, |total, (key, value)| {
            total.saturating_add(key.len().saturating_add(value.len()).saturating_add(6))
        });
        let mut document = String::with_capacity(capacity);
        for (key, value) in &self.entries {
            document.push_str(key);
            document.push_str(" = ");
            if is_bare_literal(value) {
                document.push_str(value);
            } else {
                push_basic_string(&mut document, value);
            }
            document.push('\n');
        }
        document
    }

    fn insert(&mut self, key: &str, value: &str, line: usize) -> Result<(), ConfigError> {
        Self::validate_entry(key, value, line)?;
        if self.entries.contains_key(key) {
            return Err(ConfigError::DuplicateKey { line });
        }

        self.entries.insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn validate_entry(key: &str, value: &str, line: usize) -> Result<(), ConfigError> {
        validate_identifier(key.as_bytes())
            .map_err(|error| ConfigError::InvalidKey { line, error })?;

        if memchr::memchr(0, value.as_bytes()).is_some() {
            return Err(ConfigError::InvalidValue { line });
        }

        if value.len() > MAX_VALUE_BYTES {
            return Err(ConfigError::ValueTooLong { line });
        }

        Ok(())
    }
}

/// Returns whether `value` can be written without quotes and read back intact.
///
/// Only canonical spellings qualify: TOML rejects integers with leading zeros,
/// and a value such as `007` must therefore stay quoted to round-trip.
fn is_bare_literal(value: &str) -> bool {
    if value == "true" || value == "false" {
        return true;
    }

    let digits = value.strip_prefix('-').unwrap_or(value);
    match digits.as_bytes() {
        [] => false,
        [b'0'] => true,
        [first, rest @ ..] => {
            first.is_ascii_digit()
                && *first != b'0'
                && rest.iter().all(u8::is_ascii_digit)
                // A bare integer must fit TOML's 64-bit range; anything longer
                // is data that happens to look numeric, so it stays a string.
                && value.parse::<i64>().is_ok()
        }
    }
}

/// Appends `value` as a TOML basic string, escaping what the grammar requires.
fn push_basic_string(document: &mut String, value: &str) {
    document.push('"');
    for character in value.chars() {
        match character {
            '"' => document.push_str("\\\""),
            '\\' => document.push_str("\\\\"),
            '\u{8}' => document.push_str("\\b"),
            '\t' => document.push_str("\\t"),
            '\n' => document.push_str("\\n"),
            '\u{c}' => document.push_str("\\f"),
            '\r' => document.push_str("\\r"),
            // TOML forbids raw control characters inside a basic string; the
            // rest of Unicode is allowed verbatim and stays human-readable.
            control if control.is_control() => {
                let _ = fmt::Write::write_fmt(document, format_args!("\\u{:04X}", control as u32));
            }
            other => document.push(other),
        }
    }
    document.push('"');
}

/// Decodes one TOML value into the string form [`Config`] stores.
fn parse_value(value: &str, line: usize) -> Result<String, ConfigError> {
    match value.as_bytes().first() {
        None => Err(ConfigError::InvalidLine { line }),
        Some(b'"') => parse_basic_string(value, line),
        Some(b'\'') => parse_literal_string(value, line),
        // Anything unquoted is either a bare literal this crate emits or a TOML
        // type outside the supported subset.
        Some(_) => {
            let bare = strip_comment(value);
            if is_bare_literal(bare) {
                Ok(bare.to_owned())
            } else {
                Err(ConfigError::UnsupportedConstruct { line })
            }
        }
    }
}

/// Decodes a quoted basic string and rejects trailing junk after it.
fn parse_basic_string(value: &str, line: usize) -> Result<String, ConfigError> {
    let mut decoded = String::with_capacity(value.len());
    let mut characters = value.char_indices();
    let _opening_quote = characters.next();

    while let Some((offset, character)) = characters.next() {
        match character {
            '"' => {
                let rest = value.get(offset + 1..).unwrap_or_default();
                return if strip_comment(rest).is_empty() {
                    Ok(decoded)
                } else {
                    Err(ConfigError::InvalidValue { line })
                };
            }
            '\\' => {
                let (_, escape) = characters
                    .next()
                    .ok_or(ConfigError::InvalidValue { line })?;
                match escape {
                    '"' => decoded.push('"'),
                    '\\' => decoded.push('\\'),
                    'b' => decoded.push('\u{8}'),
                    't' => decoded.push('\t'),
                    'n' => decoded.push('\n'),
                    'f' => decoded.push('\u{c}'),
                    'r' => decoded.push('\r'),
                    'u' => decoded.push(parse_escape(&mut characters, 4, line)?),
                    'U' => decoded.push(parse_escape(&mut characters, 8, line)?),
                    _ => return Err(ConfigError::InvalidValue { line }),
                }
            }
            control if control.is_control() => return Err(ConfigError::InvalidValue { line }),
            other => decoded.push(other),
        }
    }

    Err(ConfigError::InvalidValue { line })
}

/// Reads `width` hex digits and converts them to one scalar value.
fn parse_escape(
    characters: &mut std::str::CharIndices<'_>,
    width: usize,
    line: usize,
) -> Result<char, ConfigError> {
    let mut code = 0_u32;
    for _ in 0..width {
        let (_, digit) = characters
            .next()
            .ok_or(ConfigError::InvalidValue { line })?;
        let digit = digit
            .to_digit(16)
            .ok_or(ConfigError::InvalidValue { line })?;
        // Four or eight hex digits cannot overflow u32, so the shift is exact.
        code = code * 16 + digit;
    }

    char::from_u32(code).ok_or(ConfigError::InvalidValue { line })
}

/// Decodes a single-quoted literal string, which has no escape sequences.
fn parse_literal_string(value: &str, line: usize) -> Result<String, ConfigError> {
    let body = value.get(1..).unwrap_or_default();
    let Some(end) = body.find('\'') else {
        return Err(ConfigError::InvalidValue { line });
    };

    let (decoded, rest) = body.split_at(end);
    if decoded.chars().any(char::is_control) {
        return Err(ConfigError::InvalidValue { line });
    }
    if !strip_comment(rest.get(1..).unwrap_or_default()).is_empty() {
        return Err(ConfigError::InvalidValue { line });
    }

    Ok(decoded.to_owned())
}

/// Trims a trailing `#` comment and surrounding whitespace.
///
/// Only safe on input already known to sit outside a string literal, which is
/// why quoted values are decoded before their remainder reaches this.
fn strip_comment(input: &str) -> &str {
    match input.split_once('#') {
        Some((before, _)) => before.trim(),
        None => input.trim(),
    }
}

/// Errors produced while parsing or mutating [`Config`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    /// The complete document exceeds [`MAX_CONFIG_BYTES`].
    InputTooLarge,
    /// A line does not contain a key/value separator.
    InvalidLine {
        /// One-based line number, or zero for programmatic updates.
        line: usize,
    },
    /// A key does not satisfy the shared identifier policy.
    InvalidKey {
        /// One-based line number, or zero for programmatic updates.
        line: usize,
        /// The specific identifier validation failure.
        error: IdentifierError,
    },
    /// A document contains the same key more than once.
    DuplicateKey {
        /// One-based line number of the duplicate.
        line: usize,
    },
    /// A value is not a well-formed literal of the supported subset.
    InvalidValue {
        /// One-based line number, or zero for programmatic updates.
        line: usize,
    },
    /// A value exceeds [`MAX_VALUE_BYTES`].
    ValueTooLong {
        /// One-based line number, or zero for programmatic updates.
        line: usize,
    },
    /// A line uses a TOML construct outside the flat string-table subset.
    UnsupportedConstruct {
        /// One-based line number.
        line: usize,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge => formatter.write_str("configuration document is too large"),
            Self::InvalidLine { line } => write!(formatter, "invalid configuration line {line}"),
            Self::InvalidKey { line, error } => {
                write!(
                    formatter,
                    "invalid configuration key on line {line}: {error}"
                )
            }
            Self::DuplicateKey { line } => {
                write!(formatter, "duplicate configuration key on line {line}")
            }
            Self::InvalidValue { line } => {
                write!(formatter, "invalid configuration value on line {line}")
            }
            Self::ValueTooLong { line } => {
                write!(formatter, "configuration value on line {line} is too long")
            }
            Self::UnsupportedConstruct { line } => {
                write!(
                    formatter,
                    "unsupported TOML construct on configuration line {line}"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests;
