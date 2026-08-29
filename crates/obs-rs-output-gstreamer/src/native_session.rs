use std::{fs, mem::size_of, path::PathBuf, time::Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use obs_rs_audio::{AudioBuffer, AudioFormat};
use obs_rs_media::{PixelFormat, RawVideoFrame, Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::{
    OutputTransport, ReconnectOutcome, ReconnectPolicy, SegmentedRecordingPolicy,
    StreamingTransport,
};

use super::super::capabilities::configure_bundled_runtime;
use super::super::{GStreamerError, ProductionDestination, ProductionPipelinePlan};
use super::{
    appsrc, configure_encoders, configure_segmented_location_callback, configure_sink,
    configure_sources, native_error, pipeline_description, publish_recording_artifact,
    publish_segmented_recording, recover_stale_recording_artifact, recover_stale_remux_manifest,
    recover_stale_segment_artifacts, remux_matroska_to_mp4, video_caps,
    write_interrupted_remux_manifest, NativeOutputState, PipelineDescription,
};

const LOCAL_PIPELINE_START_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(5);

/// Timestamp, queue/drop, reconnect, drift, keyframe, and submit timing data.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OutputSessionTelemetry {
    video_submitted: u64,
    audio_submitted: u64,
    dropped: u64,
    reconnects: u64,
    keyframes: u64,
    video_queue_bytes: u64,
    audio_queue_bytes: u64,
    last_video_timestamp: Option<Timestamp>,
    last_audio_timestamp: Option<Timestamp>,
    audio_drift_nanos: i128,
    total_submit_latency_nanos: u128,
    max_submit_latency_nanos: u128,
}

impl OutputSessionTelemetry {
    #[must_use]
    pub const fn video_submitted(self) -> u64 {
        self.video_submitted
    }
    #[must_use]
    pub const fn audio_submitted(self) -> u64 {
        self.audio_submitted
    }
    #[must_use]
    pub const fn dropped(self) -> u64 {
        self.dropped
    }
    #[must_use]
    pub const fn reconnects(self) -> u64 {
        self.reconnects
    }
    #[must_use]
    pub const fn keyframes(self) -> u64 {
        self.keyframes
    }
    #[must_use]
    pub const fn video_queue_bytes(self) -> u64 {
        self.video_queue_bytes
    }
    #[must_use]
    pub const fn audio_queue_bytes(self) -> u64 {
        self.audio_queue_bytes
    }
    #[must_use]
    pub const fn audio_drift_nanos(self) -> i128 {
        self.audio_drift_nanos
    }
    #[must_use]
    pub const fn total_submit_latency_nanos(self) -> u128 {
        self.total_submit_latency_nanos
    }
    #[must_use]
    pub const fn max_submit_latency_nanos(self) -> u128 {
        self.max_submit_latency_nanos
    }
}

/// Native `appsrc` session. Queue elements isolate app submission,
/// encoder/mux work, and transport I/O on bounded streaming tasks.
pub struct GStreamerOutputSession {
    pipeline: gst::Pipeline,
    pub(super) video: gst_app::AppSrc,
    pub(super) audio: gst_app::AppSrc,
    state: NativeOutputState,
    telemetry: OutputSessionTelemetry,
    committed_bytes: Option<usize>,
    final_path: Option<PathBuf>,
    temp_path: Option<PathBuf>,
    remux_final_path: Option<PathBuf>,
    segmented_policy: Option<SegmentedRecordingPolicy>,
    transport: OutputTransport,
    video_duration: gst::ClockTime,
    reconnect_policy: ReconnectPolicy,
    reconnect_attempts: u32,
    next_reconnect_at: Option<Instant>,
    video_format: VideoFormat,
    video_pixel_format: PixelFormat,
}

impl GStreamerOutputSession {
    /// Builds and starts an approved production pipeline.
    ///
    /// # Errors
    ///
    /// Returns a typed runtime/pipeline error before accepting media.
    pub fn start(
        plan: &ProductionPipelinePlan,
        destination: &ProductionDestination,
        video_format: VideoFormat,
        audio_format: AudioFormat,
    ) -> Result<Self, GStreamerError> {
        Self::start_with_reconnect_limit(plan, destination, video_format, audio_format, 3)
    }

    /// Builds a production pipeline with an explicit live reconnect budget.
    ///
    /// # Errors
    ///
    /// Returns a typed runtime/pipeline error before accepting media.
    pub fn start_with_reconnect_limit(
        plan: &ProductionPipelinePlan,
        destination: &ProductionDestination,
        video_format: VideoFormat,
        audio_format: AudioFormat,
        maximum_reconnects: u32,
    ) -> Result<Self, GStreamerError> {
        Self::start_with_reconnect_policy(
            plan,
            destination,
            video_format,
            audio_format,
            ReconnectPolicy::new(maximum_reconnects),
        )
    }

    /// Builds a production pipeline with an explicit reconnect policy.
    ///
    /// # Errors
    ///
    /// Returns a typed runtime/pipeline error before accepting media.
    pub fn start_with_reconnect_policy(
        plan: &ProductionPipelinePlan,
        destination: &ProductionDestination,
        video_format: VideoFormat,
        audio_format: AudioFormat,
        reconnect_policy: ReconnectPolicy,
    ) -> Result<Self, GStreamerError> {
        configure_bundled_runtime();
        gst::init().map_err(native_error)?;
        destination.validate_for(plan.profile())?;
        let pipeline_description = pipeline_description(plan, destination)?;
        let PipelineDescription {
            description,
            final_path,
            temp_path,
            remux_final_path,
            segmented_policy,
        } = pipeline_description;
        recover_stale_recording_artifact(temp_path.as_deref())?;
        if let Some(final_path) = remux_final_path.as_deref() {
            recover_stale_recording_artifact(Some(&final_path.with_extension("mp4.part")))?;
            recover_stale_remux_manifest(final_path)?;
        }
        if let (Some(base_path), Some(policy)) = (final_path.as_deref(), segmented_policy) {
            recover_stale_segment_artifacts(base_path, policy)?;
        }
        let element = gst::parse::launch_full(&description, None, gst::ParseFlags::FATAL_ERRORS)
            .map_err(native_error)?;
        let pipeline = element.downcast::<gst::Pipeline>().map_err(|_| {
            GStreamerError::Native("GStreamer did not create a pipeline".to_owned())
        })?;
        if plan.profile().transport() != OutputTransport::WebRtc {
            configure_encoders(&pipeline, plan, video_format)?;
        }
        configure_sink(&pipeline, destination, temp_path.as_deref())?;
        if let (Some(base_path), Some(policy)) = (final_path.as_deref(), segmented_policy) {
            configure_segmented_location_callback(&pipeline, base_path, policy)?;
        }
        let video = appsrc(&pipeline, "video_source")?;
        let audio = appsrc(&pipeline, "audio_source")?;
        configure_sources(&video, &audio, plan, video_format, audio_format)?;
        let video_duration = gst::ClockTime::from_nseconds(
            1_000_000_000_u64.saturating_mul(u64::from(video_format.frame_rate().denominator()))
                / u64::from(video_format.frame_rate().numerator()),
        );
        if let Err(error) = pipeline.set_state(gst::State::Playing) {
            let _ = pipeline.set_state(gst::State::Null);
            let _ = recover_stale_recording_artifact(temp_path.as_deref());
            return Err(native_error(error));
        }
        if is_local_recording_transport(plan.profile().transport()) {
            if let Err(error) = ensure_pipeline_startable(&pipeline) {
                let _ = pipeline.set_state(gst::State::Null);
                let _ = recover_stale_recording_artifact(temp_path.as_deref());
                return Err(error);
            }
        }
        if let Some(final_path) = remux_final_path.as_deref() {
            if let Err(error) = write_interrupted_remux_manifest(final_path) {
                let _ = pipeline.set_state(gst::State::Null);
                let _ = recover_stale_recording_artifact(temp_path.as_deref());
                return Err(error);
            }
        }
        Ok(Self {
            pipeline,
            video,
            audio,
            state: NativeOutputState::Ready,
            telemetry: OutputSessionTelemetry::default(),
            committed_bytes: None,
            final_path,
            temp_path,
            remux_final_path,
            segmented_policy,
            transport: plan.profile().transport(),
            video_duration,
            reconnect_policy,
            reconnect_attempts: 0,
            next_reconnect_at: None,
            video_format,
            video_pixel_format: PixelFormat::Rgba8,
        })
    }

    #[must_use]
    pub const fn state(&self) -> NativeOutputState {
        self.state
    }

    #[must_use]
    pub const fn telemetry(&self) -> OutputSessionTelemetry {
        self.telemetry
    }

    /// Returns the total bytes published by the last successful local close.
    #[must_use]
    pub const fn committed_bytes(&self) -> Option<usize> {
        self.committed_bytes
    }

    /// Moves an owned RGBA frame into the bounded video queue.
    ///
    /// # Errors
    ///
    /// Rejects closed sessions, timestamp regression, or downstream failure.
    pub fn push_video(&mut self, frame: VideoFrame) -> Result<(), GStreamerError> {
        if self.video_pixel_format != PixelFormat::Rgba8 {
            self.set_video_caps(PixelFormat::Rgba8)?;
        }
        let timestamp = frame.timestamp();
        self.push_video_bytes(timestamp, frame.into_pixels())
    }

    /// Moves an owned validated packed/planar frame into the video queue.
    ///
    /// Appsrc caps are renegotiated only when the input layout changes, letting
    /// GPU-converted NV12/P010 reach `videoconvert` and hardware encoders
    /// without an intermediate RGBA expansion.
    ///
    /// # Errors
    ///
    /// Rejects mismatched formats, closed sessions, timestamp regression,
    /// invalid caps, or downstream failure.
    pub fn push_raw_video(&mut self, frame: RawVideoFrame) -> Result<(), GStreamerError> {
        if frame.format() != self.video_format {
            return Err(GStreamerError::Native(
                "raw video format does not match the output canvas".to_owned(),
            ));
        }
        if self.video_pixel_format != frame.pixel_format() {
            self.set_video_caps(frame.pixel_format())?;
        }
        let timestamp = frame.timestamp();
        self.push_video_bytes(timestamp, frame.into_bytes())
    }

    fn set_video_caps(&mut self, pixel_format: PixelFormat) -> Result<(), GStreamerError> {
        self.video
            .set_caps(Some(&video_caps(self.video_format, pixel_format)?));
        self.video_pixel_format = pixel_format;
        Ok(())
    }

    fn push_video_bytes(
        &mut self,
        timestamp: Timestamp,
        bytes: Vec<u8>,
    ) -> Result<(), GStreamerError> {
        self.poll_health()?;
        self.ensure_ready()?;
        if self
            .telemetry
            .last_video_timestamp
            .is_some_and(|old| timestamp < old)
        {
            return Err(GStreamerError::Native(
                "video timestamp regressed".to_owned(),
            ));
        }
        let started = Instant::now();
        let mut buffer = gst::Buffer::from_mut_slice(bytes);
        let writable = buffer
            .get_mut()
            .ok_or_else(|| GStreamerError::Native("new video buffer is shared".to_owned()))?;
        writable.set_pts(gst::ClockTime::from_nseconds(timestamp.as_nanos()));
        writable.set_duration(self.video_duration);
        if self.video.push_buffer(buffer).is_err() {
            self.telemetry.dropped = self.telemetry.dropped.saturating_add(1);
            return Err(GStreamerError::Native(
                "video appsrc rejected a frame".to_owned(),
            ));
        }
        self.telemetry.video_submitted = self.telemetry.video_submitted.saturating_add(1);
        self.telemetry.last_video_timestamp = Some(timestamp);
        self.refresh_queue_levels();
        self.record_latency(started.elapsed().as_nanos());
        self.update_drift();
        Ok(())
    }

    /// Moves one audio buffer into the bounded audio queue.
    ///
    /// # Errors
    ///
    /// Rejects closed sessions, timestamp regression, or downstream failure.
    pub fn push_audio(&mut self, buffer: AudioBuffer) -> Result<(), GStreamerError> {
        self.poll_health()?;
        self.ensure_ready()?;
        let timestamp = buffer.timestamp();
        if self
            .telemetry
            .last_audio_timestamp
            .is_some_and(|old| timestamp < old)
        {
            return Err(GStreamerError::Native(
                "audio timestamp regressed".to_owned(),
            ));
        }
        let started = Instant::now();
        let duration = gst::ClockTime::from_nseconds(
            u64::try_from(buffer.frames())
                .unwrap_or(u64::MAX)
                .saturating_mul(1_000_000_000)
                / u64::from(buffer.format().sample_rate()),
        );
        let samples = buffer.into_samples();
        let mut bytes = Vec::with_capacity(samples.len() * size_of::<f32>());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let mut gst_buffer = gst::Buffer::from_mut_slice(bytes);
        let writable = gst_buffer
            .get_mut()
            .ok_or_else(|| GStreamerError::Native("new audio buffer is shared".to_owned()))?;
        writable.set_pts(gst::ClockTime::from_nseconds(timestamp.as_nanos()));
        writable.set_duration(duration);
        if self.audio.push_buffer(gst_buffer).is_err() {
            self.telemetry.dropped = self.telemetry.dropped.saturating_add(1);
            return Err(GStreamerError::Native(
                "audio appsrc rejected a buffer".to_owned(),
            ));
        }
        self.telemetry.audio_submitted = self.telemetry.audio_submitted.saturating_add(1);
        self.telemetry.last_audio_timestamp = Some(timestamp);
        self.refresh_queue_levels();
        self.record_latency(started.elapsed().as_nanos());
        self.update_drift();
        Ok(())
    }

    /// Sends EOS, waits for mux finalization, and atomically publishes a local
    /// recording. Live sessions simply transition to closed.
    ///
    /// # Errors
    ///
    /// Returns a native or filesystem finalization error.
    pub fn close(&mut self) -> Result<(), GStreamerError> {
        // A pipeline can publish an asynchronous Error or EOS between the
        // last media submission and the user's stop action. Consume that
        // signal before sending the intentional EOS below; otherwise a
        // truncated recording could be mistaken for a clean finalization.
        self.poll_health()?;
        self.ensure_ready()?;
        let _ = self.video.end_of_stream();
        let _ = self.audio.end_of_stream();
        // Live muxers do not publish a local index and remote sinks may never
        // acknowledge EOS after a network loss. Stop them immediately so the
        // UI cannot hang for the recording-only finalization timeout.
        if !is_local_recording_transport(self.transport) {
            if let Err(error) = self.pipeline.set_state(gst::State::Null) {
                self.state = NativeOutputState::Failed;
                return Err(native_error(error));
            }
            self.state = NativeOutputState::Closed;
            return Ok(());
        }
        let Some(bus) = self.pipeline.bus() else {
            self.state = NativeOutputState::Failed;
            return Err(GStreamerError::Native(
                "recording pipeline has no message bus".to_owned(),
            ));
        };
        let message = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(10),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        match message.as_ref().map(|message| message.view()) {
            Some(gst::MessageView::Eos(_)) => {}
            Some(gst::MessageView::Error(error)) => {
                self.state = NativeOutputState::Failed;
                let _ = error;
                return Err(GStreamerError::Native(
                    "recording pipeline reported an asynchronous error".to_owned(),
                ));
            }
            _ => {
                self.state = NativeOutputState::Failed;
                return Err(GStreamerError::Native("pipeline EOS timed out".to_owned()));
            }
        }
        if let Err(error) = self.pipeline.set_state(gst::State::Null) {
            self.state = NativeOutputState::Failed;
            return Err(native_error(error));
        }
        let committed_bytes = if let Some(final_path) = &self.remux_final_path {
            let temp = self.temp_path.as_ref().ok_or_else(|| {
                GStreamerError::Native("remux source temporary path is missing".to_owned())
            })?;
            let bytes = remux_matroska_to_mp4(temp, final_path)?;
            fs::remove_file(temp).map_err(|error| {
                GStreamerError::Native(format!("remove remux source temporary recording: {error}"))
            })?;
            recover_stale_remux_manifest(final_path)?;
            Some(bytes)
        } else if let (Some(base_path), Some(policy)) = (&self.final_path, self.segmented_policy) {
            Some(publish_segmented_recording(base_path, policy)?)
        } else if let (Some(temp), Some(final_path)) = (&self.temp_path, &self.final_path) {
            Some(publish_recording_artifact(temp, final_path)?)
        } else {
            None
        };
        self.committed_bytes = committed_bytes;
        self.state = NativeOutputState::Closed;
        Ok(())
    }

    /// Polls asynchronous bus failures and reconnects live transports without
    /// growing application queues. Recordings fail instead of hiding damage.
    ///
    /// # Errors
    ///
    /// Returns the pipeline error when a recording fails or live recovery fails.
    pub fn poll_health(&mut self) -> Result<(), GStreamerError> {
        self.refresh_queue_levels();
        let Some(failure) = take_pipeline_failure(&self.pipeline) else {
            return Ok(());
        };
        self.state = NativeOutputState::Lost;
        if is_local_recording_transport(self.transport) {
            self.state = NativeOutputState::Failed;
            return Err(failure);
        }
        let now = Instant::now();
        self.schedule_reconnect(now);
        self.reconnect_live_at(now).map(|_| ())
    }

    /// Rebuilds a live transport after an application/network loss signal.
    ///
    /// # Errors
    ///
    /// Rejects recording sessions and reports a failed `GStreamer` state change.
    pub fn reconnect_live(&mut self) -> Result<ReconnectOutcome, GStreamerError> {
        self.reconnect_live_at(Instant::now())
    }

    pub(super) fn reconnect_live_at(
        &mut self,
        now: Instant,
    ) -> Result<ReconnectOutcome, GStreamerError> {
        if is_local_recording_transport(self.transport) {
            return Err(GStreamerError::Native(
                "file outputs cannot reconnect".to_owned(),
            ));
        }
        if self.reconnect_attempts >= self.reconnect_policy.max_attempts() {
            self.state = NativeOutputState::Failed;
            return Err(GStreamerError::Native(
                "live output reconnect limit reached".to_owned(),
            ));
        }
        if let Some(deadline) = self.next_reconnect_at {
            if now < deadline {
                self.state = NativeOutputState::Retrying;
                return Ok(ReconnectOutcome::Deferred {
                    retry_after: deadline.duration_since(now),
                });
            }
        }
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
        self.state = NativeOutputState::Retrying;
        self.pipeline
            .set_state(gst::State::Null)
            .map_err(native_error)?;
        if let Err(error) = self.pipeline.set_state(gst::State::Playing) {
            self.state = NativeOutputState::Failed;
            return Err(native_error(error));
        }
        self.next_reconnect_at = None;
        self.telemetry.reconnects = self.telemetry.reconnects.saturating_add(1);
        self.state = NativeOutputState::Ready;
        Ok(ReconnectOutcome::Reconnected)
    }

    fn ensure_ready(&self) -> Result<(), GStreamerError> {
        matches!(
            self.state,
            NativeOutputState::Ready | NativeOutputState::Retrying
        )
        .then_some(())
        .ok_or_else(|| GStreamerError::Native(format!("output session is {:?}", self.state)))
    }

    fn record_latency(&mut self, nanos: u128) {
        self.telemetry.total_submit_latency_nanos = self
            .telemetry
            .total_submit_latency_nanos
            .saturating_add(nanos);
        self.telemetry.max_submit_latency_nanos =
            self.telemetry.max_submit_latency_nanos.max(nanos);
    }

    fn update_drift(&mut self) {
        if let (Some(video), Some(audio)) = (
            self.telemetry.last_video_timestamp,
            self.telemetry.last_audio_timestamp,
        ) {
            self.telemetry.audio_drift_nanos =
                i128::from(audio.as_nanos()) - i128::from(video.as_nanos());
        }
    }

    fn refresh_queue_levels(&mut self) {
        self.telemetry.video_queue_bytes = self.video.property::<u64>("current-level-bytes");
        self.telemetry.audio_queue_bytes = self.audio.property::<u64>("current-level-bytes");
    }

    pub(super) fn schedule_reconnect(&mut self, now: Instant) {
        self.next_reconnect_at = now.checked_add(
            self.reconnect_policy
                .delay_for_attempt(self.reconnect_attempts),
        );
    }
}

fn is_local_recording_transport(transport: OutputTransport) -> bool {
    matches!(
        transport,
        OutputTransport::Matroska
            | OutputTransport::Mp4
            | OutputTransport::Mov
            | OutputTransport::Flv
            | OutputTransport::Hls
    )
}

fn ensure_pipeline_startable(pipeline: &gst::Pipeline) -> Result<(), GStreamerError> {
    let (state_change, current, pending) = pipeline.state(Some(LOCAL_PIPELINE_START_TIMEOUT));
    if let Some(error) = take_pipeline_failure(pipeline) {
        return Err(error);
    }
    if state_change.is_err() {
        return Err(GStreamerError::Native(
            "local production pipeline failed during startup".to_owned(),
        ));
    }
    // A non-live appsrc cannot preroll without its first media buffer, so a
    // recording may legitimately remain Paused while Playing is pending.
    // Accept that state, but reject a transition that no longer targets
    // Playing; the bus check above catches the useful encoder/muxer detail.
    if current != gst::State::Playing && pending != gst::State::Playing {
        return Err(GStreamerError::Native(format!(
            "local production pipeline did not start toward Playing (current={current:?}, pending={pending:?})"
        )));
    }
    Ok(())
}

fn take_pipeline_failure(pipeline: &gst::Pipeline) -> Option<GStreamerError> {
    let message = pipeline
        .bus()?
        .pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos])?;
    match message.view() {
        gst::MessageView::Error(error) => Some(GStreamerError::Native(format!(
            "production pipeline error: {}",
            error.error()
        ))),
        gst::MessageView::Eos(_) => Some(GStreamerError::Native(
            "production pipeline reached EOS unexpectedly".to_owned(),
        )),
        _ => None,
    }
}

impl StreamingTransport for GStreamerOutputSession {
    type Error = GStreamerError;

    fn poll(&mut self) -> Result<usize, Self::Error> {
        self.poll_health().map(|()| 0)
    }

    fn reconnect(&mut self) -> Result<ReconnectOutcome, Self::Error> {
        self.reconnect_live()
    }

    fn close(&mut self) -> Result<(), Self::Error> {
        GStreamerOutputSession::close(self)
    }
}

impl Drop for GStreamerOutputSession {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        if self.state != NativeOutputState::Closed {
            if let Some(temp) = &self.temp_path {
                let _ = fs::remove_file(temp);
            }
            if let Some(final_path) = &self.remux_final_path {
                let _ = fs::remove_file(final_path.with_extension("mp4.part"));
                let _ = recover_stale_remux_manifest(final_path);
            }
            if let (Some(base_path), Some(policy)) = (&self.final_path, self.segmented_policy) {
                let _ = recover_stale_segment_artifacts(base_path, policy);
            }
        }
    }
}
