//! Foundational Rust utilities for the incremental OBS Studio migration.
//!
//! The first Phase 1 candidate is a bounded component-identifier validator and
//! copying helper. It demonstrates the intended shape of a leaf utility: a safe
//! Rust API owns the validation rules, while a small C ABI shim translates errors
//! and makes ownership explicit.

#![warn(clippy::all)]
#![warn(clippy::pedantic)]

use std::ffi::{c_char, CString};
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;
use std::str;

/// Maximum encoded length accepted by [`validate_identifier`].
pub const MAX_IDENTIFIER_BYTES: usize = 64;

/// Validation failures for a component identifier.
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
    /// A byte after the first is not an ASCII letter, digit, underscore, or hyphen.
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

/// Validates an identifier and returns it as a borrowed UTF-8 string.
///
/// Identifiers are non-empty, at most [`MAX_IDENTIFIER_BYTES`] bytes long, start
/// with an ASCII letter or underscore, and then contain only ASCII letters, digits,
/// underscores, or hyphens. The returned string borrows `input` and has no owned
/// allocation.
///
/// # Errors
///
/// Returns [`IdentifierError`] when the input violates one of the rules above.
pub fn validate_identifier(input: &[u8]) -> Result<&str, IdentifierError> {
    if input.is_empty() {
        return Err(IdentifierError::Empty);
    }

    if input.len() > MAX_IDENTIFIER_BYTES {
        return Err(IdentifierError::TooLong);
    }

    let identifier = str::from_utf8(input).map_err(|_| IdentifierError::InvalidUtf8)?;

    if !is_identifier_start(input[0]) {
        return Err(IdentifierError::InvalidFirstCharacter);
    }

    if input[1..].iter().any(|byte| !is_identifier_continue(*byte)) {
        return Err(IdentifierError::InvalidCharacter);
    }

    Ok(identifier)
}

const fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

const fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

/// Error values returned through the C ABI.
///
/// The numeric values are part of the matching C ABI contract in
/// `include/obs_rs_util.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObsRsUtilError {
    /// The operation completed successfully.
    Ok = 0,
    /// The input pointer was null.
    NullInput = 1,
    /// The input contained no bytes.
    EmptyInput = 2,
    /// The input exceeded [`MAX_IDENTIFIER_BYTES`].
    TooLong = 3,
    /// The input was not valid UTF-8.
    InvalidUtf8 = 4,
    /// The first byte was not an ASCII letter or underscore.
    InvalidFirstCharacter = 5,
    /// A later byte was outside the allowed ASCII identifier alphabet.
    InvalidCharacter = 6,
    /// The operation could not complete because an internal panic was contained.
    InternalFailure = 7,
}

impl From<IdentifierError> for ObsRsUtilError {
    fn from(error: IdentifierError) -> Self {
        match error {
            IdentifierError::Empty => Self::EmptyInput,
            IdentifierError::TooLong => Self::TooLong,
            IdentifierError::InvalidUtf8 => Self::InvalidUtf8,
            IdentifierError::InvalidFirstCharacter => Self::InvalidFirstCharacter,
            IdentifierError::InvalidCharacter => Self::InvalidCharacter,
        }
    }
}

fn write_error(output: *mut ObsRsUtilError, error: ObsRsUtilError) {
    if !output.is_null() {
        // SAFETY: `obs_rs_util_identifier_copy` requires `output` to be null or
        // point to writable storage for one `ObsRsUtilError` value.
        unsafe { *output = error };
    }
}

unsafe fn copy_identifier(
    input: *const u8,
    length: usize,
    output: *mut ObsRsUtilError,
) -> *mut c_char {
    if input.is_null() {
        write_error(output, ObsRsUtilError::NullInput);
        return ptr::null_mut();
    }

    // SAFETY: the public function documents that `input` points to `length`
    // readable bytes when it is non-null.
    let bytes = unsafe { slice::from_raw_parts(input, length) };
    let identifier = match validate_identifier(bytes) {
        Ok(identifier) => identifier,
        Err(error) => {
            write_error(output, error.into());
            return ptr::null_mut();
        }
    };

    let Ok(value) = CString::new(identifier) else {
        write_error(output, ObsRsUtilError::InvalidCharacter);
        return ptr::null_mut();
    };

    write_error(output, ObsRsUtilError::Ok);
    value.into_raw()
}

/// Validates an identifier and returns an owned, NUL-terminated copy for C callers.
///
/// The input is a byte slice described by `(input, length)` and does not need to be
/// NUL-terminated. On success, ownership of the returned string is transferred to
/// the caller; release it exactly once with [`obs_rs_util_string_free`]. `output`
/// may be null when the caller does not need an error code. This function is a
/// setup/control-plane helper and must not be called from a real-time audio or video
/// callback.
///
/// # Safety
///
/// `input` must be null or point to at least `length` readable bytes. `output` must
/// be null or point to writable storage for one [`ObsRsUtilError`] value. A returned
/// non-null pointer must only be passed to [`obs_rs_util_string_free`] once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_rs_util_identifier_copy(
    input: *const u8,
    length: usize,
    output: *mut ObsRsUtilError,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        copy_identifier(input, length, output)
    }));

    if let Ok(value) = result {
        value
    } else {
        write_error(output, ObsRsUtilError::InternalFailure);
        ptr::null_mut()
    }
}

/// Releases a string returned by [`obs_rs_util_identifier_copy`].
///
/// Passing null is a no-op. The Rust allocator remains paired with this function so
/// that C callers never free Rust-owned memory with an unrelated allocator.
///
/// # Safety
///
/// `value` must be null or a pointer returned by
/// [`obs_rs_util_identifier_copy`] that has not already been released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_rs_util_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }

    // SAFETY: the caller guarantees that `value` came from
    // `obs_rs_util_identifier_copy` and has not been freed previously.
    drop(unsafe { CString::from_raw(value) });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    fn copy(input: &[u8]) -> (Option<String>, ObsRsUtilError) {
        let mut error = ObsRsUtilError::InternalFailure;
        // SAFETY: `input.as_ptr()` points to `input.len()` readable bytes and
        // `&mut error` is writable storage for the error result.
        let value =
            unsafe { obs_rs_util_identifier_copy(input.as_ptr(), input.len(), &raw mut error) };

        let text = if value.is_null() {
            None
        } else {
            // SAFETY: successful calls return an owned, NUL-terminated C string.
            Some(
                unsafe { CStr::from_ptr(value) }
                    .to_string_lossy()
                    .into_owned(),
            )
        };

        // SAFETY: `value` is either null or the pointer returned immediately above.
        unsafe { obs_rs_util_string_free(value) };
        (text, error)
    }

    #[test]
    fn validates_and_copies_a_valid_identifier() {
        assert_eq!(
            copy(b"obs_source-1"),
            (Some("obs_source-1".to_owned()), ObsRsUtilError::Ok)
        );
    }

    #[test]
    fn translates_empty_input() {
        assert_eq!(copy(b""), (None, ObsRsUtilError::EmptyInput));
    }

    #[test]
    fn translates_a_null_input_pointer() {
        let mut error = ObsRsUtilError::InternalFailure;
        // SAFETY: null input is explicitly supported and `&mut error` is valid.
        let value = unsafe { obs_rs_util_identifier_copy(ptr::null(), 3, &raw mut error) };

        assert!(value.is_null());
        assert_eq!(error, ObsRsUtilError::NullInput);
    }

    #[test]
    fn translates_invalid_utf8() {
        assert_eq!(copy(&[b'o', 0xff]), (None, ObsRsUtilError::InvalidUtf8));
    }

    #[test]
    fn translates_invalid_identifier_characters() {
        assert_eq!(
            copy(b"1source"),
            (None, ObsRsUtilError::InvalidFirstCharacter)
        );
        assert_eq!(
            copy(b"obs.source"),
            (None, ObsRsUtilError::InvalidCharacter)
        );
    }

    #[test]
    fn translates_an_identifier_that_is_too_long() {
        let input = vec![b'a'; MAX_IDENTIFIER_BYTES + 1];
        assert_eq!(copy(&input), (None, ObsRsUtilError::TooLong));
    }

    #[test]
    fn supports_a_null_error_output_and_null_free() {
        // SAFETY: the input pointer and length describe a valid byte slice; a null
        // error output is explicitly supported.
        let value = unsafe { obs_rs_util_identifier_copy(b"obs".as_ptr(), 3, ptr::null_mut()) };

        assert!(!value.is_null());
        // SAFETY: `value` was returned by the preceding successful call.
        unsafe { obs_rs_util_string_free(value) };
        // SAFETY: null is explicitly a no-op for the release function.
        unsafe { obs_rs_util_string_free(ptr::null_mut()) };
    }
}
