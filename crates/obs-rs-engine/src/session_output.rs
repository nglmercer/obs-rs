use std::{path::PathBuf, time::Duration};

use obs_rs_output::{
    recover_stale_packet_files, AtomicPacketFileWriter, AudioEncoderConfig, OutputProfile,
    ReconnectOutcome, ReplayBuffer, SegmentedPacketFileWriter, SegmentedRecordingPolicy,
    StreamMetrics, StreamState, StreamTarget, VideoEncoderConfig,
};
#[cfg(feature = "production-gstreamer")]
use obs_rs_output_gstreamer::{
    discover_interrupted_remux_candidates, recover_interrupted_remux_recording,
    GStreamerCapabilitySnapshot, GStreamerOutputSession, ProductionDestination,
    ProductionPipelinePlan, RemuxRecovery,
};

use super::{
    EngineError, EngineSession, OutputLifecycle, RecordingOutput, ReplaySaveStatus, StreamOutput,
};

#[allow(
    clippy::missing_errors_doc,
    reason = "output lifecycle methods share the documented EngineError boundary"
)]
impl EngineSession {
    /// Starts an atomic Matroska/MP4/MOV/FLV or `OBSRPKT1` recording based on
    /// `path`.
    ///
    /// The phase moves to `Starting` before any file work and settles on
    /// `Running` or `Failed`, so a caller that only sees the error still leaves
    /// an observable record of what happened behind.
    pub fn start_recording(&mut self, path: impl Into<PathBuf>) -> Result<(), EngineError> {
        self.start_recording_with_config(path.into(), None)
    }

    /// Starts a bounded split recording in numbered reference or native
    /// production container files.
    ///
    /// The supplied path is used as the base name; the reference writer
    /// publishes siblings such as `recording-0001.obsr`, while native
    /// production muxers publish fixed slots such as `recording-00001.mp4`.
    /// The policy bounds total segment count, target size, and target duration.
    ///
    /// # Errors
    ///
    /// Returns an error when a recording is already open, the base path does
    /// not match a supported container, the policy is invalid, or the first
    /// segment cannot be opened.
    pub fn start_segmented_recording(
        &mut self,
        path: impl Into<PathBuf>,
        policy: SegmentedRecordingPolicy,
    ) -> Result<(), EngineError> {
        self.start_segmented_recording_with_config(path.into(), policy, None)
    }

    /// Starts a segmented recording with explicit production encoder choices.
    ///
    /// The reference packet writer has no production encoder boundary and
    /// therefore ignores the pair. Native production containers negotiate it
    /// before the first segment is opened.
    ///
    /// # Errors
    ///
    /// Returns an error when the recording is already open, the policy or
    /// destination is invalid, or the selected production configuration is
    /// unavailable.
    pub fn start_segmented_recording_configured(
        &mut self,
        path: impl Into<PathBuf>,
        policy: SegmentedRecordingPolicy,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        let encoder_config = (video, audio);
        self.start_segmented_recording_with_config(path.into(), policy, Some(&encoder_config))
    }

    fn start_segmented_recording_with_config(
        &mut self,
        path: PathBuf,
        policy: SegmentedRecordingPolicy,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.recording.is_some() {
            return Err(EngineError::Busy("start recording"));
        }
        self.recording_lifecycle = OutputLifecycle::Starting;
        let result = self.open_segmented_recording(path, policy, encoder_config);
        match result {
            Ok(()) => {
                self.recording_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.recording_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    /// Starts a production recording with an explicit codec and encoder choice.
    ///
    /// # Errors
    ///
    /// Returns an error when the codec combination or implementation is not
    /// supported by the current runtime.
    pub fn start_recording_configured(
        &mut self,
        path: impl Into<PathBuf>,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        let encoder_config = (video, audio);
        self.start_recording_with_config(path.into(), Some(&encoder_config))
    }

    /// Starts an H.264/AAC Matroska recording that is automatically remuxed
    /// to the requested MP4 path when recording finishes.
    ///
    /// The active Matroska source remains in a hidden `.mkv.part` path until
    /// the native no-clobber remux succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error when the native remux capability, selected encoders,
    /// or destination path is unavailable.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_remux_recording(&mut self, path: impl Into<PathBuf>) -> Result<(), EngineError> {
        self.start_remux_recording_with_config(path.into(), None)
    }

    /// Starts automatic Matroska-to-MP4 recording with explicit encoders.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected encoders are unavailable or do not
    /// match the H.264/AAC remux profile.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_remux_recording_configured(
        &mut self,
        path: impl Into<PathBuf>,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        let encoder_config = (video, audio);
        self.start_remux_recording_with_config(path.into(), Some(&encoder_config))
    }

    /// Recovers an interrupted automatic remux beside an exact MP4 path.
    ///
    /// Recovery consumes a marked `<final>.mkv.part` only after the native
    /// bounded remux publishes the MP4. It refuses to replace an existing
    /// destination and is unavailable while this session is carrying media
    /// output.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Busy`] while recording or streaming, or a
    /// production-output error when the candidate cannot be remuxed.
    #[cfg(feature = "production-gstreamer")]
    pub fn recover_interrupted_remux_recording(
        &mut self,
        path: impl Into<PathBuf>,
    ) -> Result<RemuxRecovery, EngineError> {
        if self.recording.is_some() || self.streaming.is_some() {
            return Err(EngineError::Busy("recover an interrupted recording"));
        }
        recover_interrupted_remux_recording(path.into()).map_err(Into::into)
    }

    /// Discovers recoverable automatic-remux destinations in an idle directory.
    ///
    /// The native boundary applies hard directory and candidate limits. This
    /// method is intentionally a control-plane operation and refuses to run
    /// while media output is active.
    ///
    /// # Errors
    ///
    /// Returns `EngineError::Busy` while recording or streaming, or a typed
    /// native/filesystem error when the bounded scan cannot complete.
    #[cfg(feature = "production-gstreamer")]
    pub fn discover_interrupted_remux_candidates(
        &mut self,
        directory: impl Into<PathBuf>,
    ) -> Result<Vec<PathBuf>, EngineError> {
        if self.recording.is_some() || self.streaming.is_some() {
            return Err(EngineError::Busy("discover interrupted recordings"));
        }
        discover_interrupted_remux_candidates(directory.into()).map_err(Into::into)
    }

    /// Starts a production recording using an explicit versioned output
    /// profile. This is the engine boundary for profiles that share a file
    /// extension, such as normal and fragmented MP4.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile is unavailable, does not match the
    /// destination, or a recording is already open.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_recording_profile(
        &mut self,
        path: impl Into<PathBuf>,
        profile: OutputProfile,
    ) -> Result<(), EngineError> {
        self.start_recording_profile_with_config(path.into(), profile, None)
    }

    /// Starts a production recording using an explicit profile and encoder
    /// implementations.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile, codec, or encoder implementation is
    /// unavailable, the destination does not match, or a recording is open.
    #[cfg(feature = "production-gstreamer")]
    pub fn start_recording_profile_configured(
        &mut self,
        path: impl Into<PathBuf>,
        profile: OutputProfile,
        video: VideoEncoderConfig,
        audio: AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        let encoder_config = (video, audio);
        self.start_recording_profile_with_config(path.into(), profile, Some(&encoder_config))
    }

    fn start_recording_with_config(
        &mut self,
        path: PathBuf,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.recording.is_some() {
            return Err(EngineError::Busy("start recording"));
        }
        self.recording_lifecycle = OutputLifecycle::Starting;
        let result = self.open_recording(path, encoder_config, None);
        match result {
            Ok(()) => {
                self.recording_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.recording_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    #[cfg(feature = "production-gstreamer")]
    fn start_recording_profile_with_config(
        &mut self,
        path: PathBuf,
        profile: OutputProfile,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.recording.is_some() {
            return Err(EngineError::Busy("start recording"));
        }
        self.recording_lifecycle = OutputLifecycle::Starting;
        let result = self.open_recording(path, encoder_config, Some(profile));
        match result {
            Ok(()) => {
                self.recording_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.recording_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    #[cfg(feature = "production-gstreamer")]
    fn start_remux_recording_with_config(
        &mut self,
        path: PathBuf,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.recording.is_some() {
            return Err(EngineError::Busy("start recording"));
        }
        self.recording_lifecycle = OutputLifecycle::Starting;
        let result = self.open_remux_recording(path, encoder_config);
        match result {
            Ok(()) => {
                self.recording_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.recording_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn open_recording(
        &mut self,
        final_path: PathBuf,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
        profile_override: Option<OutputProfile>,
    ) -> Result<(), EngineError> {
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                EngineError::InvalidConfiguration("recording path must name a file".to_owned())
            })?;
        let extension = final_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if extension.eq_ignore_ascii_case("obsr") {
            if profile_override.is_some() {
                return Err(EngineError::InvalidConfiguration(
                    "production output profiles cannot target the .obsr reference container"
                        .to_owned(),
                ));
            }
            let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
            self.recording = Some(RecordingOutput::Reference(AtomicPacketFileWriter::new(
                final_path, temp_path,
            )?));
            return Ok(());
        }
        let is_mp4 = extension.eq_ignore_ascii_case("mp4");
        let is_mov = extension.eq_ignore_ascii_case("mov");
        let is_flv = extension.eq_ignore_ascii_case("flv");
        if !extension.eq_ignore_ascii_case("mkv") && !is_mp4 && !is_mov && !is_flv {
            return Err(EngineError::InvalidConfiguration(
                "recording extension must be .mkv, .mp4, .mov, .flv, or .obsr".to_owned(),
            ));
        }
        #[cfg(feature = "production-gstreamer")]
        {
            let destination = ProductionDestination::Recording(final_path.clone());
            if (is_mp4 || is_mov || is_flv)
                && encoder_config
                    .is_some_and(|(video, _)| video.codec != obs_rs_output::VideoCodec::H264)
            {
                return Err(EngineError::InvalidConfiguration(
                    "MP4, MOV, and FLV recording currently require H.264 video".to_owned(),
                ));
            }
            let profile = profile_override.unwrap_or_else(|| {
                if is_mp4 {
                    OutputProfile::mp4_h264_aac()
                } else if is_mov {
                    OutputProfile::mov_h264_aac()
                } else if is_flv {
                    OutputProfile::flv_h264_aac()
                } else {
                    encoder_config.map_or_else(OutputProfile::matroska_h264_aac, |config| {
                        match config.0.codec {
                            obs_rs_output::VideoCodec::H264 => OutputProfile::matroska_h264_aac(),
                            obs_rs_output::VideoCodec::Hevc => OutputProfile::matroska_hevc_aac(),
                            obs_rs_output::VideoCodec::Av1 => OutputProfile::matroska_av1_aac(),
                            _ => OutputProfile::reference(),
                        }
                    })
                }
            });
            self.open_native_production_recording(&destination, profile, encoder_config)
        }
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = (encoder_config, profile_override);
        #[cfg(not(feature = "production-gstreamer"))]
        Err(EngineError::InvalidConfiguration(
            "production recording support was not compiled into this host".to_owned(),
        ))
    }

    #[cfg(feature = "production-gstreamer")]
    fn open_remux_recording(
        &mut self,
        final_path: PathBuf,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if !final_path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
        {
            return Err(EngineError::InvalidConfiguration(
                "automatic remux destination must use the .mp4 extension".to_owned(),
            ));
        }
        let destination = ProductionDestination::RemuxRecording { final_path };
        self.open_native_production_recording(
            &destination,
            OutputProfile::matroska_h264_aac(),
            encoder_config,
        )
    }

    #[cfg(feature = "production-gstreamer")]
    fn open_native_production_recording(
        &mut self,
        destination: &ProductionDestination,
        profile: OutputProfile,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        let capabilities = GStreamerCapabilitySnapshot::probe_cached();
        let plan = encoder_config.map_or_else(
            || ProductionPipelinePlan::negotiate(profile, destination, &capabilities),
            |(video, audio)| {
                ProductionPipelinePlan::negotiate_configured(
                    profile,
                    destination,
                    &capabilities,
                    video,
                    audio,
                )
            },
        )?;
        let session = GStreamerOutputSession::start(
            &plan,
            destination,
            self.format,
            self.config.audio_format,
        )?;
        self.recording = Some(RecordingOutput::Production { session });
        Ok(())
    }

    fn open_segmented_recording(
        &mut self,
        base_path: PathBuf,
        policy: SegmentedRecordingPolicy,
        encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        let extension = base_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if extension.eq_ignore_ascii_case("obsr") {
            recover_stale_packet_files(&base_path)?;
            self.recording = Some(RecordingOutput::SegmentedReference(
                SegmentedPacketFileWriter::new(base_path, policy)?,
            ));
            return Ok(());
        }
        #[cfg(feature = "production-gstreamer")]
        {
            let is_mp4 = extension.eq_ignore_ascii_case("mp4");
            let is_mov = extension.eq_ignore_ascii_case("mov");
            let is_flv = extension.eq_ignore_ascii_case("flv");
            let is_matroska = extension.eq_ignore_ascii_case("mkv");
            if !is_matroska && !is_mp4 && !is_mov && !is_flv {
                return Err(EngineError::InvalidConfiguration(
                    "segmented recording base path must use .mkv, .mp4, .mov, .flv, or .obsr"
                        .to_owned(),
                ));
            }
            let destination = ProductionDestination::SegmentedRecording { base_path, policy };
            let profile = if is_mp4 {
                OutputProfile::mp4_h264_aac()
            } else if is_mov {
                OutputProfile::mov_h264_aac()
            } else if is_flv {
                OutputProfile::flv_h264_aac()
            } else {
                OutputProfile::matroska_h264_aac()
            };
            let capabilities = GStreamerCapabilitySnapshot::probe_cached();
            let plan = encoder_config.map_or_else(
                || ProductionPipelinePlan::negotiate(profile, &destination, &capabilities),
                |(video, audio)| {
                    ProductionPipelinePlan::negotiate_configured(
                        profile,
                        &destination,
                        &capabilities,
                        video,
                        audio,
                    )
                },
            )?;
            let session = GStreamerOutputSession::start(
                &plan,
                &destination,
                self.format,
                self.config.audio_format,
            )?;
            self.recording = Some(RecordingOutput::Production { session });
            Ok(())
        }
        #[cfg(not(feature = "production-gstreamer"))]
        {
            let _ = (base_path, policy, encoder_config);
            Err(EngineError::InvalidConfiguration(
                "production segmented recording support was not compiled into this host".to_owned(),
            ))
        }
    }

    /// Finalizes a recording and returns its committed byte count.
    ///
    /// A failed finalization leaves the recording open and the phase `Failed`,
    /// so the captured packets are not silently discarded and the frontend can
    /// see that the file was never committed.
    pub fn finish_recording(&mut self) -> Result<usize, EngineError> {
        let Some(mut recording) = self.recording.take() else {
            return Err(EngineError::InvalidConfiguration(
                "recording is not open".to_owned(),
            ));
        };
        self.recording_lifecycle = OutputLifecycle::Stopping;
        match recording.finalize() {
            Ok(bytes) => {
                self.recording_lifecycle = OutputLifecycle::Idle;
                Ok(bytes)
            }
            Err(error) => {
                self.recording = Some(recording);
                Err(self.fail_recording(error))
            }
        }
    }

    fn fail_recording(&mut self, error: EngineError) -> EngineError {
        self.recording_lifecycle = OutputLifecycle::Failed;
        self.last_error = Some(error.to_string());
        error
    }

    /// Aborts an open recording and removes its temporary path.
    pub fn abort_recording(&mut self) {
        if let Some(mut recording) = self.recording.take() {
            recording.abort();
        }
        // An abort is a deliberate stop, so it clears a previous failure rather
        // than leaving the session permanently marked as broken.
        self.recording_lifecycle = OutputLifecycle::Idle;
    }

    /// Starts a bounded packetized replay history.
    ///
    /// Replay capture is independent of recording and streaming. It reuses the
    /// selected packet encoders only while active, so an idle session does not
    /// pay an encode cost just to keep an empty buffer alive.
    pub fn start_replay_buffer(
        &mut self,
        capacity_bytes: usize,
        duration: Duration,
    ) -> Result<(), EngineError> {
        if self.replay_buffer.is_some() {
            return Err(EngineError::Busy("start replay buffer"));
        }
        self.replay_lifecycle = OutputLifecycle::Starting;
        self.replay_save_status = ReplaySaveStatus::Idle;
        match ReplayBuffer::new(capacity_bytes, duration) {
            Ok(buffer) => {
                self.replay_buffer = Some(buffer);
                self.replay_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.replay_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error.into())
            }
        }
    }

    /// Stops replay capture and discards its retained packet history.
    pub fn stop_replay_buffer(&mut self) {
        self.replay_lifecycle = OutputLifecycle::Stopping;
        self.replay_buffer = None;
        self.replay_lifecycle = OutputLifecycle::Idle;
        self.replay_save_status = ReplaySaveStatus::Idle;
    }

    /// Saves the retained replay packets through the atomic packet writer.
    ///
    /// The replay history remains active after a successful save, matching the
    /// OBS workflow where saving a replay does not stop capture. The packet
    /// container is the inspectable OBS-RS reference container; production
    /// remuxing remains a separate output capability.
    pub fn save_replay_buffer(&mut self, path: impl Into<PathBuf>) -> Result<usize, EngineError> {
        self.replay_save_status = ReplaySaveStatus::Saving;
        let result = self.write_replay_buffer(path.into());
        match result {
            Ok(bytes) => {
                self.replay_save_status = ReplaySaveStatus::Saved { bytes };
                Ok(bytes)
            }
            Err(error) => {
                self.replay_save_status = ReplaySaveStatus::Failed {
                    reason: error.to_string(),
                };
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn write_replay_buffer(&self, final_path: PathBuf) -> Result<usize, EngineError> {
        let Some(buffer) = self.replay_buffer.as_ref() else {
            return Err(EngineError::InvalidConfiguration(
                "replay buffer is not running".to_owned(),
            ));
        };
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                EngineError::InvalidConfiguration("replay path must name a file".to_owned())
            })?;
        let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
        let packets = buffer.keyframe_aligned_snapshot()?;
        let mut writer = AtomicPacketFileWriter::new(final_path, temp_path)?;
        if let Err(error) = packets
            .into_iter()
            .try_for_each(|packet| writer.push(packet))
        {
            let _ = writer.abort();
            return Err(error.into());
        }
        writer.finalize().map_err(Into::into)
    }

    /// Returns whether replay capture is currently active.
    #[must_use]
    pub const fn is_replay_buffer_active(&self) -> bool {
        self.replay_buffer.is_some()
    }

    /// Returns the explicit replay-capture phase.
    #[must_use]
    pub const fn replay_lifecycle(&self) -> OutputLifecycle {
        self.replay_lifecycle
    }

    /// Returns the latest replay save status.
    #[must_use]
    pub const fn replay_save_status(&self) -> &ReplaySaveStatus {
        &self.replay_save_status
    }

    /// Returns the number of packetized entries retained for replay.
    #[must_use]
    pub fn replay_buffer_packet_count(&self) -> usize {
        self.replay_buffer.as_ref().map_or(0, ReplayBuffer::len)
    }

    /// Opens a TCP or WebSocket OBS-RS packet stream.
    ///
    /// A refused or unreachable peer leaves the phase `Failed`, which is what
    /// distinguishes "the user never started a stream" from "the stream could
    /// not be established".
    pub fn start_streaming(&mut self, address: &str) -> Result<(), EngineError> {
        self.start_streaming_with_config(address, None)
    }

    /// Opens a stream with explicit production encoder choices.
    pub fn start_streaming_configured(
        &mut self,
        address: &str,
        video: &VideoEncoderConfig,
        audio: &AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        self.start_streaming_with_config(address, Some((video, audio)))
    }

    /// Opens a semantic production target without flattening credentials into a URL.
    pub fn start_streaming_target_configured(
        &mut self,
        target: &StreamTarget,
        video: &VideoEncoderConfig,
        audio: &AudioEncoderConfig,
    ) -> Result<(), EngineError> {
        if self.streaming.is_some() {
            return Err(EngineError::Busy("start streaming"));
        }
        self.streaming_lifecycle = OutputLifecycle::Starting;
        #[cfg(feature = "production-gstreamer")]
        let result = StreamOutput::connect_target(
            target,
            self.config.output_queue_bytes,
            self.config.reconnect_attempts,
            self.format,
            self.config.audio_format,
            video,
            audio,
        );
        #[cfg(not(feature = "production-gstreamer"))]
        let result = target
            .endpoint()
            .ok_or_else(|| {
                EngineError::InvalidConfiguration("stream target is incomplete".to_owned())
            })
            .and_then(|address| {
                StreamOutput::connect(
                    &address,
                    self.config.output_queue_bytes,
                    self.config.reconnect_attempts,
                    self.format,
                    self.config.audio_format,
                    Some((video, audio)),
                )
            });
        match result {
            Ok(stream) => {
                self.streaming = Some(stream);
                self.streaming_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.streaming_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn start_streaming_with_config(
        &mut self,
        address: &str,
        encoder_config: Option<(&VideoEncoderConfig, &AudioEncoderConfig)>,
    ) -> Result<(), EngineError> {
        if self.streaming.is_some() {
            return Err(EngineError::Busy("start streaming"));
        }
        self.streaming_lifecycle = OutputLifecycle::Starting;
        match StreamOutput::connect(
            address,
            self.config.output_queue_bytes,
            self.config.reconnect_attempts,
            self.format,
            self.config.audio_format,
            encoder_config,
        ) {
            Ok(stream) => {
                self.streaming = Some(stream);
                self.streaming_lifecycle = OutputLifecycle::Running;
                Ok(())
            }
            Err(error) => {
                self.streaming_lifecycle = OutputLifecycle::Failed;
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    /// Flushes queued packets without making the media producer wait during
    /// [`EngineSession::tick`].
    pub fn pump_stream(&mut self) -> Result<usize, EngineError> {
        let Some(stream) = self.streaming.as_mut() else {
            return Ok(0);
        };
        if stream.state() == StreamState::Disconnected {
            match stream.reconnect() {
                Ok(ReconnectOutcome::Reconnected) => {
                    self.streaming_lifecycle = OutputLifecycle::Running;
                }
                Ok(ReconnectOutcome::Deferred { .. }) => return Ok(0),
                Err(error) => {
                    self.last_error = Some(error.to_string());
                    self.streaming_lifecycle = OutputLifecycle::Failed;
                    return Err(error);
                }
            }
        }
        match stream.pump() {
            Ok(sent) => Ok(sent),
            Err(error) => {
                self.last_error = Some(error.to_string());
                match stream.reconnect() {
                    Ok(ReconnectOutcome::Reconnected) => {
                        // The transport is carrying media again, so the phase
                        // must say so. Leaving it at `Failed` from the attempt
                        // that just recovered would show a stopped stream in
                        // the UI while packets were flowing.
                        self.streaming_lifecycle = OutputLifecycle::Running;
                        Ok(0)
                    }
                    Ok(ReconnectOutcome::Deferred { .. }) => Ok(0),
                    Err(reconnect) => {
                        self.last_error = Some(format!("{error}; reconnect failed: {reconnect}"));
                        // A pump error the transport could not recover from is
                        // the point the stream stops carrying media, whether or
                        // not the handle is still open.
                        self.streaming_lifecycle = OutputLifecycle::Failed;
                        Err(error)
                    }
                }
            }
        }
    }

    /// Stops streaming and closes its transport.
    pub fn finish_streaming(&mut self) -> Result<(), EngineError> {
        let Some(mut stream) = self.streaming.take() else {
            // Stopping a stream that never started still clears a failed start,
            // so a retry is not blocked by the previous attempt's phase.
            self.streaming_lifecycle = OutputLifecycle::Idle;
            return Ok(());
        };
        self.streaming_lifecycle = OutputLifecycle::Stopping;
        let _ = stream.pump();
        stream.close()?;
        self.streaming_lifecycle = OutputLifecycle::Idle;
        Ok(())
    }

    /// Returns the explicit recording phase, including a failed start or commit.
    #[must_use]
    pub const fn recording_lifecycle(&self) -> OutputLifecycle {
        self.recording_lifecycle
    }

    /// Returns the explicit streaming phase, including a failed connect.
    #[must_use]
    pub const fn streaming_lifecycle(&self) -> OutputLifecycle {
        self.streaming_lifecycle
    }

    /// Returns whether a packet recording is open.
    #[must_use]
    pub const fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Returns whether a stream transport is open.
    #[must_use]
    pub const fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }

    /// Returns the most recent stream state.
    #[must_use]
    pub fn stream_state(&self) -> Option<StreamState> {
        self.streaming.as_ref().map(StreamOutput::state)
    }

    /// Returns stream counters and queued bytes.
    #[must_use]
    pub fn stream_metrics(&self) -> Option<(StreamMetrics, usize)> {
        self.streaming.as_ref().and_then(|stream| {
            stream
                .metrics()
                .map(|metrics| (metrics, stream.queued_bytes()))
        })
    }
}
