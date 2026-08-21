//! CPAL/WASAPI implementation. This module is only compiled on Windows.

use std::{
    str::FromStr,
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    time::Duration,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BufferSize, Device, DeviceId, SampleFormat, Stream, StreamConfig,
};
use obs_rs_audio::{
    AudioBuffer, AudioDeviceError, AudioDeviceInfo, AudioDeviceKind, AudioFormat, AudioInput,
    AudioInputState, MAX_AUDIO_FRAMES,
};
use obs_rs_media::Timestamp;

const CALLBACK_QUEUE_CAPACITY: usize = 12;
const READ_TIMEOUT: Duration = Duration::from_millis(750);

enum CallbackBlock {
    Samples(Vec<f32>),
    Error(String),
}

pub(super) fn discover() -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|error| unavailable("enumerate WASAPI input devices", error))?;
    let mut result = Vec::new();
    for device in devices {
        let id = device
            .id()
            .map_err(|error| unavailable("read WASAPI input device ID", error))?;
        let name = device
            .description()
            .map_err(|error| unavailable("read WASAPI input device name", error))?
            .name()
            .to_owned();
        result.push(AudioDeviceInfo::new(
            super::stable_device_id(&id),
            name,
            AudioDeviceKind::Input,
        )?);
    }
    Ok(result)
}

pub(super) fn open_input(
    device_id: &str,
    format: AudioFormat,
) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
    let raw_id = device_id
        .strip_prefix("wasapi:")
        .ok_or_else(|| AudioDeviceError::InvalidDevice(device_id.to_owned()))?;
    let id = DeviceId::from_str(raw_id).map_err(|error| {
        AudioDeviceError::InvalidDevice(format!("invalid WASAPI device ID: {error}"))
    })?;
    let host = cpal::default_host();
    let device = host
        .device_by_id(&id)
        .ok_or_else(|| AudioDeviceError::Unavailable(device_id.to_owned()))?;
    WasapiInput::open(&device, format).map(|input| Box::new(input) as Box<dyn AudioInput>)
}

struct WasapiInput {
    format: AudioFormat,
    state: AudioInputState,
    receiver: Receiver<CallbackBlock>,
    pending: Vec<f32>,
    stream: Option<Stream>,
}

impl WasapiInput {
    fn open(device: &Device, format: AudioFormat) -> Result<Self, AudioDeviceError> {
        if format.channels() == 0 {
            return Err(AudioDeviceError::InvalidDevice(
                "WASAPI input requires at least one channel".to_owned(),
            ));
        }
        let channels = format.channels();
        let sample_rate = format.sample_rate();
        let config = device
            .supported_input_configs()
            .map_err(|error| unavailable("query WASAPI input formats", error))?
            .find_map(|range| {
                (range.channels() == channels && range.contains_rate(sample_rate))
                    .then(|| range.with_sample_rate(sample_rate))
            })
            .ok_or_else(|| {
                AudioDeviceError::Unavailable(format!(
                    "WASAPI input does not support {sample_rate} Hz / {channels} channels",
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

fn build_stream(
    device: &Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    sender: SyncSender<CallbackBlock>,
    error_sender: SyncSender<CallbackBlock>,
) -> Result<Stream, AudioDeviceError> {
    let error_callback = move |error: cpal::Error| {
        let _ = error_sender.try_send(CallbackBlock::Error(error.to_string()));
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
        other => Err(AudioDeviceError::Unavailable(format!(
            "unsupported WASAPI sample format {other:?}"
        ))),
    }
}

fn send_samples<I>(sender: &SyncSender<CallbackBlock>, samples: I)
where
    I: IntoIterator<Item = f32>,
{
    let block = samples.into_iter().collect::<Vec<_>>();
    if let Err(TrySendError::Disconnected(_)) = sender.try_send(CallbackBlock::Samples(block)) {
        // The stream is being torn down. There is no consumer left to notify.
    }
}

fn unavailable(operation: &str, error: impl std::fmt::Display) -> AudioDeviceError {
    AudioDeviceError::Unavailable(format!("{operation}: {error}"))
}
