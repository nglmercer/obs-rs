use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use obs_rs_media::VideoFrame;

use obs_rs_project::Project;

use crate::{EngineError, EngineSession, EngineSnapshot, EngineStats, OutputLifecycle};

const DEFAULT_FRAME_QUEUE: usize = 8;

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
    StartRecording(PathBuf, mpsc::Sender<Result<(), String>>),
    FinishRecording(mpsc::Sender<Result<usize, String>>),
    AbortRecording,
    StartStreaming(String, mpsc::Sender<Result<(), String>>),
    FinishStreaming,
    PushFrame(VideoFrame),
    SetGain(u16, mpsc::Sender<Result<(), String>>),
    SetMuted(bool, mpsc::Sender<Result<(), String>>),
    SetAudioInput(Option<String>, mpsc::Sender<Result<(), String>>),
    SyncProject(Project, mpsc::Sender<Result<(), String>>),
    Shutdown,
}

/// Bounded background owner for engine media work.
pub struct EngineWorker {
    sender: SyncSender<WorkerCommand>,
    snapshot: Arc<Mutex<EngineWorkerSnapshot>>,
    dropped_frames: Arc<AtomicU64>,
    queued_frames: Arc<AtomicUsize>,
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
        let thread_snapshot = Arc::clone(&snapshot);
        let thread_dropped = Arc::clone(&dropped_frames);
        let thread_queued = Arc::clone(&queued_frames);
        let join = thread::Builder::new()
            .name("obs-rs-engine".to_owned())
            .spawn(move || {
                worker_loop(
                    session,
                    receiver,
                    thread_snapshot,
                    thread_dropped,
                    thread_queued,
                );
            })
            .map_err(EngineError::Io)?;
        Ok(Self {
            sender,
            snapshot,
            dropped_frames,
            queued_frames,
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
                    stream_metrics: None,
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
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::StartRecording(path.into(), reply))
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

    /// Starts a TCP/WebSocket stream on the worker thread.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when transport setup fails or the worker closes.
    pub fn start_streaming(&self, address: &str) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::StartStreaming(address.to_owned(), reply))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Stops streaming on the worker thread.
    pub fn finish_streaming(&self) {
        let _ = self.sender.send(WorkerCommand::FinishStreaming);
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

    /// Applies live input gain on the worker-owned mixer.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker or mixer rejects the update.
    pub fn set_input_gain_milli(&self, gain_milli: u16) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetGain(gain_milli, reply))
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
    pub fn set_input_muted(&self, muted: bool) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::SetMuted(muted, reply))
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
fn worker_loop(
    mut session: EngineSession,
    receiver: Receiver<WorkerCommand>,
    snapshot: Arc<Mutex<EngineWorkerSnapshot>>,
    dropped_frames: Arc<AtomicU64>,
    queued_frames: Arc<AtomicUsize>,
) {
    while let Ok(command) = receiver.recv() {
        let shutdown = match command {
            WorkerCommand::StartRecording(path, reply) => {
                let result = session
                    .start_recording(path)
                    .map_err(|error| error.to_string());
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
            WorkerCommand::AbortRecording => {
                session.abort_recording();
                false
            }
            WorkerCommand::StartStreaming(address, reply) => {
                let result = session
                    .start_streaming(&address)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::FinishStreaming => {
                let _ = session.finish_streaming();
                false
            }
            WorkerCommand::PushFrame(frame) => {
                queued_frames.fetch_sub(1, Ordering::Relaxed);
                if let Err(error) = session.push_program_frame(&frame) {
                    session.last_error = Some(error.to_string());
                }
                false
            }
            WorkerCommand::SetGain(gain, reply) => {
                let result = session
                    .set_input_gain_milli(gain)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetMuted(muted, reply) => {
                let result = session
                    .set_input_muted(muted)
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
            WorkerCommand::Shutdown => true,
        };

        if session.is_streaming() {
            if let Err(error) = session.pump_stream() {
                session.last_error = Some(error.to_string());
            }
        }
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
