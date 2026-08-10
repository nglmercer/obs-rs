//! Self-contained configuration state for the incremental OBS Studio migration.
//!
//! This crate demonstrates a Phase 2 boundary: a deterministic, testable Rust
//! policy/state core behind a small opaque C adapter. It is deliberately a
//! control-plane component. Handles are not thread-safe and none of the exported
//! functions may be called from real-time audio or video callbacks.

#![warn(clippy::all)]
#![warn(clippy::pedantic)]

use std::collections::BTreeMap;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;
use std::str;

use obs_rs_util::{validate_identifier, IdentifierError};

/// Maximum encoded size accepted by [`Config::parse`].
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;

/// Maximum encoded size accepted for one configuration value.
pub const MAX_VALUE_BYTES: usize = 4096;

/// A validated, deterministic configuration map.
///
/// Keys are sorted during serialization, which makes output stable for testing,
/// logging, and native callers. The type owns all key and value strings. A `Config`
/// value is not synchronized internally; callers must serialize access when sharing
/// it across threads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Config {
    entries: BTreeMap<String, String>,
}

impl Config {
    /// Creates an empty configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses a UTF-8 configuration document.
    ///
    /// Each non-empty, non-comment line has the form `key=value`. Lines beginning
    /// with optional whitespace followed by `#` are comments. Keys use the same
    /// ASCII identifier alphabet as [`obs_rs_util::validate_identifier`]. Values may
    /// be empty, may contain `=`, and may contain any UTF-8 text except NUL bytes.
    /// Duplicate keys are rejected rather than silently overwritten.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the document is too large, contains an invalid
    /// line or key/value, or repeats a key.
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

    /// Inserts or replaces one validated configuration entry.
    ///
    /// Replacing an existing value is intentional for programmatic updates. Parsed
    /// documents still reject duplicate keys so that malformed persisted state is
    /// not silently accepted.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidKey`], [`ConfigError::InvalidValue`], or
    /// [`ConfigError::ValueTooLong`] when the entry is invalid.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), ConfigError> {
        Self::validate_entry(key, value, 0)?;
        self.entries.insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    /// Returns a borrowed value for `key`, if present.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// Returns the number of stored entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the configuration contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serializes the configuration in sorted `key=value\n` form.
    #[must_use]
    pub fn serialize(&self) -> String {
        let mut document = String::new();
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

/// Errors produced by the configuration state core.
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

/// Error values returned by the configuration C ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObsRsConfigError {
    /// The operation completed successfully.
    Ok = 0,
    /// A required input pointer was null.
    NullInput = 1,
    /// The input bytes were not valid UTF-8.
    InvalidUtf8 = 2,
    /// A configuration line was malformed.
    InvalidLine = 3,
    /// A configuration key was invalid.
    InvalidKey = 4,
    /// A key occurred more than once in a document.
    DuplicateKey = 5,
    /// A value contained a NUL byte.
    InvalidValue = 6,
    /// A value exceeded [`MAX_VALUE_BYTES`].
    ValueTooLong = 7,
    /// The configuration handle was null.
    NullConfig = 8,
    /// The caller's output buffer cannot hold the requested bytes.
    BufferTooSmall = 9,
    /// The requested key is not present.
    KeyNotFound = 10,
    /// The complete document exceeded [`MAX_CONFIG_BYTES`].
    InputTooLarge = 11,
    /// An internal panic was contained at the C boundary.
    InternalFailure = 12,
}

impl From<&ConfigError> for ObsRsConfigError {
    fn from(error: &ConfigError) -> Self {
        match error {
            ConfigError::InputTooLarge => Self::InputTooLarge,
            ConfigError::InvalidLine { .. } => Self::InvalidLine,
            ConfigError::InvalidKey { .. } => Self::InvalidKey,
            ConfigError::DuplicateKey { .. } => Self::DuplicateKey,
            ConfigError::InvalidValue { .. } => Self::InvalidValue,
            ConfigError::ValueTooLong { .. } => Self::ValueTooLong,
        }
    }
}

/// Opaque C handle for [`Config`].
#[repr(C)]
pub struct ObsRsConfig {
    _private: [u8; 0],
}

fn write_error(output: *mut ObsRsConfigError, error: ObsRsConfigError) {
    if !output.is_null() {
        // SAFETY: each public C function documents that `output` is null or points
        // to writable storage for one error value.
        unsafe { *output = error };
    }
}

unsafe fn read_text<'a>(input: *const u8, length: usize) -> Result<&'a str, ObsRsConfigError> {
    if input.is_null() {
        return Err(ObsRsConfigError::NullInput);
    }

    // SAFETY: the public C functions require `input` to point to `length` readable
    // bytes when it is non-null. The returned lifetime is only used within the
    // calling operation and never stored in a config handle.
    let bytes = unsafe { slice::from_raw_parts(input, length) };
    str::from_utf8(bytes).map_err(|_| ObsRsConfigError::InvalidUtf8)
}

unsafe fn config_ref<'a>(config: *const ObsRsConfig) -> Result<&'a Config, ObsRsConfigError> {
    if config.is_null() {
        return Err(ObsRsConfigError::NullConfig);
    }

    // SAFETY: the public C functions require `config` to be a live handle returned
    // by `obs_rs_config_create` and not concurrently mutated.
    Ok(unsafe { &*(config.cast::<Config>()) })
}

unsafe fn config_mut<'a>(config: *mut ObsRsConfig) -> Result<&'a mut Config, ObsRsConfigError> {
    if config.is_null() {
        return Err(ObsRsConfigError::NullConfig);
    }

    // SAFETY: the public C functions require `config` to be a live, exclusively
    // accessed handle returned by `obs_rs_config_create`.
    Ok(unsafe { &mut *(config.cast::<Config>()) })
}

unsafe fn write_bytes(
    bytes: &[u8],
    output: *mut u8,
    capacity: usize,
    required: *mut usize,
    error: *mut ObsRsConfigError,
) -> bool {
    if !required.is_null() {
        // SAFETY: the caller guarantees that `required` is writable when non-null.
        unsafe { *required = bytes.len() };
    }

    if bytes.len() > capacity || (output.is_null() && !bytes.is_empty()) {
        write_error(error, ObsRsConfigError::BufferTooSmall);
        return false;
    }

    if !bytes.is_empty() {
        // SAFETY: the caller guarantees that `output` points to `capacity` writable
        // bytes when it is non-null, and the capacity check above bounds the copy.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len()) };
    }

    write_error(error, ObsRsConfigError::Ok);
    true
}

unsafe fn create_config(
    input: *const u8,
    length: usize,
    error: *mut ObsRsConfigError,
) -> *mut ObsRsConfig {
    let document = match read_text(input, length) {
        Ok(document) => document,
        Err(error_code) => {
            write_error(error, error_code);
            return ptr::null_mut();
        }
    };

    let config = match Config::parse(document) {
        Ok(config) => config,
        Err(parse_error) => {
            write_error(error, (&parse_error).into());
            return ptr::null_mut();
        }
    };

    write_error(error, ObsRsConfigError::Ok);
    Box::into_raw(Box::new(config)).cast::<ObsRsConfig>()
}

/// Creates an opaque configuration handle by parsing `(input, length)`.
///
/// The document is UTF-8 and follows [`Config::parse`]. The returned handle owns
/// its copied strings and must be released with [`obs_rs_config_destroy`]. Handles
/// are not thread-safe; callers must serialize access and must not use them from
/// real-time callbacks.
///
/// # Safety
///
/// `input` must point to `length` readable bytes and `error` must be null or point
/// to writable storage for one [`ObsRsConfigError`] value. The returned handle must
/// only be passed to the functions in this module and must be destroyed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_rs_config_create(
    input: *const u8,
    length: usize,
    error: *mut ObsRsConfigError,
) -> *mut ObsRsConfig {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        create_config(input, length, error)
    }));

    if let Ok(config) = result {
        config
    } else {
        write_error(error, ObsRsConfigError::InternalFailure);
        ptr::null_mut()
    }
}

/// Destroys a configuration handle; passing null is a no-op.
///
/// # Safety
///
/// `config` must be null or a live handle returned by [`obs_rs_config_create`] that
/// has not already been destroyed. No other thread may access it during destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_rs_config_destroy(config: *mut ObsRsConfig) {
    if config.is_null() {
        return;
    }

    // SAFETY: the caller guarantees that `config` is a live allocation returned by
    // `obs_rs_config_create`.
    drop(unsafe { Box::from_raw(config.cast::<Config>()) });
}

unsafe fn set_config(
    config: *mut ObsRsConfig,
    key: *const u8,
    key_length: usize,
    value: *const u8,
    value_length: usize,
    error: *mut ObsRsConfigError,
) -> bool {
    let config = match config_mut(config) {
        Ok(config) => config,
        Err(error_code) => {
            write_error(error, error_code);
            return false;
        }
    };
    let key = match read_text(key, key_length) {
        Ok(key) => key,
        Err(error_code) => {
            write_error(error, error_code);
            return false;
        }
    };
    let value = match read_text(value, value_length) {
        Ok(value) => value,
        Err(error_code) => {
            write_error(error, error_code);
            return false;
        }
    };

    match config.set(key, value) {
        Ok(()) => {
            write_error(error, ObsRsConfigError::Ok);
            true
        }
        Err(config_error) => {
            write_error(error, (&config_error).into());
            false
        }
    }
}

/// Inserts or replaces one configuration value through the C ABI.
///
/// `key` and `value` are explicit UTF-8 byte ranges and do not need NUL terminators.
/// The handle is mutated in place and remains owned by the caller.
///
/// # Safety
///
/// `config` must be a live, exclusively accessed handle. `key` and `value` must
/// point to their declared readable byte ranges, and `error` must be null or
/// writable. This function is not thread-safe or real-time safe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_rs_config_set(
    config: *mut ObsRsConfig,
    key: *const u8,
    key_length: usize,
    value: *const u8,
    value_length: usize,
    error: *mut ObsRsConfigError,
) -> bool {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        set_config(config, key, key_length, value, value_length, error)
    }));

    if let Ok(success) = result {
        success
    } else {
        write_error(error, ObsRsConfigError::InternalFailure);
        false
    }
}

unsafe fn get_config(
    config: *const ObsRsConfig,
    key: *const u8,
    key_length: usize,
    output: *mut u8,
    capacity: usize,
    required: *mut usize,
    error: *mut ObsRsConfigError,
) -> bool {
    let config = match config_ref(config) {
        Ok(config) => config,
        Err(error_code) => {
            write_error(error, error_code);
            return false;
        }
    };
    let key = match read_text(key, key_length) {
        Ok(key) => key,
        Err(error_code) => {
            write_error(error, error_code);
            return false;
        }
    };
    let Some(value) = config.get(key) else {
        if !required.is_null() {
            // SAFETY: `required` is documented as writable when non-null.
            unsafe { *required = 0 };
        }
        write_error(error, ObsRsConfigError::KeyNotFound);
        return false;
    };

    write_bytes(value.as_bytes(), output, capacity, required, error)
}

/// Copies a configuration value into a caller-owned buffer.
///
/// `required` receives the exact number of bytes needed, excluding any NUL
/// terminator. Callers may pass a null output with zero capacity to query that
/// length; the function then returns false with [`ObsRsConfigError::BufferTooSmall`]
/// when the value is non-empty.
///
/// # Safety
///
/// `config` must be a live, non-mutated handle. `key` must point to its declared
/// readable UTF-8 byte range. `output` must be null or point to `capacity` writable
/// bytes, `required` must be null or writable, and `error` must be null or writable.
/// This function is not thread-safe or real-time safe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_rs_config_get(
    config: *const ObsRsConfig,
    key: *const u8,
    key_length: usize,
    output: *mut u8,
    capacity: usize,
    required: *mut usize,
    error: *mut ObsRsConfigError,
) -> bool {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        get_config(config, key, key_length, output, capacity, required, error)
    }));

    if let Ok(success) = result {
        success
    } else {
        write_error(error, ObsRsConfigError::InternalFailure);
        false
    }
}

unsafe fn serialize_config(
    config: *const ObsRsConfig,
    output: *mut u8,
    capacity: usize,
    required: *mut usize,
    error: *mut ObsRsConfigError,
) -> bool {
    let config = match config_ref(config) {
        Ok(config) => config,
        Err(error_code) => {
            write_error(error, error_code);
            return false;
        }
    };

    let document = config.serialize();
    write_bytes(document.as_bytes(), output, capacity, required, error)
}

/// Serializes a configuration into a caller-owned byte buffer.
///
/// The output is sorted `key=value\n` text without a trailing NUL. Use `required` to
/// query the required capacity before copying. This function is not thread-safe or
/// real-time safe.
///
/// # Safety
///
/// `config` must be a live, non-mutated handle. `output` must be null or point to
/// `capacity` writable bytes, `required` must be null or writable, and `error` must
/// be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn obs_rs_config_serialize(
    config: *const ObsRsConfig,
    output: *mut u8,
    capacity: usize,
    required: *mut usize,
    error: *mut ObsRsConfigError,
) -> bool {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        serialize_config(config, output, capacity, required, error)
    }));

    if let Ok(success) = result {
        success
    } else {
        write_error(error, ObsRsConfigError::InternalFailure);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_and_serializes_in_key_order() {
        let config = Config::parse("# comment\nzeta=2\nalpha=1\n").expect("valid config");

        assert_eq!(config.len(), 2);
        assert_eq!(config.get("alpha"), Some("1"));
        assert_eq!(config.serialize(), "alpha=1\nzeta=2\n");
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
    fn programmatic_updates_replace_values() {
        let mut config = Config::new();
        config.set("alpha", "one").expect("valid entry");
        config.set("alpha", "two").expect("replacement is valid");

        assert_eq!(config.len(), 1);
        assert_eq!(config.get("alpha"), Some("two"));
    }

    fn create(document: &[u8]) -> (*mut ObsRsConfig, ObsRsConfigError) {
        let mut error = ObsRsConfigError::InternalFailure;
        // SAFETY: `document.as_ptr()` describes `document.len()` readable bytes and
        // `&raw mut error` is writable storage.
        let config =
            unsafe { obs_rs_config_create(document.as_ptr(), document.len(), &raw mut error) };
        (config, error)
    }

    #[test]
    fn ffi_round_trips_and_reports_required_capacity() {
        let (config, error) = create(b"zeta=2\nalpha=1\n");
        assert_eq!(error, ObsRsConfigError::Ok);
        assert!(!config.is_null());

        let mut required = 0;
        let mut error = ObsRsConfigError::InternalFailure;
        // SAFETY: the handle is live, and null output with zero capacity is the
        // documented length-query form.
        let copied = unsafe {
            obs_rs_config_serialize(
                config,
                ptr::null_mut(),
                0,
                &raw mut required,
                &raw mut error,
            )
        };
        assert!(!copied);
        assert_eq!(required, b"alpha=1\nzeta=2\n".len());
        assert_eq!(error, ObsRsConfigError::BufferTooSmall);

        let mut output = vec![0_u8; required];
        // SAFETY: `output` has exactly the required writable capacity and the
        // handle remains unmodified.
        let copied = unsafe {
            obs_rs_config_serialize(
                config,
                output.as_mut_ptr(),
                output.len(),
                &raw mut required,
                &raw mut error,
            )
        };
        assert!(copied);
        assert_eq!(error, ObsRsConfigError::Ok);
        assert_eq!(output, b"alpha=1\nzeta=2\n");

        // SAFETY: `config` is the live handle returned above.
        unsafe { obs_rs_config_destroy(config) };
    }

    #[test]
    fn ffi_updates_and_reads_a_value() {
        let (config, error) = create(b"alpha=one\n");
        assert_eq!(error, ObsRsConfigError::Ok);

        let mut error = ObsRsConfigError::InternalFailure;
        // SAFETY: all pointers describe valid ranges and the handle is exclusively
        // accessed by this test.
        let updated = unsafe {
            obs_rs_config_set(
                config,
                b"alpha".as_ptr(),
                5,
                b"two".as_ptr(),
                3,
                &raw mut error,
            )
        };
        assert!(updated);
        assert_eq!(error, ObsRsConfigError::Ok);

        let mut output = [0_u8; 3];
        let mut required = 0;
        // SAFETY: the handle is live and `output` is writable for three bytes.
        let read = unsafe {
            obs_rs_config_get(
                config,
                b"alpha".as_ptr(),
                5,
                output.as_mut_ptr(),
                output.len(),
                &raw mut required,
                &raw mut error,
            )
        };
        assert!(read);
        assert_eq!(required, 3);
        assert_eq!(output, *b"two");
        assert_eq!(error, ObsRsConfigError::Ok);

        // SAFETY: `config` is the live handle returned above.
        unsafe { obs_rs_config_destroy(config) };
    }

    #[test]
    fn ffi_rejects_invalid_input_and_null_handles() {
        let (config, error) = create(b"1alpha=value");
        assert!(config.is_null());
        assert_eq!(error, ObsRsConfigError::InvalidKey);

        let mut error = ObsRsConfigError::InternalFailure;
        // SAFETY: null handles and null error outputs are explicitly supported by
        // this boundary; the key bytes are valid.
        let updated = unsafe {
            obs_rs_config_set(
                ptr::null_mut(),
                b"alpha".as_ptr(),
                5,
                b"value".as_ptr(),
                5,
                &raw mut error,
            )
        };
        assert!(!updated);
        assert_eq!(error, ObsRsConfigError::NullConfig);
        // SAFETY: null destruction is explicitly a no-op.
        unsafe { obs_rs_config_destroy(ptr::null_mut()) };
    }
}
