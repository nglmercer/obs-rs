use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use super::{
    AudioBuffer, AudioDeviceError, AudioFormat, AudioOutput, AudioOutputProvider, AudioOutputState,
    AudioResampler,
};

const STATE_STARTING: u8 = 0;
const STATE_RUNNING: u8 = 1;
const STATE_FAILED: u8 = 2;
const STATE_STOPPED: u8 = 3;
const MAX_ERROR_MESSAGE_CHARS: usize = 512;
const RECONNECT_INTERVAL: Duration = Duration::from_secs(1);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

const COMMON_OUTPUT_FORMATS: [(u32, u16); 4] = [(48_000, 2), (44_100, 2), (48_000, 1), (44_100, 1)];

/// Lifecycle state published by an asynchronous monitoring-output worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioOutputWorkerState {
    /// The worker thread is negotiating its output device.
    Starting,
    /// The output accepted at least one complete block.
    Running,
    /// The latest device open or block write failed; bounded recovery is active.
    Failed,
    /// The worker has been cancelled and released its device.
    Stopped,
}

impl AudioOutputWorkerState {
    fn from_raw(value: u8) -> Self {
        match value {
            STATE_RUNNING => Self::Running,
            STATE_FAILED => Self::Failed,
            STATE_STOPPED => Self::Stopped,
            _ => Self::Starting,
        }
    }
}

/// Bounded telemetry for one asynchronous monitoring-output worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioOutputWorkerSnapshot {
    /// Current worker lifecycle state.
    pub state: AudioOutputWorkerState,
    /// Complete blocks waiting in the handoff queue.
    pub queued_blocks: usize,
    /// Blocks rejected because the handoff queue was full or closed.
    pub dropped_blocks: u64,
    /// Number of bounded reopen attempts after an output failure.
    pub reconnects: u64,
    /// The latest bounded device failure, if any.
    pub last_error: Option<String>,
}

/// Errors raised before an asynchronous monitoring-output worker can start.
#[derive(Debug)]
pub enum AudioOutputWorkerError {
    /// A zero-capacity handoff queue would provide no valid back-pressure.
    ZeroCapacity,
    /// The operating system rejected creation of the worker thread.
    Spawn(std::io::Error),
}

impl fmt::Display for AudioOutputWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => {
                formatter.write_str("audio output queue capacity must be non-zero")
            }
            Self::Spawn(error) => write!(formatter, "audio output worker failed to start: {error}"),
        }
    }
}

impl std::error::Error for AudioOutputWorkerError {}

/// Cloneable, non-blocking submission handle for an [`AudioOutputWorker`].
#[derive(Clone)]
pub struct AudioOutputWorkerHandle {
    sender: SyncSender<AudioBuffer>,
    state: Arc<AtomicU8>,
    queued_blocks: Arc<AtomicUsize>,
    dropped_blocks: Arc<AtomicU64>,
    reconnects: Arc<AtomicU64>,
    last_error: Arc<Mutex<Option<String>>>,
    cancelled: Arc<AtomicBool>,
}

impl AudioOutputWorkerHandle {
    /// Attempts to submit one complete monitor block without waiting.
    ///
    /// Returns `false` when the bounded queue is full or the output worker has
    /// been cancelled or the current device attempt has failed. A failed
    /// device remains restartable in the background, but blocks submitted
    /// while it is unavailable are dropped instead of being played stale after
    /// a later hot-plug recovery.
    #[must_use]
    pub fn try_write(&self, buffer: AudioBuffer) -> bool {
        if self.cancelled.load(Ordering::Acquire)
            || self.state.load(Ordering::Acquire) == STATE_FAILED
        {
            self.dropped_blocks.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.queued_blocks.fetch_add(1, Ordering::Relaxed);
        match self.sender.try_send(buffer) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.queued_blocks.fetch_sub(1, Ordering::Relaxed);
                self.dropped_blocks.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Returns a bounded snapshot without waiting for the output thread.
    #[must_use]
    pub fn snapshot(&self) -> AudioOutputWorkerSnapshot {
        AudioOutputWorkerSnapshot {
            state: AudioOutputWorkerState::from_raw(self.state.load(Ordering::Acquire)),
            queued_blocks: self.queued_blocks.load(Ordering::Acquire),
            dropped_blocks: self.dropped_blocks.load(Ordering::Acquire),
            reconnects: self.reconnects.load(Ordering::Acquire),
            last_error: self.last_error.lock().map_or_else(
                |_| Some("audio output worker status unavailable".to_owned()),
                |error| error.clone(),
            ),
        }
    }
}

/// Owns one platform audio output on a dedicated thread.
///
/// Opening and writing a native sink can block, so neither operation is
/// performed by the mixer, capture thread, engine command handler, or GUI.
/// The handoff queue stores complete bounded [`AudioBuffer`] values and uses a
/// drop-on-pressure `try_write` API; program output therefore continues when a
/// monitor device is slow or unavailable. If a sink cannot accept the engine
/// mix format, the worker tries a bounded set of common endpoint formats and
/// resamples/maps the monitor bus on its own thread.
pub struct AudioOutputWorker {
    handle: AudioOutputWorkerHandle,
    cancelled: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl AudioOutputWorker {
    /// Starts a worker that opens `device_id` at `format` on its own thread.
    ///
    /// The endpoint is first opened at the engine format. If that is not
    /// accepted, common 48/44.1 kHz mono/stereo formats are tried and the
    /// submitted monitor blocks are converted before they reach the sink.
    ///
    /// # Errors
    ///
    /// Returns [`AudioOutputWorkerError::ZeroCapacity`] for a zero queue or
    /// [`AudioOutputWorkerError::Spawn`] when the thread cannot be created.
    pub fn spawn(
        provider: Arc<dyn AudioOutputProvider>,
        device_id: impl Into<String>,
        format: AudioFormat,
        capacity_blocks: usize,
    ) -> Result<Self, AudioOutputWorkerError> {
        if capacity_blocks == 0 {
            return Err(AudioOutputWorkerError::ZeroCapacity);
        }
        let (sender, receiver) = mpsc::sync_channel(capacity_blocks);
        let state = Arc::new(AtomicU8::new(STATE_STARTING));
        let queued_blocks = Arc::new(AtomicUsize::new(0));
        let dropped_blocks = Arc::new(AtomicU64::new(0));
        let reconnects = Arc::new(AtomicU64::new(0));
        let last_error = Arc::new(Mutex::new(None));
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_queued = Arc::clone(&queued_blocks);
        let thread_reconnects = Arc::clone(&reconnects);
        let thread_last_error = Arc::clone(&last_error);
        let thread_cancelled = Arc::clone(&cancelled);
        let device_id = device_id.into();
        let join = thread::Builder::new()
            .name("obs-rs-audio-output".to_owned())
            .spawn(move || {
                run_output_worker(
                    provider,
                    device_id,
                    format,
                    receiver,
                    thread_state,
                    thread_queued,
                    thread_reconnects,
                    thread_last_error,
                    thread_cancelled,
                );
            })
            .map_err(AudioOutputWorkerError::Spawn)?;

        Ok(Self {
            handle: AudioOutputWorkerHandle {
                sender,
                state,
                queued_blocks,
                dropped_blocks,
                reconnects,
                last_error,
                cancelled: Arc::clone(&cancelled),
            },
            cancelled,
            join: Some(join),
        })
    }

    /// Returns a cloneable non-blocking submission handle.
    #[must_use]
    pub fn handle(&self) -> AudioOutputWorkerHandle {
        self.handle.clone()
    }

    /// Returns the current bounded worker telemetry.
    #[must_use]
    pub fn snapshot(&self) -> AudioOutputWorkerSnapshot {
        self.handle.snapshot()
    }
}

impl Drop for AudioOutputWorker {
    fn drop(&mut self) {
        // Cancellation is observed between complete device writes. Dropping
        // the sender wakes a worker waiting for its next block. Join healthy
        // workers within a bounded grace period so repeated monitor changes do
        // not accumulate native streams; a wedged platform write is detached
        // after the same bound so teardown remains non-blocking.
        self.cancelled.store(true, Ordering::Release);
        let Some(join) = self.join.take() else {
            return;
        };
        let deadline = std::time::Instant::now() + SHUTDOWN_GRACE;
        while !join.is_finished() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if join.is_finished() {
            let _ = join.join();
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::needless_pass_by_value,
    reason = "the worker loop receives each shared bounded telemetry cell explicitly"
)]
fn run_output_worker(
    provider: Arc<dyn AudioOutputProvider>,
    device_id: String,
    format: AudioFormat,
    receiver: mpsc::Receiver<AudioBuffer>,
    state: Arc<AtomicU8>,
    queued_blocks: Arc<AtomicUsize>,
    reconnects: Arc<AtomicU64>,
    last_error: Arc<Mutex<Option<String>>>,
    cancelled: Arc<AtomicBool>,
) {
    loop {
        if cancelled.load(Ordering::Acquire) {
            state.store(STATE_STOPPED, Ordering::Release);
            drain_queue(&receiver, &queued_blocks);
            return;
        }

        let mut output = match open_output_with_conversion(&provider, &device_id, format) {
            Ok(output) => output,
            Err(error) => {
                fail_worker(&state, &last_error, error);
                drain_queue(&receiver, &queued_blocks);
                reconnects.fetch_add(1, Ordering::Relaxed);
                if !wait_for_reconnect(&cancelled) {
                    state.store(STATE_STOPPED, Ordering::Release);
                    return;
                }
                continue;
            }
        };
        state.store(STATE_STARTING, Ordering::Release);

        loop {
            let buffer = match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(buffer) => buffer,
                Err(RecvTimeoutError::Disconnected) => {
                    output.stop();
                    state.store(STATE_STOPPED, Ordering::Release);
                    drain_queue(&receiver, &queued_blocks);
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {
                    if cancelled.load(Ordering::Acquire) {
                        output.stop();
                        state.store(STATE_STOPPED, Ordering::Release);
                        drain_queue(&receiver, &queued_blocks);
                        return;
                    }
                    continue;
                }
            };
            queued_blocks.fetch_sub(1, Ordering::Release);
            if cancelled.load(Ordering::Acquire) {
                output.stop();
                state.store(STATE_STOPPED, Ordering::Release);
                drain_queue(&receiver, &queued_blocks);
                return;
            }
            if let Err(error) = output.write_block(&buffer) {
                fail_worker(&state, &last_error, error);
                drain_queue(&receiver, &queued_blocks);
                output.stop();
                break;
            }
            state.store(STATE_RUNNING, Ordering::Release);
        }

        reconnects.fetch_add(1, Ordering::Relaxed);
        if !wait_for_reconnect(&cancelled) {
            state.store(STATE_STOPPED, Ordering::Release);
            drain_queue(&receiver, &queued_blocks);
            return;
        }
    }
}

fn wait_for_reconnect(cancelled: &AtomicBool) -> bool {
    let deadline = std::time::Instant::now() + RECONNECT_INTERVAL;
    while !cancelled.load(Ordering::Acquire) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return true;
        }
        thread::sleep(remaining.min(Duration::from_millis(20)));
    }
    false
}

fn open_output_with_conversion(
    provider: &Arc<dyn AudioOutputProvider>,
    device_id: &str,
    mix_format: AudioFormat,
) -> Result<Box<dyn AudioOutput>, AudioDeviceError> {
    let mut candidates = vec![mix_format];
    for (sample_rate, channels) in COMMON_OUTPUT_FORMATS {
        let candidate = AudioFormat::new(sample_rate, channels)?;
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }

    let mut last_error = None;
    for requested_format in candidates {
        match provider.open_output(device_id, requested_format) {
            Ok(output) => {
                let device_format = output.format();
                if device_format == mix_format {
                    return Ok(output);
                }
                let converter = AudioResampler::new(mix_format, device_format)?;
                return Ok(Box::new(ConvertedAudioOutput {
                    output,
                    converter,
                    mix_format,
                }));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AudioDeviceError::Unavailable(format!(
            "audio output {device_id} does not accept a supported format"
        ))
    }))
}

struct ConvertedAudioOutput {
    output: Box<dyn AudioOutput>,
    converter: AudioResampler,
    mix_format: AudioFormat,
}

impl AudioOutput for ConvertedAudioOutput {
    fn format(&self) -> AudioFormat {
        self.mix_format
    }

    fn state(&self) -> AudioOutputState {
        self.output.state()
    }

    fn write_block(&mut self, buffer: &AudioBuffer) -> Result<(), AudioDeviceError> {
        if buffer.format() != self.mix_format {
            return Err(AudioDeviceError::Audio(super::AudioError::FormatMismatch {
                expected: self.mix_format,
                actual: buffer.format(),
            }));
        }
        let converted = self
            .converter
            .process(buffer)
            .map_err(AudioDeviceError::from)?;
        self.output.write_block(&converted)
    }

    fn stop(&mut self) {
        self.output.stop();
    }
}

fn drain_queue(receiver: &mpsc::Receiver<AudioBuffer>, queued_blocks: &AtomicUsize) {
    while receiver.try_recv().is_ok() {
        queued_blocks.fetch_sub(1, Ordering::Release);
    }
}

fn fail_worker<E: fmt::Display>(state: &AtomicU8, last_error: &Mutex<Option<String>>, error: E) {
    if let Ok(mut last_error) = last_error.lock() {
        *last_error = Some(bounded_error(error));
    }
    state.store(STATE_FAILED, Ordering::Release);
}

fn bounded_error(error: impl fmt::Display) -> String {
    error
        .to_string()
        .chars()
        .take(MAX_ERROR_MESSAGE_CHARS)
        .collect()
}
