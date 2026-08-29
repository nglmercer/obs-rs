use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use obs_rs_audio::{AudioFormat, AudioMonitorMode};
use obs_rs_media::{RawVideoFrame, Timestamp, VideoFrame};
#[cfg(feature = "production-gstreamer")]
use obs_rs_output::OutputProfile;
use obs_rs_output::{
    AudioEncoderConfig, SegmentedRecordingPolicy, StreamTarget, VideoEncoderConfig,
};

use obs_rs_project::Project;

#[cfg(feature = "production-gstreamer")]
use crate::RemuxRecovery;
use crate::{
    DesktopAudioSource, EngineAudioChannel, EngineError, EngineSession, EngineSnapshot,
    EngineStats, OutputEvent, OutputLifecycle, ReplaySaveStatus,
};

const DEFAULT_FRAME_QUEUE: usize = 8;
const OUTPUT_EVENT_CAPACITY: usize = 64;

#[path = "worker_output.rs"]
mod worker_output;
#[path = "worker_runtime.rs"]
mod worker_runtime;

#[cfg(test)]
use std::sync::mpsc::Receiver;
use worker_runtime::{worker_closed, worker_loop};

/// State published by an [`EngineWorker`] without exposing the worker thread.
#[derive(Clone, Debug)]
pub struct EngineWorkerSnapshot {
    /// The latest engine/output/device state.
    pub engine: EngineSnapshot,
    /// Frames waiting to be processed by the worker.
    pub queued_frames: usize,
    /// Frames rejected because the bounded handoff queue was full.
    pub dropped_frames: u64,
    /// Whether the worker thread is still accepting commands.
    pub alive: bool,
}

enum WorkerCommand {
    StartRecording(
        PathBuf,
        Option<(VideoEncoderConfig, AudioEncoderConfig)>,
        mpsc::Sender<Result<(), String>>,
    ),
    #[cfg(feature = "production-gstreamer")]
    StartRemuxRecording(
        PathBuf,
        Option<(VideoEncoderConfig, AudioEncoderConfig)>,
        mpsc::Sender<Result<(), String>>,
    ),
    #[cfg(feature = "production-gstreamer")]
    RecoverInterruptedRemux(PathBuf, mpsc::Sender<Result<RemuxRecovery, String>>),
    #[cfg(feature = "production-gstreamer")]
    DiscoverInterruptedRemux(PathBuf, mpsc::Sender<Result<Vec<PathBuf>, String>>),
    #[cfg(feature = "production-gstreamer")]
    StartRecordingProfile(
        PathBuf,
        OutputProfile,
        Option<(VideoEncoderConfig, AudioEncoderConfig)>,
        mpsc::Sender<Result<(), String>>,
    ),
    StartSegmentedRecording(
        PathBuf,
        SegmentedRecordingPolicy,
        Option<(VideoEncoderConfig, AudioEncoderConfig)>,
        mpsc::Sender<Result<(), String>>,
    ),
    FinishRecording(mpsc::Sender<Result<usize, String>>),
    AbortRecording,
    StartReplayBuffer(usize, Duration, mpsc::Sender<Result<(), String>>),
    StopReplayBuffer,
    SaveReplayBuffer(PathBuf, mpsc::Sender<Result<usize, String>>),
    StartStreaming(String, Option<(VideoEncoderConfig, AudioEncoderConfig)>),
    StartStreamingTarget(StreamTarget, VideoEncoderConfig, AudioEncoderConfig),
    FinishStreaming,
    PushFrame(VideoFrame),
    PushRawFrame(RawVideoFrame),
    MonitorAudio(Timestamp),
    SetGain(EngineAudioChannel, u16, mpsc::Sender<Result<(), String>>),
    SetPan(EngineAudioChannel, i32, mpsc::Sender<Result<(), String>>),
    SetSyncOffset(EngineAudioChannel, u32, mpsc::Sender<Result<(), String>>),
    SetGainFilter(EngineAudioChannel, i32, mpsc::Sender<Result<(), String>>),
    SetInvertPolarity(EngineAudioChannel, mpsc::Sender<Result<(), String>>),
    SetLimiter(
        EngineAudioChannel,
        i32,
        u16,
        mpsc::Sender<Result<(), String>>,
    ),
    SetCompressor(
        EngineAudioChannel,
        u16,
        i32,
        u16,
        u16,
        i32,
        mpsc::Sender<Result<(), String>>,
    ),
    SetExpander(
        EngineAudioChannel,
        u16,
        i32,
        u16,
        u16,
        i32,
        mpsc::Sender<Result<(), String>>,
    ),
    SetNoiseGate(
        EngineAudioChannel,
        i32,
        i32,
        u16,
        u16,
        u16,
        mpsc::Sender<Result<(), String>>,
    ),
    SetMuted(EngineAudioChannel, bool, mpsc::Sender<Result<(), String>>),
    SetAudioInput(Option<String>, mpsc::Sender<Result<(), String>>),
    SetDesktopAudio(Option<String>, mpsc::Sender<Result<(), String>>),
    SetMonitorMode(
        EngineAudioChannel,
        AudioMonitorMode,
        mpsc::Sender<Result<(), String>>,
    ),
    SetMonitorOutput(Option<String>, mpsc::Sender<Result<(), String>>),
    SetAudioFormat(AudioFormat, mpsc::Sender<Result<(), String>>),
    SyncProject(Project, mpsc::Sender<Result<(), String>>),
    #[cfg(test)]
    TestBlock(mpsc::Sender<()>, Receiver<()>),
    Shutdown,
}

/// Bounded background owner for engine media work.
pub struct EngineWorker {
    sender: SyncSender<WorkerCommand>,
    snapshot: Arc<Mutex<EngineWorkerSnapshot>>,
    dropped_frames: Arc<AtomicU64>,
    queued_frames: Arc<AtomicUsize>,
    output_events: Arc<Mutex<VecDeque<OutputEvent>>>,
    join: Option<JoinHandle<()>>,
}

impl EngineWorker {
    /// Starts a worker with the default bounded frame handoff capacity.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the worker thread cannot be started.
    pub fn spawn(session: EngineSession) -> Result<Self, EngineError> {
        Self::spawn_with_capacity(session, DEFAULT_FRAME_QUEUE)
    }

    /// Starts a worker with an explicit frame queue capacity.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] for a zero queue capacity or a thread-spawn
    /// failure.
    pub fn spawn_with_capacity(
        session: EngineSession,
        frame_capacity: usize,
    ) -> Result<Self, EngineError> {
        if frame_capacity == 0 {
            return Err(EngineError::InvalidConfiguration(
                "engine worker frame capacity must be greater than zero".to_owned(),
            ));
        }
        let snapshot = Arc::new(Mutex::new(EngineWorkerSnapshot {
            engine: session.snapshot(),
            queued_frames: 0,
            dropped_frames: 0,
            alive: true,
        }));
        let (sender, receiver) = mpsc::sync_channel(frame_capacity);
        let dropped_frames = Arc::new(AtomicU64::new(0));
        let queued_frames = Arc::new(AtomicUsize::new(0));
        let output_events = Arc::new(Mutex::new(VecDeque::with_capacity(OUTPUT_EVENT_CAPACITY)));
        let thread_snapshot = Arc::clone(&snapshot);
        let thread_dropped = Arc::clone(&dropped_frames);
        let thread_queued = Arc::clone(&queued_frames);
        let thread_events = Arc::clone(&output_events);
        let join = thread::Builder::new()
            .name("obs-rs-engine".to_owned())
            .spawn(move || {
                worker_loop(
                    session,
                    receiver,
                    thread_snapshot,
                    thread_dropped,
                    thread_queued,
                    thread_events,
                );
            })
            .map_err(EngineError::Io)?;
        Ok(Self {
            sender,
            snapshot,
            dropped_frames,
            queued_frames,
            output_events,
            join: Some(join),
        })
    }

    /// Returns the latest thread-safe worker snapshot.
    #[must_use]
    pub fn snapshot(&self) -> EngineWorkerSnapshot {
        self.snapshot.lock().map_or_else(
            |_| EngineWorkerSnapshot {
                engine: EngineSnapshot {
                    recording: false,
                    streaming: false,
                    // A poisoned status lock means the worker died mid-update,
                    // so both outputs are reported as failed rather than idle.
                    recording_lifecycle: OutputLifecycle::Failed,
                    streaming_lifecycle: OutputLifecycle::Failed,
                    replay_lifecycle: OutputLifecycle::Failed,
                    replay_save_status: ReplaySaveStatus::Failed {
                        reason: "engine worker status lock poisoned".to_owned(),
                    },
                    replay_buffer_packets: 0,
                    stream_state: None,
                    audio_backend: "worker unavailable".to_owned(),
                    audio_fallback: true,
                    audio_active_device_id: None,
                    desktop_audio: DesktopAudioSource::Silent("worker unavailable".to_owned()),
                    desktop_audio_active_device_id: None,
                    monitor_output: None,
                    filter_diagnostics: Vec::new(),
                    stream_metrics: None,
                    production_stream_metrics: None,
                    stream_queued_bytes: 0,
                    last_error: Some("engine worker status lock poisoned".to_owned()),
                    stats: EngineStats::default(),
                },
                queued_frames: self.queued_frames.load(Ordering::Relaxed),
                dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
                alive: false,
            },
            |snapshot| {
                let mut snapshot = snapshot.clone();
                snapshot.queued_frames = self.queued_frames.load(Ordering::Relaxed);
                snapshot.dropped_frames = self.dropped_frames.load(Ordering::Relaxed);
                snapshot
            },
        )
    }

    /// Drains the bounded lifecycle event queue without blocking.
    #[must_use]
    pub fn take_output_events(&self) -> Vec<OutputEvent> {
        self.output_events
            .lock()
            .map_or_else(|_| Vec::new(), |mut events| events.drain(..).collect())
    }

    /// Attempts to enqueue one program frame without blocking the caller.
    ///
    /// Returns `false` only when the bounded queue is full or the worker is
    /// already closed. The dropped count remains visible in [`snapshot`].
    #[must_use]
    pub fn try_push_frame(&self, frame: VideoFrame) -> bool {
        self.queued_frames.fetch_add(1, Ordering::Relaxed);
        match self.sender.try_send(WorkerCommand::PushFrame(frame)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.queued_frames.fetch_sub(1, Ordering::Relaxed);
                self.dropped_frames.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut snapshot) = self.snapshot.lock() {
                    snapshot.dropped_frames = self.dropped_frames.load(Ordering::Relaxed);
                }
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.queued_frames.fetch_sub(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Attempts to enqueue one packed/planar program frame without blocking.
    #[must_use]
    pub fn try_push_raw_frame(&self, frame: RawVideoFrame) -> bool {
        self.queued_frames.fetch_add(1, Ordering::Relaxed);
        match self.sender.try_send(WorkerCommand::PushRawFrame(frame)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.queued_frames.fetch_sub(1, Ordering::Relaxed);
                self.dropped_frames.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut snapshot) = self.snapshot.lock() {
                    snapshot.dropped_frames = self.dropped_frames.load(Ordering::Relaxed);
                }
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.queued_frames.fetch_sub(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Attempts to enqueue one monitor-only audio sample without blocking.
    ///
    /// This shares the command queue with output frames, so a busy worker can
    /// discard a cosmetic meter refresh instead of delaying media or the UI.
    #[must_use]
    pub fn try_monitor_audio(&self, timestamp: Timestamp) -> bool {
        self.sender
            .try_send(WorkerCommand::MonitorAudio(timestamp))
            .is_ok()
    }

    /// Applies live input gain on the worker-owned mixer.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or mixer rejects the update.
    pub fn set_channel_gain_milli(
        &self,
        channel: EngineAudioChannel,
        gain_milli: u16,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetGain(channel, gain_milli, reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Applies live stereo pan on the worker-owned mixer.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or bounded pan range rejects
    /// the update.
    pub fn set_channel_pan_milli(
        &self,
        channel: EngineAudioChannel,
        pan_milli: i32,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetPan(channel, pan_milli, reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Applies a bounded positive sync offset on the worker-owned audio
    /// channel without making the GUI own a delay queue.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or delay-line bound rejects the
    /// update.
    pub fn set_channel_sync_offset_millis(
        &self,
        channel: EngineAudioChannel,
        milliseconds: u32,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetSyncOffset(channel, milliseconds, reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Replaces the live channel's Gain filter on the worker-owned engine.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or bounded Gain range rejects
    /// the update.
    pub fn set_channel_gain_filter_db_milli(
        &self,
        channel: EngineAudioChannel,
        milli_db: i32,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetGainFilter(channel, milli_db, reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Replaces the live channel's Invert Polarity filter on the worker-owned
    /// engine.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker rejects the update.
    pub fn set_channel_invert_polarity(
        &self,
        channel: EngineAudioChannel,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetInvertPolarity(channel, reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Replaces the live channel's bounded Limiter filter on the worker-owned
    /// engine.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or limiter bounds reject the
    /// update.
    pub fn set_channel_limiter(
        &self,
        channel: EngineAudioChannel,
        threshold_db_milli: i32,
        release_ms: u16,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetLimiter(
                channel,
                threshold_db_milli,
                release_ms,
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Replaces the live channel's bounded Compressor filter on the
    /// worker-owned engine.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or compressor bounds reject the
    /// update.
    pub fn set_channel_compressor(
        &self,
        channel: EngineAudioChannel,
        ratio_milli: u16,
        threshold_db_milli: i32,
        attack_ms: u16,
        release_ms: u16,
        output_gain_db_milli: i32,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetCompressor(
                channel,
                ratio_milli,
                threshold_db_milli,
                attack_ms,
                release_ms,
                output_gain_db_milli,
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Replaces the live channel's bounded peak Expander filter on the
    /// worker-owned engine.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or expander bounds reject the
    /// update.
    pub fn set_channel_expander(
        &self,
        channel: EngineAudioChannel,
        ratio_milli: u16,
        threshold_db_milli: i32,
        attack_ms: u16,
        release_ms: u16,
        output_gain_db_milli: i32,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetExpander(
                channel,
                ratio_milli,
                threshold_db_milli,
                attack_ms,
                release_ms,
                output_gain_db_milli,
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Replaces the live channel's bounded peak Noise Gate on the worker-owned
    /// engine.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or gate controls reject the
    /// update.
    pub fn set_channel_noise_gate(
        &self,
        channel: EngineAudioChannel,
        open_threshold_db_milli: i32,
        close_threshold_db_milli: i32,
        attack_ms: u16,
        hold_ms: u16,
        release_ms: u16,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetNoiseGate(
                channel,
                open_threshold_db_milli,
                close_threshold_db_milli,
                attack_ms,
                hold_ms,
                release_ms,
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Applies live input mute on the worker-owned mixer.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or mixer rejects the update.
    pub fn set_channel_muted(
        &self,
        channel: EngineAudioChannel,
        muted: bool,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetMuted(channel, muted, reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Applies an OBS-compatible monitor destination policy on the worker-owned
    /// mixer.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or mixer rejects the update.
    pub fn set_channel_monitor_mode(
        &self,
        channel: EngineAudioChannel,
        mode: AudioMonitorMode,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetMonitorMode(channel, mode, reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Selects or clears the worker-owned asynchronous local monitor sink.
    ///
    /// Device opening is performed by the audio-output worker; this command
    /// only changes engine ownership and returns after the replacement has been
    /// handed off.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or replacement setup rejects the
    /// request.
    pub fn set_monitor_output_id(&self, device_id: Option<&str>) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetMonitorOutput(
                device_id.map(str::to_owned),
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Rebuilds the worker-owned audio runtime for a validated new format.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when an output is active, the format cannot be
    /// negotiated, or the worker has already closed.
    pub fn set_audio_format(&self, format: AudioFormat) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetAudioFormat(format, reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Switches the worker-owned audio input without blocking the GUI on
    /// `PipeWire` discovery or process setup.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker has already closed.
    pub fn set_audio_input_id(&self, device_id: Option<&str>) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetAudioInput(
                device_id.map(str::to_owned),
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Switches the worker-owned desktop loopback output without blocking the
    /// GUI on platform device setup.
    ///
    /// `None` selects the provider's default render route. The platform
    /// provider remains responsible for deciding whether that route can be
    /// opened as a loopback input.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker has already closed.
    pub fn set_desktop_audio_id(&self, device_id: Option<&str>) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetDesktopAudio(
                device_id.map(str::to_owned),
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Synchronizes a project revision on the engine thread.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the project is invalid or an output is
    /// active and cannot be rebuilt safely.
    pub fn sync_project(&self, project: Project) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SyncProject(project, reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }
}

impl Drop for EngineWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(WorkerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
