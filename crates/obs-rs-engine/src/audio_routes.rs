use std::{
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
    AudioInputProvider, AudioInputState, AudioResampler,
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
    pub(crate) desktop_requested_id: Option<String>,
    pub(crate) desktop_active_id: Option<String>,
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
                request.format,
            ),
            refresh_route(
                provider,
                &devices,
                AudioDeviceKind::Output,
                request.desktop_requested_id.as_deref(),
                request.desktop_active_id.as_deref(),
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
    format: AudioFormat,
) -> AudioRouteUpdate {
    let Some((device_id, device_name)) = select_audio_device(devices, kind, requested_id) else {
        return AudioRouteUpdate::Unavailable(match requested_id {
            Some(requested) => format!("configured audio route {requested} is unavailable"),
            None => format!("automatic {kind:?} audio route is unavailable"),
        });
    };
    if active_id == Some(device_id.as_str()) {
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
    for (rate, channels) in [(48_000, 2), (44_100, 2), (48_000, 1), (44_100, 1)] {
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
            Ok(input) if device_format == mix_format => return Ok(input),
            Ok(input) => {
                return Ok(Box::new(ConvertedAudioInput {
                    input,
                    converter: AudioResampler::new(device_format, mix_format)?,
                    mix_format,
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
    converter: AudioResampler,
    mix_format: AudioFormat,
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
        let source = self.converter.input_format();
        let source_frames = (frames
            .saturating_mul(source.sample_rate() as usize)
            .saturating_add(self.mix_format.sample_rate() as usize - 1))
            / self.mix_format.sample_rate() as usize;
        let input = self.input.read_block(timestamp, source_frames.max(1))?;
        let converted = self.converter.process(&input)?;
        if converted.frames() == frames {
            return Ok(converted);
        }
        let sample_count = frames.saturating_mul(usize::from(self.mix_format.channels()));
        let mut samples = converted.samples().to_vec();
        samples.resize(sample_count, 0.0);
        samples.truncate(sample_count);
        AudioBuffer::new(self.mix_format, timestamp, samples).map_err(Into::into)
    }

    fn stop(&mut self) {
        self.input.stop();
    }
}

fn bounded_error(error: impl std::fmt::Display) -> String {
    error
        .to_string()
        .chars()
        .take(MAX_ROUTE_ERROR_CHARS)
        .collect()
}
