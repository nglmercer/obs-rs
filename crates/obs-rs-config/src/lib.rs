//! Safe, deterministic settings used by the OBS-RS control plane.

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

    /// Parses UTF-8 text with one `key=value` entry per line.
    ///
    /// Empty lines and lines beginning with `#` are ignored. Keys use the shared
    /// ASCII identifier policy. Values may contain `=` and any UTF-8 text except
    /// NUL bytes. Parsed duplicate keys are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the document is too large, a line is malformed,
    /// a key or value is invalid, or a key is repeated.
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

            let Some((key, value)) = line.split_once('=') else {
                return Err(ConfigError::InvalidLine { line: line_number });
            };

            config.insert(key.trim(), value, line_number)?;
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

    /// Serializes settings in sorted `key=value\n` order.
    #[must_use]
    pub fn serialize(&self) -> String {
        // Exact reservation: each entry contributes its key, '=', its value,
        // and a newline, so the document never has to grow mid-write.
        let capacity = self.entries.iter().fold(0_usize, |total, (key, value)| {
            total.saturating_add(key.len() + value.len() + 2)
        });
        let mut document = String::with_capacity(capacity);
        for (key, value) in &self.entries {
            document.push_str(key);
            document.push('=');
            document.push_str(value);
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

        if value.as_bytes().contains(&0) {
            return Err(ConfigError::InvalidValue { line });
        }

        if value.len() > MAX_VALUE_BYTES {
            return Err(ConfigError::ValueTooLong { line });
        }

        Ok(())
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
    /// A value contains a NUL byte.
    InvalidValue {
        /// One-based line number, or zero for programmatic updates.
        line: usize,
    },
    /// A value exceeds [`MAX_VALUE_BYTES`].
    ValueTooLong {
        /// One-based line number, or zero for programmatic updates.
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
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_and_serializes_in_key_order() {
        let config = Config::parse("# comment\nzeta=2\nalpha=1\n").expect("valid config");

        assert_eq!(config.len(), 2);
        assert_eq!(config.get("alpha"), Some("1"));
        assert_eq!(config.serialize(), "alpha=1\nzeta=2\n");
        assert_eq!(
            config.iter().collect::<Vec<_>>(),
            vec![("alpha", "1"), ("zeta", "2")]
        );
    }

    #[test]
    fn parses_values_that_contain_equals() {
        let config = Config::parse("url=https://example.test?a=b\n").expect("valid config");

        assert_eq!(config.get("url"), Some("https://example.test?a=b"));
    }

    #[test]
    fn rejects_malformed_and_duplicate_entries() {
        assert_eq!(
            Config::parse("missing_separator"),
            Err(ConfigError::InvalidLine { line: 1 })
        );
        assert_eq!(
            Config::parse("alpha=1\nalpha=2\n"),
            Err(ConfigError::DuplicateKey { line: 2 })
        );
    }

    #[test]
    fn rejects_invalid_keys_and_values() {
        assert!(matches!(
            Config::parse("1alpha=value"),
            Err(ConfigError::InvalidKey { line: 1, .. })
        ));
        assert_eq!(
            Config::parse("alpha=bad\0value"),
            Err(ConfigError::InvalidValue { line: 1 })
        );

        let long_value = "x".repeat(MAX_VALUE_BYTES + 1);
        assert_eq!(
            Config::parse(&format!("alpha={long_value}")),
            Err(ConfigError::ValueTooLong { line: 1 })
        );
    }

    #[test]
    fn programmatic_updates_replace_and_remove_values() {
        let mut config = Config::new();
        config.set("alpha", "one").expect("valid entry");
        config.set("alpha", "two").expect("replacement is valid");

        assert_eq!(config.len(), 1);
        assert_eq!(config.remove("alpha"), Some("two".to_owned()));
        assert!(config.is_empty());
    }
}
