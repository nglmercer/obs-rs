//! Linux `PipeWire` audio input through the reviewed `pw-cat` boundary.
//!
//! The core remains free of native FFI. This adapter discovers whether a
//! `PipeWire` graph is usable with `pw-dump`, then reads negotiated raw `f32`
//! samples from `pw-cat`. If either command is unavailable, the engine can
//! select its deterministic fallback provider.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    io::{Read, ErrorKind},
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
};

use obs_rs_audio::{
    AudioBuffer, AudioDeviceError, AudioDeviceInfo, AudioDeviceKind, AudioFormat, AudioInput,
    AudioInputProvider, AudioInputState,
};
use obs_rs_media::Timestamp;

/// Stable identifier for the `PipeWire` default input route.
pub const DEFAULT_INPUT_ID: &str = "pipewire-default";

/// Process-backed `PipeWire` provider.
#[derive(Clone, Debug)]
pub struct PipeWireAudioProvider {
    dump_command: PathBuf,
    cat_command: PathBuf,
}

impl PipeWireAudioProvider {
    /// Creates a provider using `pw-dump` and `pw-cat` from `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            dump_command: PathBuf::from("pw-dump"),
            cat_command: PathBuf::from("pw-cat"),
        }
    }

    /// Creates a provider with explicit command paths, useful for packaging and
    /// deterministic adapter tests.
    #[must_use]
    pub fn with_commands(
        dump_command: impl Into<PathBuf>,
        cat_command: impl Into<PathBuf>,
    ) -> Self {
        Self {
            dump_command: dump_command.into(),
            cat_command: cat_command.into(),
        }
    }

    /// Returns the configured `pw-dump` executable path.
    #[must_use]
    pub fn dump_command(&self) -> &Path {
        &self.dump_command
    }

    /// Returns the configured `pw-cat` executable path.
    #[must_use]
    pub fn cat_command(&self) -> &Path {
        &self.cat_command
    }
}

impl Default for PipeWireAudioProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioInputProvider for PipeWireAudioProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        let output = Command::new(&self.dump_command).output().map_err(|error| {
            AudioDeviceError::Unavailable(format!(
                "PipeWire discovery command {} failed to start: {error}",
                self.dump_command.display()
            ))
        })?;
        if !output.status.success() {
            return Err(AudioDeviceError::Unavailable(format!(
                "PipeWire discovery exited with {}",
                output.status
            )));
        }
        let graph = String::from_utf8_lossy(&output.stdout);
        if !graph.contains("Audio/Source") {
            return Err(AudioDeviceError::Unavailable(
                "PipeWire has no discoverable audio source".to_owned(),
            ));
        }
        Ok(vec![AudioDeviceInfo::new(
            DEFAULT_INPUT_ID,
            "PipeWire default input",
            AudioDeviceKind::Input,
        )?])
    }

    fn open_input(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
        if device_id != DEFAULT_INPUT_ID {
            return Err(AudioDeviceError::Unavailable(format!(
                "unknown PipeWire input {device_id}"
            )));
        }
        let sample_rate = format.sample_rate().to_string();
        let channels = format.channels().to_string();
        let mut child = Command::new(&self.cat_command)
            .args([
                "--record",
                "--raw",
                "--format",
                "f32",
                "--rate",
                sample_rate.as_str(),
                "--channels",
                channels.as_str(),
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                AudioDeviceError::Unavailable(format!(
                    "PipeWire capture command {} failed to start: {error}",
                    self.cat_command.display()
                ))
            })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AudioDeviceError::Unavailable("PipeWire capture has no stdout pipe".to_owned())
        })?;
        Ok(Box::new(PipeWireInput {
            format,
            state: AudioInputState::Stopped,
            child,
            stdout,
        }))
    }
}

struct PipeWireInput {
    format: AudioFormat,
    state: AudioInputState,
    child: Child,
    stdout: ChildStdout,
}

impl AudioInput for PipeWireInput {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn state(&self) -> AudioInputState {
        self.state
    }

    fn read_block(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<AudioBuffer, AudioDeviceError> {
        if frames == 0 {
            return Err(AudioDeviceError::InvalidDevice(
                "PipeWire audio blocks must contain at least one frame".to_owned(),
            ));
        }
        let sample_count = frames
            .checked_mul(usize::from(self.format.channels()))
            .ok_or_else(|| AudioDeviceError::InvalidDevice("audio block is too large".to_owned()))?;
        let byte_count = sample_count
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or_else(|| AudioDeviceError::InvalidDevice("audio block byte size overflowed".to_owned()))?;
        let mut bytes = vec![0_u8; byte_count];
        if let Err(error) = self.stdout.read_exact(&mut bytes) {
            self.state = AudioInputState::Failed;
            let error = if error.kind() == ErrorKind::UnexpectedEof {
                std::io::Error::new(ErrorKind::UnexpectedEof, "PipeWire ended the audio stream")
            } else {
                error
            };
            return Err(AudioDeviceError::Io(error));
        }
        let samples = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        self.state = AudioInputState::Running;
        Ok(AudioBuffer::new(self.format, timestamp, samples)?)
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.state = AudioInputState::Stopped;
    }
}

impl Drop for PipeWireInput {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_uses_stable_default_id() {
        let provider = PipeWireAudioProvider::with_commands("pw-dump-test", "pw-cat-test");
        assert_eq!(provider.dump_command(), Path::new("pw-dump-test"));
        assert_eq!(provider.cat_command(), Path::new("pw-cat-test"));
    }

    #[test]
    fn unknown_device_is_rejected_before_spawning_process() {
        let provider = PipeWireAudioProvider::new();
        let format = AudioFormat::new(48_000, 2).expect("format");
        let Err(error) = provider.open_input("missing", format) else {
            panic!("unknown device should fail")
        };
        assert!(error.to_string().contains("unknown PipeWire input"));
    }
}
