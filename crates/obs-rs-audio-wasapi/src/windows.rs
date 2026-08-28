//! CPAL/WASAPI implementation. This module is only compiled on Windows.

use std::{
    collections::VecDeque,
    str::FromStr,
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc, Mutex,
    },
    time::Duration,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BufferSize, Device, DeviceId, SampleFormat, Stream, StreamConfig,
};
use obs_rs_audio::{
    AudioBuffer, AudioDeviceError, AudioDeviceInfo, AudioDeviceKind, AudioFormat, AudioInput,
    AudioInputState, AudioOutput, AudioOutputState, MAX_AUDIO_FRAMES,
};
use obs_rs_media::Timestamp;

const CALLBACK_QUEUE_CAPACITY: usize = 12;
const READ_TIMEOUT: Duration = Duration::from_millis(750);
const MAX_ERROR_MESSAGE_CHARS: usize = 512;
const MAX_CALLBACK_SAMPLES: usize = MAX_AUDIO_FRAMES * 8;
const OUTPUT_STATE_STOPPED: u8 = 0;
const OUTPUT_STATE_RUNNING: u8 = 1;
const OUTPUT_STATE_FAILED: u8 = 2;

enum CallbackBlock {
    Samples(Vec<f32>),
    Xrun,
    Error(String),
}

pub(super) fn discover() -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
    let host = cpal::default_host();
    let default_input_id = default_device_id(host.default_input_device());
    let default_output_id = default_device_id(host.default_output_device());
    let devices = host
        .devices()
        .map_err(|error| unavailable("enumerate WASAPI devices", error))?;
    let mut result = Vec::new();
    for device in devices {
        let id = device
            .id()
            .map_err(|error| unavailable("read WASAPI device ID", error))?;
        let stable_id = super::stable_device_id(&id);
        let name = device
            .description()
            .map_err(|error| unavailable("read WASAPI device name", error))?
            .name()
            .to_owned();
        if device.supports_input() {
            let mut input =
                AudioDeviceInfo::new(stable_id.clone(), name.clone(), AudioDeviceKind::Input)?;
            input.set_default(default_input_id.as_deref() == Some(stable_id.as_str()));
            result.push(input);
        }
        if device.supports_output() {
            let mut output = AudioDeviceInfo::new(stable_id, name, AudioDeviceKind::Output)?;
            output.set_default(default_output_id.as_deref() == Some(output.id()));
            result.push(output);
        }
    }
    Ok(result)
}

pub(super) fn discover_outputs() -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
    Ok(discover()?
        .into_iter()
        .filter(|device| device.kind() == AudioDeviceKind::Output)
        .collect())
}

pub(super) fn open_input(
    device_id: &str,
    format: AudioFormat,
) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
    let id = DeviceId::from_str(device_id).map_err(|error| {
        AudioDeviceError::InvalidDevice(format!("invalid WASAPI device ID: {error}"))
    })?;
    let host = cpal::default_host();
    let device = host
        .device_by_id(&id)
        .ok_or_else(|| AudioDeviceError::Unavailable(device_id.to_owned()))?;
    WasapiInput::open(&device, format, false).map(|input| Box::new(input) as Box<dyn AudioInput>)
}

pub(super) fn open_loopback(
    device_id: &str,
    format: AudioFormat,
) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
    let id = DeviceId::from_str(device_id).map_err(|error| {
        AudioDeviceError::InvalidDevice(format!("invalid WASAPI device ID: {error}"))
    })?;
    let host = cpal::default_host();
    let device = host
        .device_by_id(&id)
        .ok_or_else(|| AudioDeviceError::Unavailable(device_id.to_owned()))?;
    WasapiInput::open(&device, format, true).map(|input| Box::new(input) as Box<dyn AudioInput>)
}

pub(super) fn open_output(
    device_id: &str,
    format: AudioFormat,
) -> Result<Box<dyn AudioOutput>, AudioDeviceError> {
    let id = DeviceId::from_str(device_id).map_err(|error| {
        AudioDeviceError::InvalidDevice(format!("invalid WASAPI device ID: {error}"))
    })?;
    let host = cpal::default_host();
    let device = host
        .device_by_id(&id)
        .ok_or_else(|| AudioDeviceError::Unavailable(device_id.to_owned()))?;
    WasapiOutput::open(&device, format).map(|output| Box::new(output) as Box<dyn AudioOutput>)
}

fn default_device_id(device: Option<Device>) -> Option<String> {
    device
        .and_then(|device| device.id().ok())
        .map(|id| super::stable_device_id(&id))
}

struct WasapiInput {
    format: AudioFormat,
    state: AudioInputState,
    receiver: Receiver<CallbackBlock>,
    pending: Vec<f32>,
    stream: Option<Stream>,
}

impl WasapiInput {
    fn open(
        device: &Device,
        format: AudioFormat,
        loopback: bool,
    ) -> Result<Self, AudioDeviceError> {
        if format.channels() == 0 {
            return Err(AudioDeviceError::InvalidDevice(
                "WASAPI input requires at least one channel".to_owned(),
            ));
        }
        let channels = format.channels();
        let sample_rate = format.sample_rate();
        // CPAL exposes a render endpoint's loopback stream through
        // `build_input_stream`, but its supported formats come from the
        // output side of the endpoint. Keep this choice explicit: a combined
        // headset must use its input side for microphone capture and its
        // output side for desktop loopback.
        let config = if loopback {
            if !device.supports_output() {
                return Err(AudioDeviceError::Unavailable(
                    "WASAPI endpoint does not support loopback".to_owned(),
                ));
            }
            device
                .supported_output_configs()
                .map_err(|error| unavailable("query WASAPI loopback formats", error))?
                .find_map(|range| {
                    (range.channels() == channels && range.contains_rate(sample_rate))
                        .then(|| range.with_sample_rate(sample_rate))
                })
        } else {
            device
                .supported_input_configs()
                .map_err(|error| unavailable("query WASAPI input formats", error))?
                .find_map(|range| {
                    (range.channels() == channels && range.contains_rate(sample_rate))
                        .then(|| range.with_sample_rate(sample_rate))
                })
        }
        .ok_or_else(|| {
            AudioDeviceError::Unavailable(format!(
                "WASAPI endpoint does not support {sample_rate} Hz / {channels} channels",
            ))
        })?;
        let cpal_config = StreamConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            buffer_size: BufferSize::Default,
        };
        let (sender, receiver) = mpsc::sync_channel(CALLBACK_QUEUE_CAPACITY);
        let error_sender = sender.clone();
        let stream = build_stream(
            device,
            cpal_config,
            config.sample_format(),
            sender,
            error_sender,
        )?;
        stream
            .play()
            .map_err(|error| unavailable("start WASAPI input stream", error))?;
        Ok(Self {
            format,
            state: AudioInputState::Stopped,
            receiver,
            pending: Vec::new(),
            stream: Some(stream),
        })
    }

    fn take_samples(&mut self, count: usize) -> Result<Vec<f32>, AudioDeviceError> {
        while self.pending.len() < count {
            let block = self.receiver.recv_timeout(READ_TIMEOUT).map_err(|error| {
                self.state = AudioInputState::Failed;
                match error {
                    mpsc::RecvTimeoutError::Timeout => AudioDeviceError::Unavailable(
                        "WASAPI input did not deliver a complete audio block".to_owned(),
                    ),
                    mpsc::RecvTimeoutError::Disconnected => {
                        AudioDeviceError::Unavailable("WASAPI input stream disconnected".to_owned())
                    }
                }
            })?;
            match block {
                CallbackBlock::Samples(block) => self.pending.extend(block),
                // CPAL reports a discontinuity as Xrun but keeps the WASAPI
                // capture stream alive. Drop only that notification and wait
                // for the next complete block; device invalidation and other
                // errors still take the existing reconnect path.
                CallbackBlock::Xrun => {}
                CallbackBlock::Error(message) => {
                    self.state = AudioInputState::Failed;
                    return Err(AudioDeviceError::Unavailable(format!(
                        "WASAPI input stream failed: {message}"
                    )));
                }
            }
        }
        Ok(self.pending.drain(..count).collect())
    }
}

impl AudioInput for WasapiInput {
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
        if frames > MAX_AUDIO_FRAMES {
            return Err(AudioDeviceError::Audio(
                obs_rs_audio::AudioError::BufferTooLarge { frames },
            ));
        }
        let sample_count = frames
            .checked_mul(usize::from(self.format.channels()))
            .ok_or_else(|| AudioDeviceError::Unavailable("WASAPI block is too large".to_owned()))?;
        let samples = self.take_samples(sample_count)?;
        self.state = AudioInputState::Running;
        AudioBuffer::new(self.format, timestamp, samples).map_err(AudioDeviceError::from)
    }

    fn stop(&mut self) {
        self.stream = None;
        self.state = AudioInputState::Stopped;
        self.pending.clear();
    }
}

impl Drop for WasapiInput {
    fn drop(&mut self) {
        self.stop();
    }
}

struct WasapiOutput {
    format: AudioFormat,
    state: Arc<AtomicU8>,
    last_error: Arc<Mutex<Option<String>>>,
    sender: Option<SyncSender<Vec<f32>>>,
    stream: Option<Stream>,
}

impl WasapiOutput {
    fn open(device: &Device, format: AudioFormat) -> Result<Self, AudioDeviceError> {
        if format.channels() == 0 {
            return Err(AudioDeviceError::InvalidDevice(
                "WASAPI output requires at least one channel".to_owned(),
            ));
        }
        let channels = format.channels();
        let sample_rate = format.sample_rate();
        let config = device
            .supported_output_configs()
            .map_err(|error| unavailable("query WASAPI output formats", error))?
            .find_map(|range| {
                (range.channels() == channels && range.contains_rate(sample_rate))
                    .then(|| range.with_sample_rate(sample_rate))
            })
            .ok_or_else(|| {
                AudioDeviceError::Unavailable(format!(
                    "WASAPI output does not support {sample_rate} Hz / {channels} channels",
                ))
            })?;
        let cpal_config = StreamConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            buffer_size: BufferSize::Default,
        };
        let (sender, receiver) = mpsc::sync_channel(CALLBACK_QUEUE_CAPACITY);
        let state = Arc::new(AtomicU8::new(OUTPUT_STATE_STOPPED));
        let last_error = Arc::new(Mutex::new(None));
        let stream = build_output_stream(
            device,
            cpal_config,
            config.sample_format(),
            receiver,
            Arc::clone(&state),
            Arc::clone(&last_error),
        )?;
        stream
            .play()
            .map_err(|error| unavailable("start WASAPI output stream", error))?;
        Ok(Self {
            format,
            state,
            last_error,
            sender: Some(sender),
            stream: Some(stream),
        })
    }

    fn mark_failed(&self, message: impl Into<String>) {
        self.state.store(OUTPUT_STATE_FAILED, Ordering::Release);
        if let Ok(mut last_error) = self.last_error.lock() {
            let message = message.into();
            *last_error = Some(bound_message(&message));
        }
    }

    fn failure(&self) -> AudioDeviceError {
        let reason = self
            .last_error
            .lock()
            .ok()
            .and_then(|message| message.clone())
            .unwrap_or_else(|| "WASAPI output stream failed".to_owned());
        AudioDeviceError::Unavailable(reason)
    }
}

impl AudioOutput for WasapiOutput {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn state(&self) -> AudioOutputState {
        match self.state.load(Ordering::Acquire) {
            OUTPUT_STATE_RUNNING => AudioOutputState::Running,
            OUTPUT_STATE_FAILED => AudioOutputState::Failed,
            _ => AudioOutputState::Stopped,
        }
    }

    fn write_block(&mut self, buffer: &AudioBuffer) -> Result<(), AudioDeviceError> {
        if buffer.format() != self.format {
            return Err(AudioDeviceError::Audio(
                obs_rs_audio::AudioError::FormatMismatch {
                    expected: self.format,
                    actual: buffer.format(),
                },
            ));
        }
        if self.state() == AudioOutputState::Failed {
            return Err(self.failure());
        }
        let sender = self.sender.as_ref().ok_or_else(|| {
            AudioDeviceError::Unavailable("WASAPI output stream is stopped".to_owned())
        })?;
        match sender.try_send(buffer.samples().to_vec()) {
            Ok(()) => {
                if self.state() != AudioOutputState::Failed {
                    self.state.store(OUTPUT_STATE_RUNNING, Ordering::Release);
                }
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                self.mark_failed("WASAPI output queue is full; monitor device is not keeping up");
                Err(self.failure())
            }
            Err(TrySendError::Disconnected(_)) => {
                self.mark_failed("WASAPI output stream disconnected");
                Err(self.failure())
            }
        }
    }

    fn stop(&mut self) {
        self.sender = None;
        self.stream = None;
        self.state.store(OUTPUT_STATE_STOPPED, Ordering::Release);
    }
}

impl Drop for WasapiOutput {
    fn drop(&mut self) {
        self.stop();
    }
}

struct OutputCallbackQueue {
    receiver: Receiver<Vec<f32>>,
    pending: VecDeque<f32>,
}

impl OutputCallbackQueue {
    fn new(receiver: Receiver<Vec<f32>>) -> Self {
        Self {
            receiver,
            pending: VecDeque::new(),
        }
    }

    fn next_sample(&mut self) -> f32 {
        while self.pending.is_empty() {
            match self.receiver.try_recv() {
                Ok(samples) if !samples.is_empty() => self.pending.extend(samples),
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return 0.0,
            }
        }
        self.pending.pop_front().unwrap_or(0.0)
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the callback receives the fixed stream state and bounded queue explicitly"
)]
fn build_output_stream(
    device: &Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    receiver: Receiver<Vec<f32>>,
    state: Arc<AtomicU8>,
    last_error: Arc<Mutex<Option<String>>>,
) -> Result<Stream, AudioDeviceError> {
    let error_callback = move |error: cpal::Error| {
        state.store(OUTPUT_STATE_FAILED, Ordering::Release);
        if let Ok(mut message) = last_error.lock() {
            let error = format!("WASAPI output stream failed: {error}");
            *message = Some(bound_message(&error));
        }
    };
    match sample_format {
        SampleFormat::F32 => {
            let mut queue = OutputCallbackQueue::new(receiver);
            device
                .build_output_stream(
                    config,
                    move |data: &mut [f32], _| {
                        for sample in data {
                            *sample = queue.next_sample();
                        }
                    },
                    error_callback,
                    Some(READ_TIMEOUT),
                )
                .map_err(|error| unavailable("build WASAPI f32 output stream", error))
        }
        SampleFormat::I16 => {
            let mut queue = OutputCallbackQueue::new(receiver);
            device
                .build_output_stream(
                    config,
                    move |data: &mut [i16], _| {
                        for sample in data {
                            *sample = float_to_i16(queue.next_sample());
                        }
                    },
                    error_callback,
                    Some(READ_TIMEOUT),
                )
                .map_err(|error| unavailable("build WASAPI i16 output stream", error))
        }
        SampleFormat::U16 => {
            let mut queue = OutputCallbackQueue::new(receiver);
            device
                .build_output_stream(
                    config,
                    move |data: &mut [u16], _| {
                        for sample in data {
                            *sample = float_to_u16(queue.next_sample());
                        }
                    },
                    error_callback,
                    Some(READ_TIMEOUT),
                )
                .map_err(|error| unavailable("build WASAPI u16 output stream", error))
        }
        SampleFormat::U8 => {
            let mut queue = OutputCallbackQueue::new(receiver);
            device
                .build_output_stream(
                    config,
                    move |data: &mut [u8], _| {
                        for sample in data {
                            *sample = float_to_u8(queue.next_sample());
                        }
                    },
                    error_callback,
                    Some(READ_TIMEOUT),
                )
                .map_err(|error| unavailable("build WASAPI u8 output stream", error))
        }
        other => Err(AudioDeviceError::Unavailable(format!(
            "unsupported WASAPI output sample format {other:?}"
        ))),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the sample is clamped to the representable integer range before conversion"
)]
fn float_to_i16(sample: f32) -> i16 {
    let sample = sample.clamp(-1.0, 1.0);
    if sample <= -1.0 {
        i16::MIN
    } else {
        (sample * f32::from(i16::MAX)).round() as i16
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the sample is clamped to the representable integer range before conversion"
)]
fn float_to_u16(sample: f32) -> u16 {
    (((sample.clamp(-1.0, 1.0) + 1.0) * 32_767.5).round()) as u16
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the sample is clamped to the representable integer range before conversion"
)]
fn float_to_u8(sample: f32) -> u8 {
    (((sample.clamp(-1.0, 1.0) + 1.0) * 127.5).round()) as u8
}

fn build_stream(
    device: &Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    sender: SyncSender<CallbackBlock>,
    error_sender: SyncSender<CallbackBlock>,
) -> Result<Stream, AudioDeviceError> {
    let error_callback = move |error: cpal::Error| {
        let block = if error.kind() == cpal::ErrorKind::Xrun {
            CallbackBlock::Xrun
        } else {
            CallbackBlock::Error(error.to_string())
        };
        let _ = error_sender.try_send(block);
    };
    match sample_format {
        SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| send_samples(&sender, data.iter().copied()),
                error_callback,
                Some(READ_TIMEOUT),
            )
            .map_err(|error| unavailable("build WASAPI f32 input stream", error)),
        SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    send_samples(
                        &sender,
                        data.iter().map(|sample| f32::from(*sample) / 32_768.0),
                    );
                },
                error_callback,
                Some(READ_TIMEOUT),
            )
            .map_err(|error| unavailable("build WASAPI i16 input stream", error)),
        SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| {
                    send_samples(
                        &sender,
                        data.iter()
                            .map(|sample| (f32::from(*sample) - 32_768.0) / 32_768.0),
                    );
                },
                error_callback,
                Some(READ_TIMEOUT),
            )
            .map_err(|error| unavailable("build WASAPI u16 input stream", error)),
        SampleFormat::U8 => device
            .build_input_stream(
                config,
                move |data: &[u8], _| {
                    send_samples(
                        &sender,
                        data.iter()
                            .map(|sample| (f32::from(*sample) - 128.0) / 128.0),
                    );
                },
                error_callback,
                Some(READ_TIMEOUT),
            )
            .map_err(|error| unavailable("build WASAPI u8 input stream", error)),
        other => Err(AudioDeviceError::Unavailable(format!(
            "unsupported WASAPI sample format {other:?}"
        ))),
    }
}

fn send_samples<I>(sender: &SyncSender<CallbackBlock>, samples: I)
where
    I: IntoIterator<Item = f32>,
{
    let mut block = Vec::new();
    for sample in samples {
        if block.len() >= MAX_CALLBACK_SAMPLES {
            let _ = sender.try_send(CallbackBlock::Error(format!(
                "WASAPI callback exceeded the {MAX_CALLBACK_SAMPLES}-sample block limit"
            )));
            return;
        }
        block.push(sample);
    }
    if let Err(TrySendError::Disconnected(_)) = sender.try_send(CallbackBlock::Samples(block)) {
        // The stream is being torn down. There is no consumer left to notify.
    }
}

fn unavailable(operation: &str, error: impl std::fmt::Display) -> AudioDeviceError {
    AudioDeviceError::Unavailable(format!("{operation}: {error}"))
}

fn bound_message(message: &str) -> String {
    message.chars().take(MAX_ERROR_MESSAGE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_output_conversions_clamp_and_preserve_endpoints() {
        assert_eq!(float_to_i16(-2.0), i16::MIN);
        assert_eq!(float_to_i16(-1.0), i16::MIN);
        assert_eq!(float_to_i16(0.0), 0);
        assert_eq!(float_to_i16(1.0), i16::MAX);
        assert_eq!(float_to_i16(2.0), i16::MAX);
        assert_eq!(float_to_u16(-2.0), u16::MIN);
        assert_eq!(float_to_u16(-1.0), u16::MIN);
        assert_eq!(float_to_u16(0.0), 32_768);
        assert_eq!(float_to_u16(1.0), u16::MAX);
        assert_eq!(float_to_u16(2.0), u16::MAX);
        assert_eq!(float_to_u8(-2.0), u8::MIN);
        assert_eq!(float_to_u8(-1.0), u8::MIN);
        assert_eq!(float_to_u8(0.0), 128);
        assert_eq!(float_to_u8(1.0), u8::MAX);
        assert_eq!(float_to_u8(2.0), u8::MAX);
    }

    #[test]
    fn output_queue_is_bounded_and_fills_underflow_with_silence() {
        let (sender, receiver) = mpsc::sync_channel(CALLBACK_QUEUE_CAPACITY);
        for _ in 0..CALLBACK_QUEUE_CAPACITY {
            sender
                .send(vec![0.25])
                .expect("bounded queue accepts capacity");
        }
        assert!(sender.try_send(vec![0.5]).is_err());

        let mut queue = OutputCallbackQueue::new(receiver);
        for _ in 0..CALLBACK_QUEUE_CAPACITY {
            assert_eq!(queue.next_sample().to_bits(), 0.25_f32.to_bits());
        }
        assert_eq!(queue.next_sample().to_bits(), 0.0_f32.to_bits());
    }

    #[test]
    fn input_callback_rejects_samples_beyond_the_bound() {
        let (sender, receiver) = mpsc::sync_channel(CALLBACK_QUEUE_CAPACITY);
        send_samples(&sender, std::iter::repeat_n(0.0, MAX_CALLBACK_SAMPLES + 1));
        assert!(matches!(
            receiver.try_recv().expect("callback error"),
            CallbackBlock::Error(_)
        ));
    }

    #[test]
    fn input_xruns_are_ignored_until_a_complete_block_arrives() {
        let format = AudioFormat::new(48_000, 1).expect("format");
        let (sender, receiver) = mpsc::sync_channel(CALLBACK_QUEUE_CAPACITY);
        sender.send(CallbackBlock::Xrun).expect("xrun");
        sender
            .send(CallbackBlock::Samples(vec![0.25, -0.25]))
            .expect("samples");
        let mut input = WasapiInput {
            format,
            state: AudioInputState::Stopped,
            receiver,
            pending: Vec::new(),
            stream: None,
        };
        let samples = input.take_samples(2).expect("samples after xrun");
        assert_eq!(samples, vec![0.25, -0.25]);
    }
}
