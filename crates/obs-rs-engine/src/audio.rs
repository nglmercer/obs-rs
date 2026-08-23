use std::sync::Arc;

use obs_rs_audio::{
    AudioBuffer, AudioDeviceKind, AudioFormat, AudioInput, AudioInputProvider,
    SimulatedAudioProvider,
};
use obs_rs_media::Timestamp;

use super::audio_routes::{open_input_with_conversion, select_audio_device};
use super::AUDIO_RECONNECT_INTERVAL_NANOS;

pub(super) fn open_audio_input(
    provider: &Arc<dyn AudioInputProvider>,
    format: AudioFormat,
    requested_id: Option<&str>,
) -> (Box<dyn AudioInput>, String, bool, Option<String>) {
    if let Some((input, device_name, device_id)) =
        open_live_audio_input(provider, format, requested_id)
    {
        return (input, device_name, false, Some(device_id));
    }
    let fallback = SimulatedAudioProvider::new()
        .open_input("test-audio", format)
        .unwrap_or_else(|error| unreachable!("fallback audio format is valid: {error}"));
    (fallback, "simulated fallback".to_owned(), true, None)
}

pub(super) fn audio_reconnect_deadline(enabled: bool) -> Option<Timestamp> {
    enabled.then_some(Timestamp::from_nanos(AUDIO_RECONNECT_INTERVAL_NANOS))
}

pub(super) fn open_live_audio_input(
    provider: &Arc<dyn AudioInputProvider>,
    format: AudioFormat,
    requested_id: Option<&str>,
) -> Option<(Box<dyn AudioInput>, String, String)> {
    let (device_id, device_name) = discover_audio_input_device(provider, requested_id)?;
    let input = open_input_with_conversion(provider, &device_id, format).ok()?;
    Some((input, device_name, device_id))
}

pub(super) fn discover_audio_input_device(
    provider: &Arc<dyn AudioInputProvider>,
    requested_id: Option<&str>,
) -> Option<(String, String)> {
    let devices = provider.discover().ok()?;
    select_audio_device(&devices, AudioDeviceKind::Input, requested_id)
}

/// Opens the playback monitor that feeds the desktop channel.
///
/// Desktop capture reads a device the platform classifies as an *output*; a
/// provider that can record from it hands back what the machine is playing.
/// Returning `None` is a normal outcome — a headless session or a provider
/// without monitor support simply records a silent desktop channel — so this
/// never substitutes the simulated signal, which would make the meter lie.
pub(super) fn open_desktop_audio(
    provider: &Arc<dyn AudioInputProvider>,
    format: AudioFormat,
    requested_id: Option<&str>,
) -> (Option<Box<dyn AudioInput>>, String, Option<String>) {
    let Ok(devices) = provider.discover() else {
        return (None, "unavailable".to_owned(), None);
    };
    let selected = select_audio_device(&devices, AudioDeviceKind::Output, requested_id);
    let Some((device_id, device_name)) = selected else {
        return (None, "no playback monitor".to_owned(), None);
    };
    match open_input_with_conversion(provider, &device_id, format) {
        Ok(input) => (Some(input), device_name, Some(device_id)),
        Err(_) => (
            None,
            "unavailable (no compatible device format)".to_owned(),
            None,
        ),
    }
}

pub(super) fn open_live_desktop_audio(
    provider: &Arc<dyn AudioInputProvider>,
    format: AudioFormat,
    requested_id: Option<&str>,
) -> Option<(Box<dyn AudioInput>, String, String)> {
    let devices = provider.discover().ok()?;
    let (device_id, device_name) =
        select_audio_device(&devices, AudioDeviceKind::Output, requested_id)?;
    let input = open_input_with_conversion(provider, &device_id, format).ok()?;
    Some((input, device_name, device_id))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to the full u16 range before conversion"
)]
pub(super) fn audio_peak_milli(buffer: &AudioBuffer) -> u16 {
    let peak = buffer
        .samples()
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    (peak * 1_000.0).round().clamp(0.0, f32::from(u16::MAX)) as u16
}
