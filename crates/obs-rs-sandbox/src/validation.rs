use std::path::Path;

use obs_rs_util::Identifier;

use super::{
    error::SandboxError,
    protocol::{MAX_SANDBOX_ARGUMENTS, MAX_SANDBOX_ARGUMENT_BYTES, MAX_SANDBOX_SOURCE_KINDS},
};

pub(crate) fn validate_source_kinds(source_kinds: &[Identifier]) -> Result<(), SandboxError> {
    if source_kinds.is_empty() {
        return Err(invalid_manifest("at least one source kind is required"));
    }
    if source_kinds.len() > MAX_SANDBOX_SOURCE_KINDS {
        return Err(invalid_manifest("too many source kinds"));
    }
    for (index, kind) in source_kinds.iter().enumerate() {
        if source_kinds[..index].contains(kind) {
            return Err(invalid_manifest(format!(
                "source kind {kind} is duplicated"
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_command(command: &Path, arguments: &[String]) -> Result<(), SandboxError> {
    if command.as_os_str().is_empty() || command.to_string_lossy().contains('\0') {
        return Err(SandboxError::InvalidCommand {
            reason: "executable path is empty or contains NUL".to_owned(),
        });
    }
    if arguments.len() > MAX_SANDBOX_ARGUMENTS {
        return Err(SandboxError::InvalidArguments {
            reason: format!("argument count exceeds {MAX_SANDBOX_ARGUMENTS}"),
        });
    }
    if arguments
        .iter()
        .any(|argument| argument.len() > MAX_SANDBOX_ARGUMENT_BYTES || argument.contains('\0'))
    {
        return Err(SandboxError::InvalidArguments {
            reason: format!(
                "an argument exceeds {MAX_SANDBOX_ARGUMENT_BYTES} bytes or contains NUL"
            ),
        });
    }
    Ok(())
}

pub(crate) fn invalid_manifest(reason: impl Into<String>) -> SandboxError {
    SandboxError::InvalidManifest {
        reason: reason.into(),
    }
}
