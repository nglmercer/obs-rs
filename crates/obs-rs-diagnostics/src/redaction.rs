use std::fmt;

pub const REDACTED: &str = "[REDACTED]";

/// Wrapper that prevents a sensitive value from appearing in `Debug` output.
#[derive(Clone, Eq, PartialEq)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED)
    }
}

/// Redacts known secret/path fields from line-oriented diagnostic text.
///
/// Keys are matched case-insensitively after trimming. Unknown lines are
/// preserved so the resulting diagnostic remains useful and deterministic.
#[must_use]
pub fn redact_diagnostics_text(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for line in input.lines() {
        if let Some((key, value)) = line.split_once('=') {
            output.push_str(key);
            output.push('=');
            if is_sensitive_key(key) {
                output.push_str(REDACTED);
            } else {
                output.push_str(value);
            }
        } else if let Some((key, value)) = line.split_once(':') {
            output.push_str(key);
            output.push(':');
            if is_sensitive_key(key) {
                output.push_str(REDACTED);
            } else {
                output.push_str(value);
            }
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    output
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.trim().to_ascii_lowercase().replace('-', "_");
    matches!(
        key.as_str(),
        "plugin_path"
            | "plugin_command"
            | "update_url"
            | "update_credentials"
            | "authorization"
            | "srt_passphrase"
            | "passphrase"
            | "webrtc_signaling"
            | "signaling_endpoint"
            | "signaling_token"
            | "restore_token"
            | "portal_token"
    ) || key.ends_with("_secret")
        || key.ends_with("_password")
        || key.ends_with("_credential")
}
