//! Small, safe value types shared by the OBS-RS workspace.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

pub mod file;
pub mod json;
pub mod random;

pub use file::replace_file;
pub use json::{Json, JsonError};
pub use random::{fill_random, random_u64, RandomError, RandomPool};

use std::{fmt, str, str::FromStr};

/// Maximum UTF-8 byte length accepted for an [`Identifier`].
pub const MAX_IDENTIFIER_BYTES: usize = 64;

/// Validation failures for an [`Identifier`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentifierError {
    /// The identifier contains no bytes.
    Empty,
    /// The identifier exceeds [`MAX_IDENTIFIER_BYTES`].
    TooLong,
    /// The input is not valid UTF-8.
    InvalidUtf8,
    /// The first byte is not an ASCII letter or underscore.
    InvalidFirstCharacter,
    /// A later byte is outside the identifier alphabet.
    InvalidCharacter,
}

impl fmt::Display for IdentifierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "identifier is empty",
            Self::TooLong => "identifier is too long",
            Self::InvalidUtf8 => "identifier is not valid UTF-8",
            Self::InvalidFirstCharacter => {
                "identifier must start with an ASCII letter or underscore"
            }
            Self::InvalidCharacter => {
                "identifier contains a character outside the ASCII identifier alphabet"
            }
        };

        formatter.write_str(message)
    }
}

impl std::error::Error for IdentifierError {}

/// A validated, owned identifier used for plugin, source, and scene names.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Identifier(String);

impl Identifier {
    /// Creates an identifier after validating its UTF-8 and ASCII alphabet.
    ///
    /// # Errors
    ///
    /// Returns an [`IdentifierError`] when `input` is empty, too long, or contains
    /// a byte outside the identifier alphabet.
    pub fn new(input: &str) -> Result<Self, IdentifierError> {
        validate_identifier(input.as_bytes())
            .map(str::to_owned)
            .map(Self)
    }

    /// Returns the validated identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Converts the identifier into its owned string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for Identifier {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Lets keyed collections look up an [`Identifier`] from a plain `&str`.
///
/// `Ord`, `Eq`, and `Hash` all delegate to the identifier text, so they agree
/// with the borrowed `str` implementations as `Borrow` requires. This is what
/// allows `map.get("name")` on an `Identifier`-keyed map without constructing
/// and validating a temporary owned identifier.
impl std::borrow::Borrow<str> for Identifier {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Identifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Identifier {
    type Err = IdentifierError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::new(input)
    }
}

impl TryFrom<&str> for Identifier {
    type Error = IdentifierError;

    fn try_from(input: &str) -> Result<Self, Self::Error> {
        Self::new(input)
    }
}

/// Validates an identifier and returns the original borrowed text.
///
/// # Errors
///
/// Returns an [`IdentifierError`] when the byte slice is empty, too long, not valid
/// UTF-8, or contains a byte outside the identifier alphabet.
pub fn validate_identifier(input: &[u8]) -> Result<&str, IdentifierError> {
    if input.is_empty() {
        return Err(IdentifierError::Empty);
    }

    if input.len() > MAX_IDENTIFIER_BYTES {
        return Err(IdentifierError::TooLong);
    }

    let identifier = str::from_utf8(input).map_err(|_| IdentifierError::InvalidUtf8)?;

    for (index, byte) in input.iter().copied().enumerate() {
        let valid = if index == 0 {
            is_identifier_start(byte)
        } else {
            is_identifier_continue(byte)
        };
        if !valid {
            return Err(if index == 0 {
                IdentifierError::InvalidFirstCharacter
            } else {
                IdentifierError::InvalidCharacter
            });
        }
    }

    Ok(identifier)
}

const fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

const fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_identifier_and_preserves_text() {
        let identifier = Identifier::new("obs_source-1").expect("fixture is valid");

        assert_eq!(identifier.as_str(), "obs_source-1");
        assert_eq!(identifier.to_string(), "obs_source-1");
        assert_eq!("obs_source-1".parse::<Identifier>(), Ok(identifier));
    }

    #[test]
    fn rejects_empty_invalid_and_too_long_values() {
        assert_eq!(Identifier::new(""), Err(IdentifierError::Empty));
        assert_eq!(
            Identifier::new("1source"),
            Err(IdentifierError::InvalidFirstCharacter)
        );
        assert_eq!(
            Identifier::new("obs.source"),
            Err(IdentifierError::InvalidCharacter)
        );
        assert_eq!(
            Identifier::new(&"x".repeat(MAX_IDENTIFIER_BYTES + 1)),
            Err(IdentifierError::TooLong)
        );
        assert_eq!(
            validate_identifier(&[b'o', 0xff]),
            Err(IdentifierError::InvalidUtf8)
        );
    }
}
