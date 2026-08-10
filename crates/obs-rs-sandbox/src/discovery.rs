use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::sync_channel,
    thread,
};

use super::{
    error::SandboxError,
    manifest::SandboxedPluginManifest,
    protocol::{
        MAX_SANDBOX_MANIFEST_BYTES, SANDBOX_FRAME_DELIVERY_TIMEOUT, SANDBOX_MANIFEST_ARGUMENT,
    },
    validation::{invalid_manifest, validate_command},
};

/// Probes one extension process for a bounded, versioned manifest.
///
/// The command is launched directly with `arguments` followed by
/// [`SANDBOX_MANIFEST_ARGUMENT`]. The child must write exactly one serialized
/// [`SandboxedPluginManifest`] to stdout and exit successfully. The probe has
/// the same byte and time bounds as source delivery, and stderr is discarded.
///
/// # Errors
///
/// Returns [`SandboxError`] when launch, timeout, output, or manifest validation
/// fails.
pub fn discover_sandbox_manifest(
    command: impl AsRef<Path>,
    arguments: &[String],
) -> Result<SandboxedPluginManifest, SandboxError> {
    let command = command.as_ref().to_owned();
    let mut probe_arguments = arguments.to_vec();
    probe_arguments.push(SANDBOX_MANIFEST_ARGUMENT.to_owned());
    validate_command(&command, &probe_arguments)?;

    let mut child = Command::new(&command)
        .args(&probe_arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| SandboxError::InvalidCommand {
            reason: error.to_string(),
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        SandboxError::InvalidCommand {
            reason: "manifest probe did not expose stdout".to_owned(),
        }
    })?;
    let (sender, receiver) = sync_channel(1);
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let read_result = stdout
            .take((MAX_SANDBOX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut output)
            .map(|_| output)
            .map_err(|error| error.to_string());
        let _ = sender.send(read_result);
    });

    let read_result = match receiver.recv_timeout(SANDBOX_FRAME_DELIVERY_TIMEOUT) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(SandboxError::InvalidCommand {
                reason: format!(
                    "manifest probe did not finish within {SANDBOX_FRAME_DELIVERY_TIMEOUT:?}"
                ),
            });
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(SandboxError::InvalidCommand {
                reason: "manifest probe reader disconnected".to_owned(),
            });
        }
    };
    let output = match read_result {
        Ok(output) => output,
        Err(reason) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(SandboxError::InvalidCommand { reason });
        }
    };
    if output.len() > MAX_SANDBOX_MANIFEST_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        return Err(SandboxError::ManifestTooLarge);
    }
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            let _ = child.kill();
            let _ = reader.join();
            return Err(SandboxError::InvalidCommand {
                reason: error.to_string(),
            });
        }
    };
    let _ = reader.join();
    if !status.success() {
        return Err(SandboxError::InvalidCommand {
            reason: format!("manifest probe exited with {status}"),
        });
    }
    let document = String::from_utf8(output)
        .map_err(|_| invalid_manifest("manifest probe output is not UTF-8"))?;
    SandboxedPluginManifest::parse(&document)
}
