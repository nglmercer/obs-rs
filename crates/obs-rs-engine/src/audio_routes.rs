use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use obs_rs_audio::{
    AudioBuffer, AudioDeviceError, AudioDeviceInfo, AudioDeviceKind, AudioFormat, AudioInput,
    AudioInputProvider, AudioInputState, StreamingAudioResampler, COMMON_AUDIO_DEVICE_FORMATS,
};
use obs_rs_media::Timestamp;

/// Media-time interval between automatic route catalog refreshes.
pub(crate) const ROUTE_REFRESH_INTERVAL_NANOS: u64 = 250_000_000;

const ROUTE_QUEUE_CAPACITY: usize = 1;
const MAX_ROUTE_ERROR_CHARS: usize = 512;
const ROUTE_SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// One audio-route refresh request. The sequence lets the engine discard a
/// result that was already in flight when the user changed an explicit device
/// selection.
pub(crate) struct AudioRouteRequest {
    pub(crate) sequence: u64,
    pub(crate) format: AudioFormat,
    pub(crate) microphone_requested_id: Option<String>,
    pub(crate) microphone_active_id: Option<String>,
    /// The active microphone ID can remain stable after its native stream has
    /// failed (for example, when WASAPI invalidates a client without changing
    /// the endpoint's identity). A failed stream must be reopened even when
    /// discovery returns the same ID.
    pub(crate) microphone_active_failed: bool,
    pub(crate) desktop_requested_id: Option<String>,
    pub(crate) desktop_active_id: Option<String>,
    /// See [`Self::microphone_active_failed`], for the render endpoint used by
    /// desktop loopback capture.
    pub(crate) desktop_active_failed: bool,
}

/// A result from the bounded automatic-route worker.
pub(crate) struct AudioRouteResult {
    pub(crate) sequence: u64,
    pub(crate) microphone: AudioRouteUpdate,
    pub(crate) desktop: AudioRouteUpdate,
}

/// One route's change after a provider catalog refresh.
pub(crate) enum AudioRouteUpdate {
    /// The selected route is still the active route.
    Unchanged,
    /// A new input was opened off the engine/audio tick.
    Opened(AudioRoute),
    /// The requested or automatic route is not currently available.
    Unavailable(String),
}

/// An opened input together with the provider identity used to select it.
pub(crate) struct AudioRoute {
    pub(crate) input: Box<dyn AudioInput>,
    pub(crate) device_id: String,
    pub(crate) device_name: String,
}

/// A bounded worker that discovers and opens audio routes.
pub(crate) struct AudioRouteWorker {
    sender: SyncSender<AudioRouteRequest>,
    result: Arc<Mutex<Option<AudioRouteResult>>>,
    cancelled: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl AudioRouteWorker {
    /// Starts one provider-discovery/opening thread with a capacity-one request
    /// queue and a capacity-one latest result slot.
    pub(crate) fn spawn(provider: Arc<dyn AudioInputProvider>) -> Result<Self, std::io::Error> {
        let (sender, receiver) = mpsc::sync_channel(ROUTE_QUEUE_CAPACITY);
        let result = Arc::new(Mutex::new(None));
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread_result = Arc::clone(&result);
        let thread_cancelled = Arc::clone(&cancelled);
        let join = thread::Builder::new()
            .name("obs-rs-audio-routes".to_owned())
            .spawn(move || run_route_worker(provider, receiver, thread_result, thread_cancelled))?;
        Ok(Self {
            sender,
            result,
            cancelled,
            join: Some(join),
        })
    }

    /// Attempts to enqueue the newest refresh without waiting for discovery or
    /// device opening. A `false` result means the worker is still processing a
    /// previous bounded request.
    pub(crate) fn try_refresh(&self, request: AudioRouteRequest) -> bool {
        if self.cancelled.load(Ordering::Acquire) {
            return false;
        }
        match self.sender.try_send(request) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }

    /// Takes the latest completed result without waiting for the worker.
    pub(crate) fn take_result(&self) -> Option<AudioRouteResult> {
        self.result.lock().ok()?.take()
    }
}

impl Drop for AudioRouteWorker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        // The worker owns all provider discovery/opening work. Join a healthy
        // worker so an engine that repeatedly changes devices does not leave
        // route threads behind; detach a native call that ignores
        // cancellation after a bounded grace period.
        let Some(join) = self.join.take() else {
            return;
        };
        let deadline = std::time::Instant::now() + ROUTE_SHUTDOWN_GRACE;
        while !join.is_finished() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if join.is_finished() {
            let _ = join.join();
        }
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the worker loop owns its bounded provider and synchronization cells"
)]
fn run_route_worker(
    provider: Arc<dyn AudioInputProvider>,
    receiver: Receiver<AudioRouteRequest>,
    result: Arc<Mutex<Option<AudioRouteResult>>>,
    cancelled: Arc<AtomicBool>,
) {
    while !cancelled.load(Ordering::Acquire) {
        let request = match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let completed = refresh_routes(&provider, request);
        if let Ok(mut slot) = result.lock() {
            *slot = Some(completed);
        }
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the completed request is consumed into one bounded result"
)]
fn refresh_routes(
    provider: &Arc<dyn AudioInputProvider>,
    request: AudioRouteRequest,
) -> AudioRouteResult {
    let (microphone, desktop) = match provider.discover() {
        Ok(devices) => (
            refresh_route(
                provider,
                &devices,
                AudioDeviceKind::Input,
                request.microphone_requested_id.as_deref(),
                request.microphone_active_id.as_deref(),
                request.microphone_active_failed,
                request.format,
            ),
            refresh_route(
                provider,
                &devices,
                AudioDeviceKind::Output,
                request.desktop_requested_id.as_deref(),
                request.desktop_active_id.as_deref(),
                request.desktop_active_failed,
                request.format,
            ),
        ),
        Err(error) => {
            let reason = bounded_error(error);
            (
                AudioRouteUpdate::Unavailable(reason.clone()),
                AudioRouteUpdate::Unavailable(reason),
            )
        }
    };
    AudioRouteResult {
        sequence: request.sequence,
        microphone,
        desktop,
    }
}

fn refresh_route(
    provider: &Arc<dyn AudioInputProvider>,
    devices: &[AudioDeviceInfo],
    kind: AudioDeviceKind,
    requested_id: Option<&str>,
    active_id: Option<&str>,
    active_failed: bool,
    format: AudioFormat,
) -> AudioRouteUpdate {
    let Some((device_id, device_name)) = select_audio_device(devices, kind, requested_id) else {
        return AudioRouteUpdate::Unavailable(match requested_id {
            Some(requested) => format!("configured audio route {requested} is unavailable"),
            None => format!("automatic {kind:?} audio route is unavailable"),
        });
    };
    if active_id == Some(device_id.as_str()) && !active_failed {
        return AudioRouteUpdate::Unchanged;
    }
    let opened = if kind == AudioDeviceKind::Output {
        open_loopback_with_conversion(provider, &device_id, format)
    } else {
        open_input_with_conversion(provider, &device_id, format)
    };
    match opened {
        Ok(input) => AudioRouteUpdate::Opened(AudioRoute {
            input,
            device_id,
            device_name,
        }),
        Err(error) => AudioRouteUpdate::Unavailable(bounded_error(error)),
    }
}

pub(crate) fn select_audio_device(
    devices: &[AudioDeviceInfo],
    kind: AudioDeviceKind,
    requested_id: Option<&str>,
) -> Option<(String, String)> {
    let selected = if let Some(requested) = requested_id {
        devices
            .iter()
            .find(|device| device.kind() == kind && device.id() == requested && device.available())
    } else {
        devices
            .iter()
            .find(|device| device.kind() == kind && device.available() && device.is_default())
            .or_else(|| {
                devices
                    .iter()
                    .find(|device| device.kind() == kind && device.available())
            })
    }?;
    Some((selected.id().to_owned(), selected.name().to_owned()))
}

pub(crate) fn open_input_with_conversion(
    provider: &Arc<dyn AudioInputProvider>,
    device_id: &str,
    mix_format: AudioFormat,
) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
    open_audio_with_conversion(provider, device_id, mix_format, false)
}

pub(crate) fn open_loopback_with_conversion(
    provider: &Arc<dyn AudioInputProvider>,
    device_id: &str,
    mix_format: AudioFormat,
) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
    open_audio_with_conversion(provider, device_id, mix_format, true)
}

fn open_audio_with_conversion(
    provider: &Arc<dyn AudioInputProvider>,
    device_id: &str,
    mix_format: AudioFormat,
    loopback: bool,
) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
    let mut candidates = vec![mix_format];
    for (rate, channels) in COMMON_AUDIO_DEVICE_FORMATS {
        let candidate = AudioFormat::new(rate, channels)?;
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    let mut last_error = None;
    for device_format in candidates {
        let opened = if loopback {
            provider.open_loopback(device_id, device_format)
        } else {
            provider.open_input(device_id, device_format)
        };
        match opened {
            Ok(input) => {
                // A provider may negotiate a nearby format instead of the
                // requested one. Build the converter from the actual input
                // contract so the first block cannot be interpreted with the
                // wrong sample rate or channel layout.
                let actual_format = input.format();
                if actual_format == mix_format {
                    return Ok(input);
                }
                return Ok(Box::new(ConvertedAudioInput {
                    input,
                    converter: StreamingAudioResampler::new(actual_format, mix_format)?,
                    mix_format,
                    pending: VecDeque::new(),
                    next_source_timestamp: None,
                    source_timestamp_remainder: 0,
                }));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AudioDeviceError::Unavailable(format!(
            "audio device {device_id} does not accept a supported format"
        ))
    }))
}

struct ConvertedAudioInput {
    input: Box<dyn AudioInput>,
    converter: StreamingAudioResampler,
    mix_format: AudioFormat,
    pending: VecDeque<f32>,
    next_source_timestamp: Option<Timestamp>,
    /// Fractional nanoseconds left after advancing the source timeline. The
    /// remainder is expressed in source-rate ticks, so repeated blocks retain
    /// the exact rational duration instead of flooring every block separately.
    source_timestamp_remainder: u64,
}

impl AudioInput for ConvertedAudioInput {
    fn format(&self) -> AudioFormat {
        self.mix_format
    }

    fn state(&self) -> AudioInputState {
        self.input.state()
    }

    fn read_block(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<AudioBuffer, AudioDeviceError> {
        if frames > obs_rs_audio::MAX_AUDIO_FRAMES {
            return Err(AudioDeviceError::Audio(
                obs_rs_audio::AudioError::BufferTooLarge { frames },
            ));
        }
        let source = self.converter.input_format();
        let channels = usize::from(self.mix_format.channels());
        let sample_count = frames.checked_mul(channels).ok_or(AudioDeviceError::Audio(
            obs_rs_audio::AudioError::BufferTooLarge { frames },
        ))?;
        while self.pending.len() < sample_count {
            let missing_frames = (sample_count - self.pending.len()).div_ceil(channels);
            let source_frames = (missing_frames
                .saturating_mul(source.sample_rate() as usize)
                .saturating_add(self.mix_format.sample_rate() as usize - 1))
                / self.mix_format.sample_rate() as usize;
            let source_frames = source_frames.max(1);
            let source_timestamp = self.next_source_timestamp.unwrap_or(timestamp);
            let input = self.input.read_block(source_timestamp, source_frames)?;
            let converted = self.converter.process(&input)?;
            let (next_source_timestamp, remainder) = advance_audio_timestamp(
                source_timestamp,
                input.frames(),
                source.sample_rate(),
                self.source_timestamp_remainder,
            )?;
            self.next_source_timestamp = Some(next_source_timestamp);
            self.source_timestamp_remainder = remainder;
            if converted.frames() == 0 {
                return Err(AudioDeviceError::Unavailable(
                    "audio resampler produced no frames".to_owned(),
                ));
            }
            self.pending.extend(converted.into_samples());
        }
        let samples = self.pending.drain(..sample_count).collect();
        AudioBuffer::new(self.mix_format, timestamp, samples).map_err(Into::into)
    }

    fn stop(&mut self) {
        self.input.stop();
    }
}

fn advance_audio_timestamp(
    timestamp: Timestamp,
    frames: usize,
    sample_rate: u32,
    remainder: u64,
) -> Result<(Timestamp, u64), AudioDeviceError> {
    if sample_rate == 0 {
        return Err(AudioDeviceError::Unavailable(
            "audio sample rate cannot be zero".to_owned(),
        ));
    }
    let numerator = u128::try_from(frames)
        .map_err(|_| AudioDeviceError::Unavailable("audio frame count overflowed".to_owned()))?
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(u128::from(remainder)))
        .ok_or_else(|| AudioDeviceError::Unavailable("audio timestamp overflowed".to_owned()))?;
    let sample_rate = u128::from(sample_rate);
    let nanos = numerator / sample_rate;
    let remainder = u64::try_from(numerator % sample_rate)
        .map_err(|_| AudioDeviceError::Unavailable("audio timestamp overflowed".to_owned()))?;
    let nanos = u64::try_from(nanos)
        .map_err(|_| AudioDeviceError::Unavailable("audio timestamp overflowed".to_owned()))?;
    let timestamp = timestamp
        .checked_add(nanos)
        .ok_or_else(|| AudioDeviceError::Unavailable("audio timestamp overflowed".to_owned()))?;
    Ok((timestamp, remainder))
}

fn bounded_error(error: impl std::fmt::Display) -> String {
    error
        .to_string()
        .chars()
        .take(MAX_ROUTE_ERROR_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use obs_rs_audio::SimulatedAudioProvider;

    struct FixedFormatProvider {
        format: AudioFormat,
    }

    impl AudioInputProvider for FixedFormatProvider {
        fn discover(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
            Ok(Vec::new())
        }

        fn open_input(
            &self,
            _device_id: &str,
            format: AudioFormat,
        ) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
            if format != self.format {
                return Err(AudioDeviceError::Unavailable(format!(
                    "fixed test input only accepts {0:?}, requested {format:?}",
                    self.format
                )));
            }
            SimulatedAudioProvider::new().open_input("test-audio", format)
        }
    }

    struct NegotiatingFormatProvider {
        requested: AudioFormat,
        actual: AudioFormat,
    }

    impl AudioInputProvider for NegotiatingFormatProvider {
        fn discover(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
            Ok(Vec::new())
        }

        fn open_input(
            &self,
            _device_id: &str,
            format: AudioFormat,
        ) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
            if format != self.requested {
                return Err(AudioDeviceError::Unavailable(
                    "test provider rejected this requested format".to_owned(),
                ));
            }
            SimulatedAudioProvider::new().open_input("test-audio", self.actual)
        }
    }

    #[test]
    fn failed_active_route_reopens_even_when_identity_is_unchanged() {
        let provider: Arc<dyn AudioInputProvider> = Arc::new(SimulatedAudioProvider::new());
        let mut device = AudioDeviceInfo::new(
            "test-audio",
            "Deterministic test signal",
            AudioDeviceKind::Input,
        )
        .expect("device");
        device.set_default(true);
        let devices = [device];
        let format = AudioFormat::new(48_000, 2).expect("format");

        assert!(matches!(
            refresh_route(
                &provider,
                &devices,
                AudioDeviceKind::Input,
                None,
                Some("test-audio"),
                false,
                format,
            ),
            AudioRouteUpdate::Unchanged
        ));
        assert!(matches!(
            refresh_route(
                &provider,
                &devices,
                AudioDeviceKind::Input,
                None,
                Some("test-audio"),
                true,
                format,
            ),
            AudioRouteUpdate::Opened(route) if route.device_id == "test-audio"
        ));
    }

    #[test]
    fn route_negotiation_reaches_fixed_96_khz_endpoint_formats() {
        let mix_format = AudioFormat::new(48_000, 2).expect("mix format");
        let native_format = AudioFormat::new(96_000, 1).expect("native format");
        let provider: Arc<dyn AudioInputProvider> = Arc::new(FixedFormatProvider {
            format: native_format,
        });

        let mut input = open_input_with_conversion(&provider, "fixed-96k", mix_format)
            .expect("the bounded fallback list should reach 96 kHz mono");
        assert_eq!(input.format(), mix_format);
        let block = input
            .read_block(Timestamp::ZERO, 480)
            .expect("fixed-rate input should be converted");
        assert_eq!(block.frames(), 480);
        assert_eq!(block.format(), mix_format);
    }

    #[test]
    fn route_conversion_uses_the_provider_negotiated_format() {
        let mix_format = AudioFormat::new(48_000, 2).expect("mix format");
        let actual_format = AudioFormat::new(96_000, 1).expect("actual format");
        let provider: Arc<dyn AudioInputProvider> = Arc::new(NegotiatingFormatProvider {
            requested: mix_format,
            actual: actual_format,
        });

        let mut input = open_input_with_conversion(&provider, "negotiated", mix_format)
            .expect("the provider should open the requested format");
        assert_eq!(input.format(), mix_format);
        let block = input
            .read_block(Timestamp::ZERO, 480)
            .expect("the negotiated format should be converted");
        assert_eq!(block.frames(), 480);
        assert!(block
            .samples()
            .chunks_exact(2)
            .all(|frame| (frame[0] - frame[1]).abs() < f32::EPSILON));
    }

    #[test]
    fn audio_timestamp_preserves_fractional_sample_duration() {
        let mut timestamp = Timestamp::ZERO;
        let mut remainder = 0;
        for _ in 0..44_100 {
            (timestamp, remainder) =
                advance_audio_timestamp(timestamp, 1, 44_100, remainder).expect("timestamp");
        }

        assert_eq!(timestamp.as_nanos(), 1_000_000_000);
        assert_eq!(remainder, 0);
    }
}
