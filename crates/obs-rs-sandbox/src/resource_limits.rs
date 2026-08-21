use std::{path::Path, process::Command};

use super::SandboxError;

/// Resource ceilings applied to every Linux plugin subprocess.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SandboxResourceLimits {
    pub address_space_bytes: u64,
    pub cpu_seconds: u64,
    pub file_bytes: u64,
    pub open_files: u64,
    pub processes: u64,
}

impl Default for SandboxResourceLimits {
    fn default() -> Self {
        Self {
            address_space_bytes: 2 * 1_024 * 1_024 * 1_024,
            cpu_seconds: 60 * 60,
            file_bytes: 2 * 1_024 * 1_024 * 1_024,
            open_files: 256,
            processes: 64,
        }
    }
}

#[allow(clippy::unnecessary_wraps)]
pub(crate) fn limited_command(
    executable: &Path,
    arguments: &[String],
) -> Result<Command, SandboxError> {
    #[cfg(target_os = "linux")]
    {
        const PRLIMIT: &str = "/usr/bin/prlimit";
        if !Path::new(PRLIMIT).is_file() {
            return Err(SandboxError::ResourceLimitsUnavailable);
        }
        let limits = SandboxResourceLimits::default();
        let mut command = Command::new(PRLIMIT);
        command
            .arg(format!("--as={}", limits.address_space_bytes))
            .arg(format!("--cpu={}", limits.cpu_seconds))
            .arg(format!("--fsize={}", limits.file_bytes))
            .arg(format!("--nofile={}", limits.open_files))
            .arg(format!("--nproc={}", limits.processes))
            .arg("--")
            .arg(executable)
            .args(arguments);
        Ok(command)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let mut command = Command::new(executable);
        command.args(arguments);
        Ok(command)
    }
}
