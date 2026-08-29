use std::{path::PathBuf, sync::mpsc, time::Duration};

use crate::RemuxRecovery;
use obs_rs_output::OutputProfile;
use obs_rs_output::{
    AudioEncoderConfig, SegmentedRecordingPolicy, StreamTarget, VideoEncoderConfig,
};

use super::worker_runtime::{
    command_enqueue_error, push_output_event, set_recording_lifecycle, set_replay_lifecycle,
    set_replay_save_status, set_streaming_lifecycle, worker_closed,
};
use super::{
    EngineError, EngineWorker, OutputEvent, OutputLifecycle, ReplaySaveStatus, WorkerCommand,
};

impl EngineWorker {
    /// Requests a recording start and waits for the worker to validate it.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker is closed or the engine rejects
    /// the recording.
    pub fn start_recording(&self, path: impl Into<PathBuf>) -> Result<(), EngineError> {
        self.start_recording_with_config(path.into(), None)
    }

    /// Requests a bounded split recording start and waits for the worker to
    /// validate the first segment.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker is closed, the policy is
    /// invalid, or the first segment cannot be opened.
    pub fn start_segmented_recording(
        &self,
        path: impl Into<PathBuf>,
        policy: SegmentedRecordingPolicy,
    ) -> Result<(), EngineError> {
        self.start_segmented_recording_with_config(path.into(), policy, None)
    }

    /// Requests a segmented recording with explicit production encoder
    /// choices. The reference packet path ignores the pair.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker is closed or the selected
    /// production configuration cannot open the first segment.
    pub fn start_segmented_recording_configured(
        &self,
        path: impl Into<PathBuf>,
        policy: SegmentedRecordingPolicy,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        self.start_segmented_recording_with_config(path.into(), policy, Some((video, audio)))
    }

    fn start_segmented_recording_with_config(
        &self,
        path: PathBuf,
        policy: SegmentedRecordingPolicy,
        encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::StartSegmentedRecording(
                path,
                policy,
                encoder_config,
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
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

    /// Requests an automatic H.264/AAC Matroska-to-MP4 recording start.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker is closed or the native remux
    /// boundary rejects the destination.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_remux_recording(&self, path: impl Into<PathBuf>) -> Result<(), EngineError> {
        self.start_remux_recording_with_config(path.into(), None)
    }

    /// Requests automatic remux recording with explicit encoder choices.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker is closed or the encoders do
    /// not match the native H.264/AAC remux profile.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_remux_recording_configured(
        &self,
        path: impl Into<PathBuf>,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        self.start_remux_recording_with_config(path.into(), Some((video, audio)))
    }

    /// Recovers an interrupted automatic remux on the worker thread.
    ///
    /// This synchronous convenience method is for control-plane callers that
    /// are already off the UI and media submission paths. GUI callers should
    /// use [`Self::try_recover_interrupted_remux_recording`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker closes or the native recovery
    /// rejects the path or candidate.
    #[cfg(feature = "production-gstreamer")]
    pub fn recover_interrupted_remux_recording(
        &self,
        path: impl Into<PathBuf>,
    ) -> Result<RemuxRecovery, EngineError> {
        let receive = self.try_recover_interrupted_remux_recording(path)?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Enqueues an interrupted automatic-remux recovery without waiting for
    /// native demuxing or file publication.
    ///
    /// The returned one-shot receiver is bounded to one result. The request
    /// itself is accepted only when the worker command queue has capacity.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the bounded worker queue is full or closed.
    #[cfg(feature = "production-gstreamer")]
    pub fn try_recover_interrupted_remux_recording(
        &self,
        path: impl Into<PathBuf>,
    ) -> Result<mpsc::Receiver<Result<RemuxRecovery, String>>, EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .try_send(WorkerCommand::RecoverInterruptedRemux(path.into(), reply))
            .map_err(|error| command_enqueue_error(&error))?;
        Ok(receive)
    }

    /// Discovers recoverable automatic-remux destinations without waiting for
    /// the worker or filesystem scan.
    ///
    /// The returned one-shot receiver is bounded to one result.
    ///
    /// # Errors
    ///
    /// Returns `EngineError` when the bounded worker command queue is full or
    /// closed.
    #[cfg(feature = "production-gstreamer")]
    pub fn try_discover_interrupted_remux_candidates(
        &self,
        directory: impl Into<PathBuf>,
    ) -> Result<mpsc::Receiver<Result<Vec<PathBuf>, String>>, EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .try_send(WorkerCommand::DiscoverInterruptedRemux(
                directory.into(),
                reply,
            ))
            .map_err(|error| command_enqueue_error(&error))?;
        Ok(receive)
    }

    /// Discovers recoverable automatic-remux destinations synchronously.
    ///
    /// This convenience method is for control-plane callers already off UI and
    /// media submission paths. GUI callers should use the try_ variant.
    ///
    /// # Errors
    ///
    /// Returns `EngineError` when the worker closes or the bounded scan fails.
    #[cfg(feature = "production-gstreamer")]
    pub fn discover_interrupted_remux_candidates(
        &self,
        directory: impl Into<PathBuf>,
    ) -> Result<Vec<PathBuf>, EngineError> {
        self.try_discover_interrupted_remux_candidates(directory)?
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Requests a production recording using an explicit versioned profile.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the worker is closed or the selected
    /// profile cannot be opened by the engine.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_recording_profile(
        &self,
        path: impl Into<PathBuf>,
        profile: OutputProfile,
        encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::StartRecordingProfile(
                path.into(),
                profile,
                encoder_config,
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
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

    #[cfg(feature = "production-gstreamer")]
    fn start_remux_recording_with_config(
        &self,
        path: PathBuf,
        encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        let (reply, receive) = mpsc::channel();
        self.sender
            .send(WorkerCommand::StartRemuxRecording(
                path,
                encoder_config,
                reply,
            ))
            .map_err(|_| worker_closed())?;
        receive
            .recv()
            .map_err(|_| worker_closed())?
            .map_err(EngineError::Worker)
    }

    /// Enqueues recording setup without waiting for file or encoder work.
    ///
    /// The GUI uses this boundary so opening a container or negotiating a
    /// production encoder cannot block its event thread. The lifecycle is
    /// published as `Starting` immediately; the worker publishes `Running` or
    /// `Failed` after it processes the command.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] only when the bounded command queue rejects the
    /// request or the worker has already closed.
    pub fn try_start_recording(
        &self,
        path: impl Into<PathBuf>,
        encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        set_recording_lifecycle(&self.snapshot, OutputLifecycle::Starting);
        let (reply, _receive) = mpsc::channel();
        match self.sender.try_send(WorkerCommand::StartRecording(
            path.into(),
            encoder_config,
            reply,
        )) {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = command_enqueue_error(&error);
                set_recording_lifecycle(&self.snapshot, OutputLifecycle::Failed);
                Err(error)
            }
        }
    }

    /// Enqueues automatic Matroska-to-MP4 recording without waiting for
    /// native pipeline setup.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] only when the bounded command queue rejects the
    /// request or the worker has already closed.
    #[cfg(feature = "production-gstreamer")]
    pub fn try_start_remux_recording(
        &self,
        path: impl Into<PathBuf>,
        encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        set_recording_lifecycle(&self.snapshot, OutputLifecycle::Starting);
        let (reply, _receive) = mpsc::channel();
        match self.sender.try_send(WorkerCommand::StartRemuxRecording(
            path.into(),
            encoder_config,
            reply,
        )) {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = command_enqueue_error(&error);
                set_recording_lifecycle(&self.snapshot, OutputLifecycle::Failed);
                Err(error)
            }
        }
    }

    /// Enqueues a production recording with an explicit profile without
    /// waiting for container or encoder setup.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] only when the bounded command queue rejects the
    /// request or the worker has already closed.
    #[cfg(feature = "production-gstreamer")]
    pub fn try_start_recording_profile(
        &self,
        path: impl Into<PathBuf>,
        profile: OutputProfile,
        encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        set_recording_lifecycle(&self.snapshot, OutputLifecycle::Starting);
        let (reply, _receive) = mpsc::channel();
        match self.sender.try_send(WorkerCommand::StartRecordingProfile(
            path.into(),
            profile,
            encoder_config,
            reply,
        )) {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = command_enqueue_error(&error);
                set_recording_lifecycle(&self.snapshot, OutputLifecycle::Failed);
                Err(error)
            }
        }
    }

    /// Enqueues a bounded split recording without waiting for segment-file
    /// setup on the caller's thread.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] only when the bounded command queue rejects the
    /// request or the worker has already closed.
    pub fn try_start_segmented_recording(
        &self,
        path: impl Into<PathBuf>,
        policy: SegmentedRecordingPolicy,
    ) -> Result<(), EngineError> {
        self.try_start_segmented_recording_with_config(path.into(), policy, None)
    }

    /// Enqueues a segmented recording with explicit production encoder
    /// choices without waiting for worker-side setup.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the bounded command queue rejects the
    /// request or the worker has already closed.
    pub fn try_start_segmented_recording_configured(
        &self,
        path: impl Into<PathBuf>,
        policy: SegmentedRecordingPolicy,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        self.try_start_segmented_recording_with_config(path.into(), policy, Some((video, audio)))
    }

    fn try_start_segmented_recording_with_config(
        &self,
        path: PathBuf,
        policy: SegmentedRecordingPolicy,
        encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        set_recording_lifecycle(&self.snapshot, OutputLifecycle::Starting);
        let (reply, _receive) = mpsc::channel();
        match self.sender.try_send(WorkerCommand::StartSegmentedRecording(
            path,
            policy,
            encoder_config,
            reply,
        )) {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = command_enqueue_error(&error);
                set_recording_lifecycle(&self.snapshot, OutputLifecycle::Failed);
                Err(error)
            }
        }
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

    /// Enqueues recording finalization without waiting for container work.
    ///
    /// The lifecycle becomes `Stopping` immediately and settles to `Idle` or
    /// `Failed` when the worker has finalized the file.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] only when the bounded command queue rejects the
    /// request or the worker has already closed.
    pub fn try_finish_recording(&self) -> Result<(), EngineError> {
        set_recording_lifecycle(&self.snapshot, OutputLifecycle::Stopping);
        let (reply, _receive) = mpsc::channel();
        match self.sender.try_send(WorkerCommand::FinishRecording(reply)) {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = command_enqueue_error(&error);
                set_recording_lifecycle(&self.snapshot, OutputLifecycle::Failed);
                Err(error)
            }
        }
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

    /// Enqueues replay capture without waiting for worker-side allocation.
    ///
    /// The replay lifecycle is published as `Starting` immediately and is
    /// settled by the worker after it validates the bounded buffer.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the bounded worker queue is full or closed.
    pub fn try_start_replay_buffer(
        &self,
        capacity_bytes: usize,
        duration: Duration,
    ) -> Result<(), EngineError> {
        set_replay_lifecycle(&self.snapshot, OutputLifecycle::Starting);
        let (reply, _receive) = mpsc::channel();
        match self.sender.try_send(WorkerCommand::StartReplayBuffer(
            capacity_bytes,
            duration,
            reply,
        )) {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = command_enqueue_error(&error);
                set_replay_lifecycle(&self.snapshot, OutputLifecycle::Failed);
                Err(error)
            }
        }
    }

    /// Stops replay capture and discards its worker-owned history.
    pub fn stop_replay_buffer(&self) {
        let _ = self.sender.send(WorkerCommand::StopReplayBuffer);
    }

    /// Enqueues replay teardown without waiting for worker-side cleanup.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the bounded worker queue is full or closed.
    pub fn try_stop_replay_buffer(&self) -> Result<(), EngineError> {
        let previous = self.snapshot().engine.replay_lifecycle;
        set_replay_lifecycle(&self.snapshot, OutputLifecycle::Stopping);
        match self.sender.try_send(WorkerCommand::StopReplayBuffer) {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = command_enqueue_error(&error);
                set_replay_lifecycle(&self.snapshot, previous);
                Err(error)
            }
        }
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

    /// Enqueues an atomic replay save without waiting for snapshot or file I/O.
    ///
    /// The save status is published as `Saving` immediately and settles to
    /// `Saved` or `Failed` when the worker finishes the request.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the bounded worker queue is full or closed.
    pub fn try_save_replay_buffer(&self, path: impl Into<PathBuf>) -> Result<(), EngineError> {
        set_replay_save_status(&self.snapshot, ReplaySaveStatus::Saving);
        let (reply, _receive) = mpsc::channel();
        match self
            .sender
            .try_send(WorkerCommand::SaveReplayBuffer(path.into(), reply))
        {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = command_enqueue_error(&error);
                set_replay_save_status(
                    &self.snapshot,
                    ReplaySaveStatus::Failed {
                        reason: error.to_string(),
                    },
                );
                Err(error)
            }
        }
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
        match self
            .sender
            .try_send(WorkerCommand::StartStreamingTarget(target, video, audio))
        {
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
}

#[cfg(not(feature = "production-gstreamer"))]
#[allow(
    clippy::missing_errors_doc,
    reason = "portable stubs all return the same explicit optional-runtime error"
)]
impl EngineWorker {
    /// Reports that automatic remux is unavailable in the portable build.
    pub fn start_remux_recording(&self, _path: impl Into<PathBuf>) -> Result<(), EngineError> {
        Err(native_output_unavailable())
    }

    /// Reports that automatic remux is unavailable in the portable build.
    pub fn start_remux_recording_configured(
        &self,
        _path: impl Into<PathBuf>,
        _video: VideoEncoderConfig,
        _audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        Err(native_output_unavailable())
    }

    /// Reports that interrupted-remux recovery is unavailable in the portable
    /// build.
    pub fn recover_interrupted_remux_recording(
        &self,
        _path: impl Into<PathBuf>,
    ) -> Result<RemuxRecovery, EngineError> {
        Err(native_output_unavailable())
    }

    /// Reports that interrupted-remux recovery is unavailable in the portable
    /// build without starting a worker request.
    pub fn try_recover_interrupted_remux_recording(
        &self,
        _path: impl Into<PathBuf>,
    ) -> Result<mpsc::Receiver<Result<RemuxRecovery, String>>, EngineError> {
        Err(native_output_unavailable())
    }

    /// Reports that interrupted-remux discovery is unavailable in the portable
    /// build.
    pub fn discover_interrupted_remux_candidates(
        &self,
        _directory: impl Into<PathBuf>,
    ) -> Result<Vec<PathBuf>, EngineError> {
        Err(native_output_unavailable())
    }

    /// Reports that interrupted-remux discovery is unavailable in the portable
    /// build without starting a worker request.
    pub fn try_discover_interrupted_remux_candidates(
        &self,
        _directory: impl Into<PathBuf>,
    ) -> Result<mpsc::Receiver<Result<Vec<PathBuf>, String>>, EngineError> {
        Err(native_output_unavailable())
    }

    /// Reports that explicit production profiles are unavailable in the
    /// portable build.
    pub fn start_recording_profile(
        &self,
        _path: impl Into<PathBuf>,
        _profile: OutputProfile,
        _encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        Err(native_output_unavailable())
    }

    /// Reports that automatic remux is unavailable in the portable build.
    pub fn try_start_remux_recording(
        &self,
        _path: impl Into<PathBuf>,
        _encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        Err(native_output_unavailable())
    }

    /// Reports that explicit production profiles are unavailable in the
    /// portable build.
    pub fn try_start_recording_profile(
        &self,
        _path: impl Into<PathBuf>,
        _profile: OutputProfile,
        _encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        Err(native_output_unavailable())
    }
}

#[cfg(not(feature = "production-gstreamer"))]
fn native_output_unavailable() -> EngineError {
    EngineError::InvalidConfiguration(
        "production GStreamer output is unavailable in this build".to_owned(),
    )
}
