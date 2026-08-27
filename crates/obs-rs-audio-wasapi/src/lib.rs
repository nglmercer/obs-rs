//! Shared-mode Windows audio input behind the portable OBS-RS audio traits.
//!
//! The Windows implementation uses CPAL's WASAPI host. CPAL owns the native
//! device and callback details; this crate only exposes validated `f32` blocks
//! through the bounded pull interface used by the engine.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use obs_rs_audio::{
    AudioDeviceError, AudioDeviceInfo, AudioFormat, AudioInput, AudioInputProvider, AudioOutput,
    AudioOutputProvider,
};

#[cfg(target_os = "windows")]
mod windows;

/// A Windows shared-mode input provider.
#[derive(Clone, Copy, Debug, Default)]
pub struct WasapiAudioProvider;

impl WasapiAudioProvider {
    /// Creates a provider that discovers the current WASAPI input snapshot.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "windows")]
impl AudioInputProvider for WasapiAudioProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        windows::discover()
    }

    fn open_input(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
        windows::open_input(device_id, format)
    }
}

#[cfg(target_os = "windows")]
impl AudioOutputProvider for WasapiAudioProvider {
    fn discover_outputs(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        windows::discover_outputs()
    }

    fn open_output(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioOutput>, AudioDeviceError> {
        windows::open_output(device_id, format)
    }
}

#[cfg(not(target_os = "windows"))]
impl AudioInputProvider for WasapiAudioProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        Err(AudioDeviceError::Unavailable(
            "WASAPI input requires Windows".to_owned(),
        ))
    }

    fn open_input(
        &self,
        _device_id: &str,
        _format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
        Err(AudioDeviceError::Unavailable(
            "WASAPI input requires Windows".to_owned(),
        ))
    }
}

#[cfg(not(target_os = "windows"))]
impl AudioOutputProvider for WasapiAudioProvider {
    fn discover_outputs(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        Err(AudioDeviceError::Unavailable(
            "WASAPI output requires Windows".to_owned(),
        ))
    }

    fn open_output(
        &self,
        _device_id: &str,
        _format: AudioFormat,
    ) -> Result<Box<dyn AudioOutput>, AudioDeviceError> {
        Err(AudioDeviceError::Unavailable(
            "WASAPI output requires Windows".to_owned(),
        ))
    }
}

/// A stable, bounded provider-facing identifier for a CPAL device.
#[cfg(target_os = "windows")]
fn stable_device_id(device_id: &cpal::DeviceId) -> String {
    device_id.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_is_constructible_on_every_target() {
        let provider = WasapiAudioProvider::new();
        #[cfg(not(target_os = "windows"))]
        assert!(provider.discover().is_err());
        #[cfg(target_os = "windows")]
        let _ = provider;
    }
}
