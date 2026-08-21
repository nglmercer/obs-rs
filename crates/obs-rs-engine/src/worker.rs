use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use obs_rs_media::{RawVideoFrame, Timestamp, VideoFrame};
use obs_rs_output::{AudioEncoderConfig, StreamTarget, VideoEncoderConfig};

use obs_rs_project::Project;

use crate::{
    DesktopAudioSource, EngineAudioChannel, EngineError, EngineSession, EngineSnapshot,
    EngineStats, OutputEvent, OutputLifecycle,
};

const DEFAULT_FRAME_QUEUE: usize = 8;
const OUTPUT_EVENT_CAPACITY: usize = 64;

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
        u16,
        i32,
        u16,
        u16,
        i32,
        mpsc::Sender<Result<(), String>>,
    ),
    SetMuted(EngineAudioChannel, bool, mpsc::Sender<Result<(), String>>),
    SetAudioInput(Option<String>, mpsc::Sender<Result<(), String>>),
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
                    stream_state: None,
                    audio_backend: "worker unavailable".to_owned(),
                    audio_fallback: true,
                    desktop_audio: DesktopAudioSource::Silent("worker unavailable".to_owned()),
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

    /// Requests a recording start and waits for the worker to validate it.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker is closed or the engine rejects
    /// the recording.
    pub fn start_recording(&self, path: impl Into<PathBuf>) -> Result<(), EngineError> {
        self.start_recording_with_config(path.into(), None)
    }

    /// Requests a configured production recording start on the worker thread.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker is closed or the runtime rejects
    /// the selected codec or encoder.
    pub fn start_recording_configured(
        &self,
        path: impl Into<PathBuf>,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        self.start_recording_with_config(path.into(), Some((video, audio)))
    }

    fn start_recording_with_config(
        &self,
        path: PathBuf,
        encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::StartRecording(path, encoder_config, reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Requests recording finalization on the worker thread.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when finalization fails or the worker closes.
    pub fn finish_recording(&self) -> Result<usize, EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::FinishRecording(reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Cancels an open recording without doing file work on the caller.
    pub fn abort_recording(&self) {
        let _ = self.sender.send(WorkerCommand::AbortRecording);
    }

    /// Starts bounded replay capture on the worker thread.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker queue is closed or the replay
    /// bounds are invalid.
    pub fn start_replay_buffer(
        &self,
        capacity_bytes: usize,
        duration: Duration,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::StartReplayBuffer(
                capacity_bytes,
                duration,
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Stops replay capture and discards its worker-owned history.
    pub fn stop_replay_buffer(&self) {
        let _ = self.sender.send(WorkerCommand::StopReplayBuffer);
    }

    /// Saves the worker-owned replay history without moving packet data onto
    /// the caller's event thread.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker is closed, replay is inactive,
    /// or the atomic packet file cannot be written.
    pub fn save_replay_buffer(&self, path: impl Into<PathBuf>) -> Result<usize, EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SaveReplayBuffer(path.into(), reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Enqueues a stream start without waiting for transport or encoder setup.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] only when the bounded worker queue cannot accept
    /// the request. Transport failures are published through
    /// [`Self::take_output_events`] and [`Self::snapshot`].
    pub fn start_streaming(&self, address: &str) -> Result<(), EngineError> {
        self.start_streaming_with_config(address, None)
    }

    /// Enqueues a stream start with explicit production encoder tuning.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the bounded command queue rejects it.
    pub fn start_streaming_configured(
        &self,
        address: &str,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        self.start_streaming_with_config(address, Some((video, audio)))
    }

    /// Enqueues a typed production target without exposing secrets in an URL.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the bounded worker queue rejects the request.
    pub fn start_streaming_target_configured(
        &self,
        target: StreamTarget,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        set_streaming_lifecycle(&self.snapshot, OutputLifecycle::Starting);
        push_output_event(&self.output_events, OutputEvent::Starting);
        self.sender
            .try_send(WorkerCommand::StartStreamingTarget(target, video, audio))
            .map_err(|error| command_enqueue_error(&error))
    }

    fn start_streaming_with_config(
        &self,
        address: &str,
        encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        set_streaming_lifecycle(&self.snapshot, OutputLifecycle::Starting);
        push_output_event(&self.output_events, OutputEvent::Starting);
        match self.sender.try_send(WorkerCommand::StartStreaming(
            address.to_owned(),
            encoder_config,
        )) {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = command_enqueue_error(&error);
                set_streaming_lifecycle(&self.snapshot, OutputLifecycle::Failed);
                push_output_event(
                    &self.output_events,
                    OutputEvent::Failed {
                        reason: error.to_string(),
                    },
                );
                Err(error)
            }
        }
    }

    /// Enqueues stream teardown without waiting for network or pipeline work.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the bounded worker queue cannot accept the
    /// request.
    pub fn finish_streaming(&self) -> Result<(), EngineError> {
        set_streaming_lifecycle(&self.snapshot, OutputLifecycle::Stopping);
        push_output_event(&self.output_events, OutputEvent::Stopping);
        match self.sender.try_send(WorkerCommand::FinishStreaming) {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = command_enqueue_error(&error);
                set_streaming_lifecycle(&self.snapshot, OutputLifecycle::Failed);
                push_output_event(
                    &self.output_events,
                    OutputEvent::Failed {
                        reason: error.to_string(),
                    },
                );
                Err(error)
            }
        }
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

    /// Replaces the live channel's bounded peak-based Gate preset on the
    /// worker-owned engine.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or gate controls reject the
    /// update.
    pub fn set_channel_noise_gate(
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
            .send(WorkerCommand::SetNoiseGate(
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

#[allow(
    clippy::needless_pass_by_value,
    reason = "the worker thread takes ownership of its receive and status handles"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive command loop keeps worker state transitions serialized"
)]
fn worker_loop(
    mut session: EngineSession,
    receiver: Receiver<WorkerCommand>,
    snapshot: Arc<Mutex<EngineWorkerSnapshot>>,
    dropped_frames: Arc<AtomicU64>,
    queued_frames: Arc<AtomicUsize>,
    output_events: Arc<Mutex<VecDeque<OutputEvent>>>,
) {
    while let Ok(command) = receiver.recv() {
        let shutdown = match command {
            WorkerCommand::StartRecording(path, encoder_config, reply) => {
                let result = start_recording(&mut session, path, encoder_config);
                let _ = reply.send(result);
                false
            }
            WorkerCommand::FinishRecording(reply) => {
                let result = session
                    .finish_recording()
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::AbortRecording => abort_recording(&mut session),
            WorkerCommand::StartReplayBuffer(capacity_bytes, duration, reply) => {
                let result = session
                    .start_replay_buffer(capacity_bytes, duration)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::StopReplayBuffer => {
                session.stop_replay_buffer();
                false
            }
            WorkerCommand::SaveReplayBuffer(path, reply) => {
                let result = session
                    .save_replay_buffer(path)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::StartStreaming(address, encoder_config) => {
                start_stream(
                    &mut session,
                    &address,
                    encoder_config.as_ref(),
                    &output_events,
                );
                false
            }
            WorkerCommand::StartStreamingTarget(target, video, audio) => {
                start_stream_target(&mut session, &target, &video, &audio, &output_events);
                false
            }
            WorkerCommand::FinishStreaming => {
                finish_streaming(&mut session, &output_events);
                false
            }
            WorkerCommand::PushFrame(frame) => {
                queued_frames.fetch_sub(1, Ordering::Relaxed);
                if let Err(error) = session.push_program_frame(&frame) {
                    session.last_error = Some(error.to_string());
                }
                false
            }
            WorkerCommand::PushRawFrame(frame) => {
                queued_frames.fetch_sub(1, Ordering::Relaxed);
                if let Err(error) = session.push_program_raw_frame(&frame) {
                    session.last_error = Some(error.to_string());
                }
                false
            }
            WorkerCommand::MonitorAudio(timestamp) => {
                if let Err(error) = session.monitor_audio_until(timestamp) {
                    session.last_error = Some(error.to_string());
                }
                false
            }
            WorkerCommand::SetGain(channel, gain, reply) => {
                let result = session
                    .set_channel_gain_milli(channel, gain)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetGainFilter(channel, milli_db, reply) => {
                let result = session
                    .set_channel_gain_filter_db_milli(channel, milli_db)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetInvertPolarity(channel, reply) => {
                let result = session
                    .set_channel_invert_polarity(channel)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetLimiter(channel, threshold_db_milli, release_ms, reply) => {
                let result = session
                    .set_channel_limiter(channel, threshold_db_milli, release_ms)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetCompressor(
                channel,
                ratio_milli,
                threshold_db_milli,
                attack_ms,
                release_ms,
                output_gain_db_milli,
                reply,
            ) => {
                let result = session
                    .set_channel_compressor(
                        channel,
                        ratio_milli,
                        threshold_db_milli,
                        attack_ms,
                        release_ms,
                        output_gain_db_milli,
                    )
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetExpander(
                channel,
                ratio_milli,
                threshold_db_milli,
                attack_ms,
                release_ms,
                output_gain_db_milli,
                reply,
            ) => {
                let result = session
                    .set_channel_expander(
                        channel,
                        ratio_milli,
                        threshold_db_milli,
                        attack_ms,
                        release_ms,
                        output_gain_db_milli,
                    )
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetNoiseGate(
                channel,
                ratio_milli,
                threshold_db_milli,
                attack_ms,
                release_ms,
                output_gain_db_milli,
                reply,
            ) => {
                let result = session
                    .set_channel_noise_gate(
                        channel,
                        ratio_milli,
                        threshold_db_milli,
                        attack_ms,
                        release_ms,
                        output_gain_db_milli,
                    )
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetMuted(channel, muted, reply) => {
                let result = session
                    .set_channel_muted(channel, muted)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetAudioInput(device_id, reply) => {
                session.set_audio_input_id(device_id.as_deref());
                let _ = reply.send(Ok(()));
                false
            }
            WorkerCommand::SyncProject(project, reply) => {
                let result = session
                    .sync_project(project)
                    .map_err(|error| error.to_string());
                if let Err(error) = &result {
                    session.last_error = Some(error.clone());
                }
                let _ = reply.send(result);
                false
            }
            #[cfg(test)]
            WorkerCommand::TestBlock(entered, release) => {
                let _ = entered.send(());
                let _ = release.recv();
                false
            }
            WorkerCommand::Shutdown => true,
        };

        pump_stream(&mut session);
        publish_snapshot(
            &session,
            &snapshot,
            &dropped_frames,
            &queued_frames,
            !shutdown,
        );
        if shutdown {
            break;
        }
    }
    let _ = session.finish_streaming();
    session.abort_recording();
    publish_snapshot(&session, &snapshot, &dropped_frames, &queued_frames, false);
}

fn pump_stream(session: &mut EngineSession) {
    if session.is_streaming() {
        if let Err(error) = session.pump_stream() {
            session.last_error = Some(error.to_string());
        }
    }
}

fn publish_snapshot(
    session: &EngineSession,
    snapshot: &Arc<Mutex<EngineWorkerSnapshot>>,
    dropped_frames: &Arc<AtomicU64>,
    queued_frames: &Arc<AtomicUsize>,
    alive: bool,
) {
    if let Ok(mut target) = snapshot.lock() {
        *target = EngineWorkerSnapshot {
            engine: session.snapshot(),
            queued_frames: queued_frames.load(Ordering::Relaxed),
            dropped_frames: dropped_frames.load(Ordering::Relaxed),
            alive,
        };
    }
}

fn worker_closed() -> EngineError {
    EngineError::Worker("engine worker is closed".to_owned())
}

fn abort_recording(session: &mut EngineSession) -> bool {
    session.abort_recording();
    false
}

fn start_recording(
    session: &mut EngineSession,
    path: PathBuf,
    encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
) -> Result<(), String> {
    match encoder_config {
        Some((video, audio)) => session.start_recording_configured(path, video, audio),
        None => session.start_recording(path),
    }
    .map_err(|error| error.to_string())
}

fn start_stream(
    session: &mut EngineSession,
    address: &str,
    encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    output_events: &Arc<Mutex<VecDeque<OutputEvent>>>,
) {
    let result = match encoder_config {
        Some((video, audio)) => session.start_streaming_configured(address, video, audio),
        None => session.start_streaming(address),
    };
    match result {
        Ok(()) => push_output_event(output_events, OutputEvent::Running),
        Err(error) => push_output_event(
            output_events,
            OutputEvent::Failed {
                reason: error.to_string(),
            },
        ),
    }
}

fn start_stream_target(
    session: &mut EngineSession,
    target: &StreamTarget,
    video: &VideoEncoderConfig,
    audio: &AudioEncoderConfig,
    output_events: &Arc<Mutex<VecDeque<OutputEvent>>>,
) {
    match session.start_streaming_target_configured(target, video, audio) {
        Ok(()) => push_output_event(output_events, OutputEvent::Running),
        Err(error) => push_output_event(
            output_events,
            OutputEvent::Failed {
                reason: error.to_string(),
            },
        ),
    }
}

fn finish_streaming(
    session: &mut EngineSession,
    output_events: &Arc<Mutex<VecDeque<OutputEvent>>>,
) {
    match session.finish_streaming() {
        Ok(()) => push_output_event(output_events, OutputEvent::Stopped),
        Err(error) => push_output_event(
            output_events,
            OutputEvent::Failed {
                reason: error.to_string(),
            },
        ),
    }
}

fn command_enqueue_error(error: &TrySendError<WorkerCommand>) -> EngineError {
    match error {
        TrySendError::Full(_) => EngineError::Worker("engine worker queue is full".to_owned()),
        TrySendError::Disconnected(_) => worker_closed(),
    }
}

fn set_streaming_lifecycle(
    snapshot: &Arc<Mutex<EngineWorkerSnapshot>>,
    lifecycle: OutputLifecycle,
) {
    if let Ok(mut snapshot) = snapshot.lock() {
        snapshot.engine.streaming_lifecycle = lifecycle;
    }
}

fn push_output_event(events: &Arc<Mutex<VecDeque<OutputEvent>>>, event: OutputEvent) {
    if let Ok(mut events) = events.lock() {
        if events.len() == OUTPUT_EVENT_CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use obs_rs_audio::AudioFormat;
    use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

    use super::*;
    use crate::EngineConfig;

    fn worker(capacity: usize) -> EngineWorker {
        let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("valid frame rate"))
            .expect("valid video format");
        let audio = AudioFormat::new(48_000, 2).expect("valid audio format");
        let session = EngineSession::for_format(format, EngineConfig::new(audio)).expect("session");
        EngineWorker::spawn_with_capacity(session, capacity).expect("worker")
    }

    #[test]
    fn stream_start_returns_without_waiting_for_worker_or_transport_setup() {
        let worker = Arc::new(worker(1));
        let (entered_send, entered_receive) = mpsc::channel();
        let (release_send, release_receive) = mpsc::channel();
        worker
            .sender
            .send(WorkerCommand::TestBlock(entered_send, release_receive))
            .expect("block command");
        entered_receive.recv().expect("worker entered barrier");

        let requesting_worker = Arc::clone(&worker);
        let (result_send, result_receive) = mpsc::channel();
        thread::spawn(move || {
            let _ = result_send.send(requesting_worker.start_streaming("127.0.0.1:0"));
        });
        result_receive
            .recv_timeout(Duration::from_millis(250))
            .expect("start must return while the worker remains blocked")
            .expect("start request accepted");
        assert_eq!(
            worker.snapshot().engine.streaming_lifecycle,
            OutputLifecycle::Starting
        );
        assert_eq!(worker.take_output_events(), vec![OutputEvent::Starting]);

        release_send.send(()).expect("release worker");
        for _ in 0..100 {
            if worker.snapshot().engine.streaming_lifecycle == OutputLifecycle::Failed {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            worker.snapshot().engine.streaming_lifecycle,
            OutputLifecycle::Failed
        );
        assert!(matches!(
            worker.take_output_events().as_slice(),
            [OutputEvent::Failed { .. }]
        ));
    }

    #[test]
    fn stream_stop_returns_without_waiting_for_worker_teardown() {
        let worker = Arc::new(worker(1));
        let (entered_send, entered_receive) = mpsc::channel();
        let (release_send, release_receive) = mpsc::channel();
        worker
            .sender
            .send(WorkerCommand::TestBlock(entered_send, release_receive))
            .expect("block command");
        entered_receive.recv().expect("worker entered barrier");

        let requesting_worker = Arc::clone(&worker);
        let (result_send, result_receive) = mpsc::channel();
        thread::spawn(move || {
            let _ = result_send.send(requesting_worker.finish_streaming());
        });
        result_receive
            .recv_timeout(Duration::from_millis(250))
            .expect("stop must return while the worker remains blocked")
            .expect("stop request accepted");
        assert_eq!(
            worker.snapshot().engine.streaming_lifecycle,
            OutputLifecycle::Stopping
        );
        assert_eq!(worker.take_output_events(), vec![OutputEvent::Stopping]);

        release_send.send(()).expect("release worker");
        for _ in 0..100 {
            if worker.snapshot().engine.streaming_lifecycle == OutputLifecycle::Idle {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            worker.snapshot().engine.streaming_lifecycle,
            OutputLifecycle::Idle
        );
        assert_eq!(worker.take_output_events(), vec![OutputEvent::Stopped]);
    }

    #[test]
    fn output_lifecycle_events_stay_bounded() {
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let attempts = u32::try_from(OUTPUT_EVENT_CAPACITY).expect("small event capacity") + 10;
        for attempt in 0..attempts {
            push_output_event(&events, OutputEvent::Reconnecting { attempt });
        }
        let events = events.lock().expect("event queue");
        assert_eq!(events.len(), OUTPUT_EVENT_CAPACITY);
        assert_eq!(
            events.front(),
            Some(&OutputEvent::Reconnecting { attempt: 10 })
        );
    }

    #[test]
    fn gain_filter_updates_stay_on_the_worker_and_validate_bounds() {
        let worker = worker(1);
        worker
            .set_channel_gain_filter_db_milli(EngineAudioChannel::Microphone, -6_000)
            .expect("valid gain filter");
        worker
            .set_channel_invert_polarity(EngineAudioChannel::Microphone)
            .expect("valid invert polarity filter");
        worker
            .set_channel_limiter(EngineAudioChannel::Microphone, -6_000, 60)
            .expect("valid limiter filter");
        worker
            .set_channel_compressor(EngineAudioChannel::Microphone, 10_000, -18_000, 6, 60, 0)
            .expect("valid compressor filter");
        worker
            .set_channel_expander(EngineAudioChannel::Microphone, 10_000, -40_000, 10, 50, 0)
            .expect("valid expander filter");
        worker
            .set_channel_noise_gate(EngineAudioChannel::Microphone, 10_000, -40_000, 10, 125, 0)
            .expect("valid noise gate filter");
        let error = worker
            .set_channel_gain_filter_db_milli(EngineAudioChannel::Microphone, 30_001)
            .expect_err("unbounded gain filter");
        assert!(matches!(error, EngineError::Worker(reason) if reason.contains("outside")));
        let error = worker
            .set_channel_limiter(EngineAudioChannel::Microphone, -60_001, 60)
            .expect_err("unbounded limiter threshold");
        assert!(matches!(error, EngineError::Worker(reason) if reason.contains("threshold")));
        let error = worker
            .set_channel_compressor(EngineAudioChannel::Microphone, 32_001, -18_000, 6, 60, 0)
            .expect_err("unbounded compressor ratio");
        assert!(matches!(error, EngineError::Worker(reason) if reason.contains("ratio")));
        let error = worker
            .set_channel_expander(EngineAudioChannel::Microphone, 20_001, -40_000, 10, 50, 0)
            .expect_err("unbounded expander ratio");
        assert!(matches!(error, EngineError::Worker(reason) if reason.contains("ratio")));
        let error = worker
            .set_channel_noise_gate(EngineAudioChannel::Microphone, 20_001, -40_000, 10, 125, 0)
            .expect_err("unbounded noise gate ratio");
        assert!(matches!(error, EngineError::Worker(reason) if reason.contains("ratio")));
    }

    #[test]
    fn replay_buffer_controls_stay_on_the_worker() {
        let worker = worker(1);
        let path =
            std::env::temp_dir().join(format!("obs-rs-replay-worker-{}.obsr", std::process::id()));
        worker
            .start_replay_buffer(1024 * 1024, Duration::from_secs(5))
            .expect("start replay buffer");
        let format =
            VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
        assert!(worker.try_push_frame(VideoFrame::solid(
            format,
            Timestamp::ZERO,
            [0x20, 0x40, 0x80, 0xFF],
        )));
        for _ in 0..100 {
            if worker.snapshot().queued_frames == 0 {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let bytes = worker
            .save_replay_buffer(&path)
            .expect("save replay buffer");
        assert!(bytes > 16);
        worker.stop_replay_buffer();
        std::fs::remove_file(path).expect("remove replay file");
    }
}
