use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use obs_rs_audio::{AudioBuffer, AudioFormat};
use obs_rs_media::{PixelFormat, RawVideoFrame, Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::{
    EncoderPreset, OutputProfileKind, OutputTransport, RateControl, ReconnectOutcome,
    ReconnectPolicy, SegmentedRecordingPolicy, StreamingTransport, VideoCodec,
};

use super::{GStreamerError, ProductionDestination, ProductionPipelinePlan};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOutputState {
    Opening,
    Ready,
    Lost,
    Retrying,
    Failed,
    Closed,
}

/// Remuxes the native H.264/AAC Matroska recording into MP4 without decoding
/// or re-encoding either stream.
///
/// The operation is intentionally a control-plane function. It streams through
/// bounded `GStreamer` elements, writes a hidden destination, waits for EOS, and
/// publishes the destination only after a non-empty file exists. The caller
/// must keep it off the UI, capture, audio, and render threads.
///
/// # Errors
///
/// Returns a typed endpoint, `GStreamer`, or filesystem error when the source
/// is invalid, the pipeline fails, or the destination cannot be published.
#[allow(
    clippy::too_many_lines,
    reason = "native remux setup and teardown must keep the bounded pipeline lifecycle together"
)]
pub fn remux_matroska_to_mp4(
    source_path: impl AsRef<Path>,
    final_path: impl Into<PathBuf>,
) -> Result<usize, GStreamerError> {
    gst::init().map_err(native_error)?;
    let source_path = source_path.as_ref();
    let final_path = final_path.into();
    if source_path == final_path {
        return Err(GStreamerError::InvalidEndpoint(
            "remux source and destination must differ".to_owned(),
        ));
    }
    if !source_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mkv"))
    {
        return Err(GStreamerError::InvalidEndpoint(
            "remux source must use the .mkv extension".to_owned(),
        ));
    }
    if !final_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
    {
        return Err(GStreamerError::InvalidEndpoint(
            "remux destination must use the .mp4 extension".to_owned(),
        ));
    }
    let source = source_path
        .to_str()
        .ok_or_else(|| GStreamerError::InvalidEndpoint("remux source is not UTF-8".to_owned()))?;
    let source_bytes = fs::metadata(source_path)
        .map_err(|error| GStreamerError::Native(format!("inspect remux source: {error}")))?
        .len();
    if source_bytes == 0 {
        return Err(GStreamerError::Native(
            "refusing to remux an empty Matroska source".to_owned(),
        ));
    }
    let temporary_path = final_path.with_extension("mp4.part");
    recover_stale_recording_artifact(Some(&temporary_path))?;
    let temporary = temporary_path.to_str().ok_or_else(|| {
        GStreamerError::InvalidEndpoint("remux destination is not UTF-8".to_owned())
    })?;

    let source_element = gst::ElementFactory::make("filesrc")
        .property("location", source)
        .build()
        .map_err(native_error)?;
    let demuxer = gst::ElementFactory::make("matroskademux")
        .build()
        .map_err(native_error)?;
    let video_queue = gst::ElementFactory::make("queue")
        .property("max-size-buffers", 16_u32)
        .property("max-size-bytes", 4_194_304_u32)
        .build()
        .map_err(native_error)?;
    let audio_queue = gst::ElementFactory::make("queue")
        .property("max-size-buffers", 16_u32)
        .property("max-size-bytes", 4_194_304_u32)
        .build()
        .map_err(native_error)?;
    let video_parser = gst::ElementFactory::make("h264parse")
        .build()
        .map_err(native_error)?;
    let audio_parser = gst::ElementFactory::make("aacparse")
        .build()
        .map_err(native_error)?;
    let muxer = gst::ElementFactory::make("mp4mux")
        .property("faststart", true)
        .build()
        .map_err(native_error)?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", temporary)
        .build()
        .map_err(native_error)?;
    let pipeline = gst::Pipeline::new();
    pipeline
        .add_many([
            &source_element,
            &demuxer,
            &video_queue,
            &audio_queue,
            &video_parser,
            &audio_parser,
            &muxer,
            &sink,
        ])
        .map_err(native_error)?;
    gst::Element::link_many([&source_element, &demuxer]).map_err(native_error)?;
    gst::Element::link_many([&video_queue, &video_parser]).map_err(native_error)?;
    gst::Element::link_many([&audio_queue, &audio_parser]).map_err(native_error)?;

    let video_mux_pad = muxer
        .request_pad_simple("video_%u")
        .ok_or_else(|| GStreamerError::Native("MP4 video pad is unavailable".to_owned()))?;
    let audio_mux_pad = muxer
        .request_pad_simple("audio_%u")
        .ok_or_else(|| GStreamerError::Native("MP4 audio pad is unavailable".to_owned()))?;
    video_parser
        .static_pad("src")
        .ok_or_else(|| GStreamerError::Native("H.264 parser source pad is unavailable".to_owned()))?
        .link(&video_mux_pad)
        .map_err(native_error)?;
    audio_parser
        .static_pad("src")
        .ok_or_else(|| GStreamerError::Native("AAC parser source pad is unavailable".to_owned()))?
        .link(&audio_mux_pad)
        .map_err(native_error)?;
    gst::Element::link_many([&muxer, &sink]).map_err(native_error)?;

    let link_error = Arc::new(Mutex::new(None::<String>));
    let link_error_for_callback = Arc::clone(&link_error);
    let video_sink = video_queue.static_pad("sink").ok_or_else(|| {
        GStreamerError::Native("remux video queue sink pad is unavailable".to_owned())
    })?;
    let audio_sink = audio_queue.static_pad("sink").ok_or_else(|| {
        GStreamerError::Native("remux audio queue sink pad is unavailable".to_owned())
    })?;
    demuxer.connect_pad_added(move |_demuxer, source_pad| {
        let caps = source_pad
            .current_caps()
            .unwrap_or_else(|| source_pad.query_caps(None));
        let Some(structure) = caps.structure(0) else {
            return;
        };
        let sink_pad = match structure.name().as_str() {
            "video/x-h264" if !video_sink.is_linked() => &video_sink,
            "audio/mpeg" if !audio_sink.is_linked() => &audio_sink,
            _ => return,
        };
        if let Err(error) = source_pad.link(sink_pad) {
            if let Ok(mut link_error) = link_error_for_callback.lock() {
                *link_error = Some(error.to_string());
            }
        }
    });

    pipeline
        .set_state(gst::State::Playing)
        .map_err(native_error)?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| GStreamerError::Native("remux pipeline has no bus".to_owned()))?;
    let message = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(60 * 60),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    let pipeline_result = match message.as_ref().map(|message| message.view()) {
        Some(gst::MessageView::Eos(_)) => Ok(()),
        Some(gst::MessageView::Error(error)) => Err(GStreamerError::Native(format!(
            "remux pipeline reported an error: {}",
            error.error()
        ))),
        _ => Err(GStreamerError::Native(
            "remux pipeline timed out".to_owned(),
        )),
    };
    let state_result = pipeline.set_state(gst::State::Null).map_err(native_error);
    if let Err(error) = pipeline_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    state_result?;
    if let Ok(link_error) = link_error.lock() {
        if let Some(error) = link_error.as_ref() {
            let _ = fs::remove_file(&temporary_path);
            return Err(GStreamerError::Native(format!(
                "remux stream link failed: {error}"
            )));
        }
    }
    match publish_recording_artifact(&temporary_path, &final_path) {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            let _ = fs::remove_file(&temporary_path);
            Err(error)
        }
    }
}

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
    video: gst_app::AppSrc,
    audio: gst_app::AppSrc,
    state: NativeOutputState,
    telemetry: OutputSessionTelemetry,
    committed_bytes: Option<usize>,
    final_path: Option<PathBuf>,
    temp_path: Option<PathBuf>,
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
        gst::init().map_err(native_error)?;
        destination.validate_for(plan.profile())?;
        let pipeline_description = pipeline_description(plan, destination)?;
        let PipelineDescription {
            description,
            final_path,
            temp_path,
            segmented_policy,
        } = pipeline_description;
        recover_stale_recording_artifact(temp_path.as_deref())?;
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
        pipeline
            .set_state(gst::State::Playing)
            .map_err(native_error)?;
        Ok(Self {
            pipeline,
            video,
            audio,
            state: NativeOutputState::Ready,
            telemetry: OutputSessionTelemetry::default(),
            committed_bytes: None,
            final_path,
            temp_path,
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
        self.ensure_ready()?;
        let _ = self.video.end_of_stream();
        let _ = self.audio.end_of_stream();
        // Live muxers do not publish a local index and remote sinks may never
        // acknowledge EOS after a network loss. Stop them immediately so the
        // UI cannot hang for the recording-only finalization timeout.
        if !matches!(
            self.transport,
            OutputTransport::Matroska
                | OutputTransport::Mp4
                | OutputTransport::Mov
                | OutputTransport::Flv
                | OutputTransport::Hls
        ) {
            self.pipeline
                .set_state(gst::State::Null)
                .map_err(native_error)?;
            self.state = NativeOutputState::Closed;
            return Ok(());
        }
        if let Some(bus) = self.pipeline.bus() {
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
        }
        self.pipeline
            .set_state(gst::State::Null)
            .map_err(native_error)?;
        let committed_bytes =
            if let (Some(base_path), Some(policy)) = (&self.final_path, self.segmented_policy) {
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
        let failed = self.pipeline.bus().is_some_and(|bus| {
            bus.pop_filtered(&[gst::MessageType::Error])
                .is_some_and(|message| matches!(message.view(), gst::MessageView::Error(_)))
        });
        if !failed {
            return Ok(());
        }
        self.state = NativeOutputState::Lost;
        if matches!(
            self.transport,
            OutputTransport::Matroska
                | OutputTransport::Mp4
                | OutputTransport::Mov
                | OutputTransport::Flv
                | OutputTransport::Hls
        ) {
            self.state = NativeOutputState::Failed;
            return Err(GStreamerError::Native(
                "recording pipeline reported an asynchronous error".to_owned(),
            ));
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

    fn reconnect_live_at(&mut self, now: Instant) -> Result<ReconnectOutcome, GStreamerError> {
        if matches!(
            self.transport,
            OutputTransport::Matroska
                | OutputTransport::Mp4
                | OutputTransport::Mov
                | OutputTransport::Flv
                | OutputTransport::Hls
        ) {
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

    fn schedule_reconnect(&mut self, now: Instant) {
        self.next_reconnect_at = now.checked_add(
            self.reconnect_policy
                .delay_for_attempt(self.reconnect_attempts),
        );
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
            if let (Some(base_path), Some(policy)) = (&self.final_path, self.segmented_policy) {
                let _ = recover_stale_segment_artifacts(base_path, policy);
            }
        }
    }
}

fn appsrc(pipeline: &gst::Pipeline, name: &str) -> Result<gst_app::AppSrc, GStreamerError> {
    pipeline
        .by_name(name)
        .ok_or_else(|| GStreamerError::Native(format!("{name} is missing")))?
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| GStreamerError::Native(format!("{name} is not appsrc")))
}

fn configure_sources(
    video: &gst_app::AppSrc,
    audio: &gst_app::AppSrc,
    plan: &ProductionPipelinePlan,
    video_format: VideoFormat,
    audio_format: AudioFormat,
) -> Result<(), GStreamerError> {
    let video_caps = video_caps(video_format, PixelFormat::Rgba8)?;
    let audio_caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field("channels", i32::from(audio_format.channels()))
        .field(
            "rate",
            i32::try_from(audio_format.sample_rate()).map_err(native_error)?,
        )
        .build();
    for (source, caps) in [(video, &video_caps), (audio, &audio_caps)] {
        source.set_caps(Some(caps));
        source.set_format(gst::Format::Time);
        source.set_is_live(!matches!(
            plan.profile().transport(),
            OutputTransport::Matroska
                | OutputTransport::Mp4
                | OutputTransport::Mov
                | OutputTransport::Flv
        ));
        source.set_max_bytes(plan.bounded_queue_bytes() as u64);
        source.set_block(false);
        source.set_leaky_type(gst_app::AppLeakyType::Downstream);
    }
    Ok(())
}

fn video_caps(
    video_format: VideoFormat,
    pixel_format: PixelFormat,
) -> Result<gst::Caps, GStreamerError> {
    let format = match pixel_format {
        PixelFormat::Rgba8 => "RGBA",
        PixelFormat::Bgra8 => "BGRA",
        PixelFormat::Rgb8 => "RGB",
        PixelFormat::Gray8 => "GRAY8",
        PixelFormat::I420 => "I420",
        PixelFormat::Nv12 => "NV12",
        PixelFormat::P010 => "P010_10LE",
    };
    Ok(gst::Caps::builder("video/x-raw")
        .field("format", format)
        .field(
            "width",
            i32::try_from(video_format.width()).map_err(native_error)?,
        )
        .field(
            "height",
            i32::try_from(video_format.height()).map_err(native_error)?,
        )
        .field(
            "framerate",
            gst::Fraction::new(
                i32::try_from(video_format.frame_rate().numerator()).map_err(native_error)?,
                i32::try_from(video_format.frame_rate().denominator()).map_err(native_error)?,
            ),
        )
        .build())
}

struct PipelineDescription {
    description: String,
    final_path: Option<PathBuf>,
    temp_path: Option<PathBuf>,
    segmented_policy: Option<SegmentedRecordingPolicy>,
}

impl PipelineDescription {
    fn live(description: String) -> Self {
        Self {
            description,
            final_path: None,
            temp_path: None,
            segmented_policy: None,
        }
    }

    fn recording(description: String, final_path: PathBuf, temp_path: PathBuf) -> Self {
        Self {
            description,
            final_path: Some(final_path),
            temp_path: Some(temp_path),
            segmented_policy: None,
        }
    }

    fn segmented(
        description: String,
        base_path: PathBuf,
        policy: SegmentedRecordingPolicy,
    ) -> Self {
        Self {
            description,
            final_path: Some(base_path),
            temp_path: None,
            segmented_policy: Some(policy),
        }
    }
}

fn pipeline_description(
    plan: &ProductionPipelinePlan,
    destination: &ProductionDestination,
) -> Result<PipelineDescription, GStreamerError> {
    let queue = plan.bounded_queue_bytes();
    let video = plan.video_encoder();
    let audio = plan.audio_encoder();
    let profile_caps = match (
        plan.video_config().codec,
        plan.video_config().profile.as_deref(),
    ) {
        (VideoCodec::H264, Some("baseline")) => "video/x-h264,profile=baseline ! ",
        (VideoCodec::H264, Some("main")) => "video/x-h264,profile=main ! ",
        (VideoCodec::H264, Some("high")) => "video/x-h264,profile=high ! ",
        _ => "",
    };
    let v = format!("appsrc name=video_source ! queue max-size-bytes={queue} leaky=downstream ! videoconvert ! {video} name=video_encoder ! {profile_caps}");
    let audio_config = plan.audio_config();
    let audio_rate = audio_config.sample_rate;
    let audio_channels = audio_config.channels;
    let a = format!("appsrc name=audio_source ! queue max-size-bytes={queue} leaky=downstream ! audioconvert ! audioresample ! audio/x-raw,rate={audio_rate},channels={audio_channels} ! {audio} name=audio_encoder ! ");
    match destination {
        ProductionDestination::Recording(final_path) => {
            recording_pipeline_description(plan, &v, &a, final_path)
        }
        ProductionDestination::SegmentedRecording { base_path, policy } => {
            segmented_recording_pipeline_description(plan, &v, &a, base_path, *policy)
        }
        _ => live_pipeline_description(plan, &v, &a, destination),
    }
}

fn recording_pipeline_description(
    plan: &ProductionPipelinePlan,
    video: &str,
    audio: &str,
    final_path: &Path,
) -> Result<PipelineDescription, GStreamerError> {
    let (description, extension) = match plan.profile().transport() {
        OutputTransport::Matroska => {
            let parser = matroska_parser(plan.video_config().codec)?;
            (
                format!(
                    "{video}{parser} ! mux. {audio}aacparse ! mux. matroskamux name=mux ! filesink name=output_sink"
                ),
                "mkv",
            )
        }
        OutputTransport::Mp4 => {
            require_h264(plan.video_config().codec, "MP4")?;
            let mux = if plan.profile().kind() == OutputProfileKind::FragmentedMp4H264Aac {
                "mp4mux name=mux fragment-duration=1000 streamable=true"
            } else {
                "mp4mux name=mux faststart=true"
            };
            (
                format!("{video}h264parse ! mux. {audio}aacparse ! mux. {mux} ! filesink name=output_sink"),
                "mp4",
            )
        }
        OutputTransport::Mov => {
            require_h264(plan.video_config().codec, "MOV")?;
            (
                format!(
                    "{video}h264parse ! mux. {audio}aacparse ! mux. qtmux name=mux faststart=true ! filesink name=output_sink"
                ),
                "mov",
            )
        }
        OutputTransport::Flv => {
            require_h264(plan.video_config().codec, "FLV")?;
            (
                format!(
                    "{video}h264parse ! mux. {audio}aacparse ! mux. flvmux name=mux streamable=false ! filesink name=output_sink"
                ),
                "flv",
            )
        }
        _ => {
            return Err(GStreamerError::InvalidEndpoint(
                "destination does not match pipeline".to_owned(),
            ))
        }
    };
    Ok(PipelineDescription::recording(
        description,
        final_path.to_owned(),
        final_path.with_extension(format!("{extension}.part")),
    ))
}

fn segmented_recording_pipeline_description(
    plan: &ProductionPipelinePlan,
    video: &str,
    audio: &str,
    base_path: &Path,
    policy: SegmentedRecordingPolicy,
) -> Result<PipelineDescription, GStreamerError> {
    let (parser, muxer) = match plan.profile().transport() {
        OutputTransport::Matroska => (matroska_parser(plan.video_config().codec)?, "matroskamux"),
        OutputTransport::Mp4 => {
            require_h264(plan.video_config().codec, "MP4")?;
            ("h264parse", "mp4mux")
        }
        OutputTransport::Mov => {
            require_h264(plan.video_config().codec, "MOV")?;
            ("h264parse", "qtmux")
        }
        OutputTransport::Flv => {
            require_h264(plan.video_config().codec, "FLV")?;
            ("h264parse", "flvmux")
        }
        _ => {
            return Err(GStreamerError::InvalidEndpoint(
                "destination does not match pipeline".to_owned(),
            ))
        }
    };
    Ok(PipelineDescription::segmented(
        segmented_pipeline_description(video, audio, parser, muxer, policy),
        base_path.to_owned(),
        policy,
    ))
}

fn live_pipeline_description(
    plan: &ProductionPipelinePlan,
    video: &str,
    audio: &str,
    destination: &ProductionDestination,
) -> Result<PipelineDescription, GStreamerError> {
    match (plan.profile().transport(), destination) {
        (OutputTransport::Rtmp, ProductionDestination::Rtmp { .. })
        | (OutputTransport::Rtmps, ProductionDestination::Rtmps { .. }) => {
            let sink = plan.rtmp_sink().ok_or_else(|| {
                GStreamerError::Native("negotiated RTMP sink is missing".to_owned())
            })?;
            Ok(PipelineDescription::live(format!("{video}h264parse config-interval=-1 ! mux. {audio}aacparse ! mux. flvmux name=mux streamable=true ! {sink} name=output_sink")))
        }
        (OutputTransport::SrtMpegTs, ProductionDestination::Srt { .. }) =>
            Ok(PipelineDescription::live(format!("{video}h264parse config-interval=-1 ! mux. {audio}aacparse ! mux. mpegtsmux name=mux ! srtsink name=output_sink"))),
        (OutputTransport::WebRtc, ProductionDestination::WebRtc { .. }) =>
            Ok(PipelineDescription::live(format!("whipclientsink name=output_sink appsrc name=video_source ! queue max-size-bytes={} leaky=downstream ! videoconvert ! output_sink. appsrc name=audio_source ! queue max-size-bytes={} leaky=downstream ! audioconvert ! audioresample ! output_sink.", plan.bounded_queue_bytes(), plan.bounded_queue_bytes()))),
        (OutputTransport::Hls, ProductionDestination::Hls { .. }) =>
            Ok(PipelineDescription::live(format!("hlssink2 name=output_sink {video}h264parse ! output_sink.video {audio}aacparse ! output_sink.audio"))),
        (OutputTransport::RistMpegTs, ProductionDestination::Rist { .. }) =>
            Ok(PipelineDescription::live(format!("{video}h264parse config-interval=-1 ! mux. {audio}aacparse ! mux. mpegtsmux name=mux ! rtpmp2tpay ! ristsink name=output_sink"))),
        _ => Err(GStreamerError::InvalidEndpoint("destination does not match pipeline".to_owned())),
    }
}

fn matroska_parser(codec: VideoCodec) -> Result<&'static str, GStreamerError> {
    match codec {
        VideoCodec::H264 => Ok("h264parse"),
        VideoCodec::Hevc => Ok("h265parse"),
        VideoCodec::Av1 => Ok("av1parse"),
        codec => Err(GStreamerError::Native(format!(
            "unsupported Matroska video codec {codec:?}"
        ))),
    }
}

fn require_h264(codec: VideoCodec, container: &str) -> Result<(), GStreamerError> {
    if codec == VideoCodec::H264 {
        Ok(())
    } else {
        Err(GStreamerError::Native(format!(
            "{container} production recording currently requires H.264 video"
        )))
    }
}

fn segmented_pipeline_description(
    video: &str,
    audio: &str,
    parser: &str,
    muxer: &str,
    policy: SegmentedRecordingPolicy,
) -> String {
    let duration = u64::try_from(policy.max_segment_duration().as_nanos()).unwrap_or(u64::MAX);
    let bytes = u64::try_from(policy.max_segment_bytes()).unwrap_or(u64::MAX);
    format!(
        "splitmuxsink name=output_sink muxer-factory={muxer} max-size-time={duration} max-size-bytes={bytes} max-files={} start-index=1 {video}{parser} ! output_sink.video {audio}aacparse ! output_sink.audio_0",
        policy.max_segments()
    )
}

fn configure_encoders(
    pipeline: &gst::Pipeline,
    plan: &ProductionPipelinePlan,
    format: VideoFormat,
) -> Result<(), GStreamerError> {
    let video = pipeline
        .by_name("video_encoder")
        .ok_or_else(|| GStreamerError::Native("video encoder is missing".to_owned()))?;
    let audio = pipeline
        .by_name("audio_encoder")
        .ok_or_else(|| GStreamerError::Native("audio encoder is missing".to_owned()))?;
    let config = plan.video_config();
    let fps = u64::from(format.frame_rate().numerator())
        .div_ceil(u64::from(format.frame_rate().denominator()));
    let gop = u64::from(config.keyframe_interval_secs).saturating_mul(fps);
    configure_video_encoder(video, plan.video_encoder(), config, gop)?;
    configure_audio_encoder(&audio, plan)?;
    Ok(())
}

#[allow(
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    reason = "the exhaustive plugin dispatch keeps each encoder's property vocabulary isolated"
)]
fn configure_video_encoder(
    video: gst::Element,
    encoder: &str,
    config: &obs_rs_output::VideoEncoderConfig,
    gop: u64,
) -> Result<(), GStreamerError> {
    match encoder {
        "openh264enc" => {
            video.set_property("bitrate", config.bitrate_kbps.saturating_mul(1_000));
            video.set_property(
                "max-bitrate",
                config.max_bitrate_kbps.unwrap_or(0).saturating_mul(1_000),
            );
            video.set_property("gop-size", u32::try_from(gop).unwrap_or(u32::MAX));
            video.set_property_from_str(
                "rate-control",
                match config.rate_control {
                    RateControl::Cbr | RateControl::Vbr => "bitrate",
                    RateControl::Cqp => "quality",
                },
            );
            video.set_property_from_str(
                "complexity",
                match config.preset {
                    EncoderPreset::Speed => "low",
                    EncoderPreset::Balanced => "medium",
                    EncoderPreset::Quality => "high",
                },
            );
        }
        "nvh264enc" => {
            set_first_string_property(&video, &["bitrate"], &config.bitrate_kbps.to_string());
            set_first_string_property(&video, &["gop-size"], &gop.to_string());
            set_first_string_property(
                &video,
                &["bframes", "b-frames", "max-bframes"],
                &config.b_frames.to_string(),
            );
            set_first_string_property(
                &video,
                &["rc-mode"],
                match config.rate_control {
                    RateControl::Cbr => "cbr",
                    RateControl::Vbr => "vbr",
                    RateControl::Cqp => "constqp",
                },
            );
            set_first_string_property(
                &video,
                &["preset"],
                match config.preset {
                    EncoderPreset::Speed => "hp",
                    EncoderPreset::Balanced => "default",
                    EncoderPreset::Quality => "hq",
                },
            );
        }
        "vah264enc" | "vaapih264enc" => {
            set_first_string_property(&video, &["bitrate"], &config.bitrate_kbps.to_string());
            set_first_string_property(
                &video,
                &["key-int-max", "keyframe-period"],
                &gop.to_string(),
            );
            set_first_string_property(&video, &["rate-control"], config.rate_control.id());
            set_first_string_property(
                &video,
                &["target-usage"],
                match config.preset {
                    EncoderPreset::Speed => "7",
                    EncoderPreset::Balanced => "4",
                    EncoderPreset::Quality => "1",
                },
            );
            set_first_string_property(
                &video,
                &["b-frames", "max-bframes"],
                &config.b_frames.to_string(),
            );
        }
        "vah265enc" | "vaapih265enc" | "nvh265enc" => {
            set_first_string_property(&video, &["bitrate"], &config.bitrate_kbps.to_string());
            set_first_string_property(
                &video,
                &["key-int-max", "gop-size", "keyframe-period"],
                &gop.to_string(),
            );
            set_first_string_property(
                &video,
                &["bframes", "b-frames", "max-bframes"],
                &config.b_frames.to_string(),
            );
        }
        "x265enc" => {
            video.set_property("bitrate", config.bitrate_kbps);
            video.set_property("key-int-max", i32::try_from(gop).unwrap_or(i32::MAX));
            video.set_property_from_str(
                "speed-preset",
                match config.preset {
                    EncoderPreset::Speed => "ultrafast",
                    EncoderPreset::Balanced => "medium",
                    EncoderPreset::Quality => "slow",
                },
            );
        }
        "svthevcenc" => {
            video.set_property("bitrate", config.bitrate_kbps);
            video.set_property("key-int-max", i32::try_from(gop.min(255)).unwrap_or(255));
            video.set_property_from_str("rc", config.rate_control.id());
            video.set_property(
                "speed",
                match config.preset {
                    EncoderPreset::Speed => 9_u32,
                    EncoderPreset::Balanced => 6,
                    EncoderPreset::Quality => 2,
                },
            );
        }
        "svtav1enc" => {
            video.set_property("target-bitrate", config.bitrate_kbps);
            video.set_property("max-bitrate", config.max_bitrate_kbps.unwrap_or(0));
            video.set_property(
                "intra-period-length",
                i32::try_from(gop).unwrap_or(i32::MAX),
            );
            video.set_property(
                "preset",
                match config.preset {
                    EncoderPreset::Speed => 12_u32,
                    EncoderPreset::Balanced => 8,
                    EncoderPreset::Quality => 4,
                },
            );
        }
        "av1enc" | "aomenc" => {
            video.set_property("target-bitrate", config.bitrate_kbps);
            set_first_string_property(&video, &["keyframe-max-dist"], &gop.to_string());
            set_first_string_property(
                &video,
                &["cpu-used"],
                match config.preset {
                    EncoderPreset::Speed => "8",
                    EncoderPreset::Balanced => "4",
                    EncoderPreset::Quality => "1",
                },
            );
        }
        "rav1enc" => {
            set_first_string_property(
                &video,
                &["bitrate", "target-bitrate"],
                &config.bitrate_kbps.to_string(),
            );
            set_first_string_property(
                &video,
                &["key-frame-interval", "keyframe-max-dist"],
                &gop.to_string(),
            );
        }
        "vp8enc" => {
            video.set_property("target-bitrate", config.bitrate_kbps.saturating_mul(1_000));
            video.set_property("keyframe-max-dist", i32::try_from(gop).unwrap_or(i32::MAX));
        }
        unsupported => {
            return Err(GStreamerError::Native(format!(
                "unsupported configured video encoder {unsupported}"
            )));
        }
    }
    Ok(())
}

fn configure_audio_encoder(
    audio: &gst::Element,
    plan: &ProductionPipelinePlan,
) -> Result<(), GStreamerError> {
    let audio_bitrate = plan.audio_config().bitrate_kbps.saturating_mul(1_000);
    match plan.audio_encoder() {
        "avenc_aac" => {
            audio.set_property("bitrate", i32::try_from(audio_bitrate).unwrap_or(i32::MAX));
        }
        "opusenc" => {
            audio.set_property("bitrate", i32::try_from(audio_bitrate).unwrap_or(i32::MAX));
            if let Some(complexity) = plan.audio_config().complexity {
                audio.set_property("complexity", i32::from(complexity));
            }
        }
        encoder => {
            return Err(GStreamerError::Native(format!(
                "unsupported configured audio encoder {encoder}"
            )));
        }
    }
    Ok(())
}

fn set_first_string_property(element: &gst::Element, names: &[&str], value: &str) {
    if let Some(name) = names
        .iter()
        .find(|name| element.find_property(name).is_some())
    {
        element.set_property_from_str(name, value);
    }
}

fn configure_sink(
    pipeline: &gst::Pipeline,
    destination: &ProductionDestination,
    temp_path: Option<&std::path::Path>,
) -> Result<(), GStreamerError> {
    let sink = pipeline
        .by_name("output_sink")
        .ok_or_else(|| GStreamerError::Native("output sink is missing".to_owned()))?;
    match destination {
        ProductionDestination::Recording(_) => {
            let location = temp_path.and_then(std::path::Path::to_str).ok_or_else(|| {
                GStreamerError::InvalidEndpoint("recording path is not UTF-8".to_owned())
            })?;
            sink.set_property("location", location);
        }
        ProductionDestination::SegmentedRecording { base_path, .. } => {
            let location = segmented_recording_pattern(base_path)?;
            sink.set_property("location", location.to_string_lossy().as_ref());
        }
        ProductionDestination::Rtmp { endpoint } | ProductionDestination::Rtmps { endpoint } => {
            sink.set_property("location", endpoint);
        }
        ProductionDestination::Srt {
            endpoint,
            passphrase,
        } => {
            let uri = passphrase.as_ref().map_or_else(
                || endpoint.clone(),
                |secret| {
                    let separator = if endpoint.contains('?') { '&' } else { '?' };
                    format!("{endpoint}{separator}passphrase={secret}")
                },
            );
            sink.set_property("uri", uri);
        }
        ProductionDestination::WebRtc {
            signaling_endpoint,
            bearer_token,
        } => {
            set_first_string_property(&sink, &["whip-endpoint", "endpoint"], signaling_endpoint);
            if let Some(token) = bearer_token {
                set_first_string_property(&sink, &["auth-token", "bearer-token"], token);
            }
        }
        ProductionDestination::Hls {
            directory,
            segment_duration_secs,
            playlist_size,
            ..
        } => {
            fs::create_dir_all(directory).map_err(native_error)?;
            let segments = directory.join("segment%05d.ts");
            let playlist = directory.join("playlist.m3u8");
            sink.set_property("location", segments.to_string_lossy().as_ref());
            sink.set_property("playlist-location", playlist.to_string_lossy().as_ref());
            sink.set_property("target-duration", segment_duration_secs);
            sink.set_property("playlist-length", playlist_size);
            sink.set_property("max-files", playlist_size.saturating_add(1));
        }
        ProductionDestination::Rist {
            host,
            port,
            sender_buffer_ms,
            ..
        } => {
            sink.set_property("address", host);
            sink.set_property("port", u32::from(*port));
            sink.set_property("sender-buffer", sender_buffer_ms);
            sink.set_property("stats-update-interval", 1_000_u32);
        }
    }
    Ok(())
}

fn configure_segmented_location_callback(
    pipeline: &gst::Pipeline,
    base_path: &Path,
    policy: SegmentedRecordingPolicy,
) -> Result<(), GStreamerError> {
    let sink = pipeline
        .by_name("output_sink")
        .ok_or_else(|| GStreamerError::Native("split sink is missing".to_owned()))?;
    let base_path = base_path.to_owned();
    sink.connect("format-location", false, move |values| {
        let index = values
            .get(1)
            .and_then(|value| value.get::<u32>().ok())
            .map_or(1, |value| usize::try_from(value).unwrap_or(1));
        let slot = ((index.saturating_sub(1)) % policy.max_segments()).saturating_add(1);
        let (_, temp_path) = segmented_recording_paths(&base_path, slot)
            .expect("validated segmented recording path must remain representable");
        Some(temp_path.to_string_lossy().to_string().to_value())
    });
    Ok(())
}

fn native_error(error: impl std::fmt::Display) -> GStreamerError {
    GStreamerError::Native(error.to_string())
}

fn recover_stale_recording_artifact(temp_path: Option<&Path>) -> Result<(), GStreamerError> {
    let Some(temp_path) = temp_path else {
        return Ok(());
    };
    remove_stale_recording_path(temp_path)
}

fn recover_stale_segment_artifacts(
    base_path: &Path,
    policy: SegmentedRecordingPolicy,
) -> Result<(), GStreamerError> {
    for index in 1..=policy.max_segments() {
        let (_, temp_path) = segmented_recording_paths(base_path, index)?;
        remove_stale_recording_path(&temp_path)?;
    }
    Ok(())
}

fn remove_stale_recording_path(path: &Path) -> Result<(), GStreamerError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(GStreamerError::Native(format!(
            "remove stale production recording artifact: {error}"
        ))),
    }
}

fn segmented_recording_pattern(base_path: &Path) -> Result<PathBuf, GStreamerError> {
    let file_name = base_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| GStreamerError::InvalidEndpoint("recording path is not UTF-8".to_owned()))?;
    let stem = base_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            GStreamerError::InvalidEndpoint("recording path has no UTF-8 stem".to_owned())
        })?;
    let extension = base_path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            GStreamerError::InvalidEndpoint("recording path has no UTF-8 extension".to_owned())
        })?;
    if file_name.contains('%') {
        return Err(GStreamerError::InvalidEndpoint(
            "segmented recording path cannot contain '%'".to_owned(),
        ));
    }
    Ok(base_path.with_file_name(format!("{stem}-%05d.{extension}.part")))
}

fn segmented_recording_paths(
    base_path: &Path,
    index: usize,
) -> Result<(PathBuf, PathBuf), GStreamerError> {
    let file_name = base_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| GStreamerError::InvalidEndpoint("recording path is not UTF-8".to_owned()))?;
    let stem = base_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            GStreamerError::InvalidEndpoint("recording path has no UTF-8 stem".to_owned())
        })?;
    let extension = base_path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            GStreamerError::InvalidEndpoint("recording path has no UTF-8 extension".to_owned())
        })?;
    if file_name.contains('%') || index == 0 || index > 99_999 {
        return Err(GStreamerError::InvalidEndpoint(
            "segmented recording path or index is invalid".to_owned(),
        ));
    }
    let stem = format!("{stem}-{index:05}.{extension}");
    let final_path = base_path.with_file_name(&stem);
    let temp_path = base_path.with_file_name(format!("{stem}.part"));
    Ok((final_path, temp_path))
}

fn publish_segmented_recording(
    base_path: &Path,
    policy: SegmentedRecordingPolicy,
) -> Result<usize, GStreamerError> {
    let mut published = 0_usize;
    let mut total_bytes = 0_usize;
    for index in 1..=policy.max_segments() {
        let (final_path, temp_path) = segmented_recording_paths(base_path, index)?;
        match fs::metadata(&temp_path) {
            Ok(_) => {
                let bytes = publish_recording_artifact(&temp_path, &final_path)?;
                published = published.saturating_add(1);
                total_bytes = total_bytes.checked_add(bytes).ok_or_else(|| {
                    GStreamerError::Native(
                        "published production segment size exceeds platform limits".to_owned(),
                    )
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(GStreamerError::Native(format!(
                    "inspect production recording segment: {error}"
                )))
            }
        }
    }
    if published == 0 {
        return Err(GStreamerError::Native(
            "production segment muxer produced no recording artifacts".to_owned(),
        ));
    }
    Ok(total_bytes)
}

fn publish_recording_artifact(temp: &Path, final_path: &Path) -> Result<usize, GStreamerError> {
    let bytes = fs::metadata(temp)
        .map_err(|error| GStreamerError::Native(format!("inspect production recording: {error}")))?
        .len();
    let bytes = usize::try_from(bytes).map_err(|_| {
        GStreamerError::Native("production recording size exceeds platform limits".to_owned())
    })?;
    if bytes == 0 {
        return Err(GStreamerError::Native(
            "refusing to publish an empty production recording".to_owned(),
        ));
    }
    fs::hard_link(temp, final_path).map_err(|error| {
        GStreamerError::Native(format!(
            "publish production recording without replacing an existing file: {error}"
        ))
    })?;
    fs::remove_file(temp).map_err(|error| {
        GStreamerError::Native(format!(
            "remove published production recording temporary path: {error}"
        ))
    })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obs_rs_output::{
        AudioEncoderConfig, EncoderImplementation, OutputProfile, OutputProfileKind,
        VideoEncoderConfig,
    };

    fn plan(profile: OutputProfile) -> ProductionPipelinePlan {
        let video_config = VideoEncoderConfig {
            implementation: EncoderImplementation::new("openh264enc"),
            ..VideoEncoderConfig::default()
        };
        let audio_config = AudioEncoderConfig {
            implementation: EncoderImplementation::new("avenc_aac"),
            ..AudioEncoderConfig::default()
        };
        ProductionPipelinePlan {
            profile,
            video_encoder: "openh264enc".to_owned(),
            audio_encoder: "avenc_aac".to_owned(),
            bounded_queue_bytes: 1_048_576,
            atomic_recording: false,
            video_config,
            audio_config,
            rtmp_sink: matches!(
                profile.kind(),
                OutputProfileKind::RtmpH264Aac | OutputProfileKind::RtmpsH264Aac
            )
            .then(|| "rtmp2sink".to_owned()),
        }
    }

    #[test]
    fn live_protocols_build_exact_mux_and_sink_graphs() {
        gst::init().expect("GStreamer runtime");
        let cases = [
            (
                plan(OutputProfile::rtmp_h264_aac()),
                ProductionDestination::Rtmp {
                    endpoint: "rtmp://127.0.0.1/live/key".to_owned(),
                },
                "flvmux",
                "rtmp2sink",
            ),
            (
                plan(OutputProfile::rtmps_h264_aac()),
                ProductionDestination::Rtmps {
                    endpoint: "rtmps://127.0.0.1/live/key".to_owned(),
                },
                "flvmux",
                "rtmp2sink",
            ),
            (
                plan(OutputProfile::srt_mpeg_ts_h264_aac()),
                ProductionDestination::Srt {
                    endpoint: "srt://127.0.0.1:9000".to_owned(),
                    passphrase: None,
                },
                "mpegtsmux",
                "srtsink",
            ),
        ];
        for (plan, destination, mux, sink) in cases {
            let pipeline_description =
                pipeline_description(&plan, &destination).expect("pipeline description");
            assert!(pipeline_description.description.contains(mux));
            assert!(pipeline_description.description.contains(sink));
            assert!(
                pipeline_description.final_path.is_none()
                    && pipeline_description.temp_path.is_none()
                    && pipeline_description.segmented_policy.is_none()
            );
            let element = gst::parse::launch_full(
                &pipeline_description.description,
                None,
                gst::ParseFlags::FATAL_ERRORS,
            )
            .expect("pipeline parses");
            let pipeline = element.downcast::<gst::Pipeline>().expect("pipeline");
            configure_sink(&pipeline, &destination, None).expect("sink endpoint");
            let output_sink = pipeline.by_name("output_sink").expect("named sink");
            let configured = match &destination {
                ProductionDestination::Rtmp { .. } | ProductionDestination::Rtmps { .. } => {
                    output_sink.property::<String>("location")
                }
                ProductionDestination::Srt { .. } => output_sink.property::<String>("uri"),
                _ => unreachable!("only live protocols are in the cases"),
            };
            let (ProductionDestination::Rtmp { endpoint }
            | ProductionDestination::Rtmps { endpoint }
            | ProductionDestination::Srt { endpoint, .. }) = destination
            else {
                unreachable!("only live protocols are in the cases")
            };
            assert_eq!(configured, endpoint);
        }
        assert_eq!(
            OutputProfile::rtmps_h264_aac().kind(),
            OutputProfileKind::RtmpsH264Aac
        );
    }

    #[test]
    fn openh264_and_aac_tuning_reaches_native_encoder_properties() {
        gst::init().expect("GStreamer runtime");
        if gst::ElementFactory::find("openh264enc").is_none()
            || gst::ElementFactory::find("avenc_aac").is_none()
        {
            return;
        }
        let mut plan = plan(OutputProfile::matroska_h264_aac());
        plan.video_config.bitrate_kbps = 7_500;
        plan.video_config.max_bitrate_kbps = Some(8_000);
        plan.video_config.keyframe_interval_secs = 3;
        plan.video_config.rate_control = RateControl::Cqp;
        plan.video_config.preset = EncoderPreset::Quality;
        plan.audio_config.bitrate_kbps = 192;
        let destination = ProductionDestination::Recording(PathBuf::from("configured.mkv"));
        let pipeline_description = pipeline_description(&plan, &destination).expect("description");
        let pipeline = gst::parse::launch(&pipeline_description.description)
            .expect("pipeline")
            .downcast::<gst::Pipeline>()
            .expect("pipeline type");
        let format = VideoFormat::new(
            320,
            180,
            obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
        )
        .expect("video format");
        configure_encoders(&pipeline, &plan, format).expect("encoder config");
        let video = pipeline.by_name("video_encoder").expect("video encoder");
        let audio = pipeline.by_name("audio_encoder").expect("audio encoder");
        assert_eq!(video.property::<u32>("bitrate"), 7_500_000);
        assert_eq!(video.property::<u32>("max-bitrate"), 8_000_000);
        assert_eq!(video.property::<u32>("gop-size"), 90);
        assert_eq!(audio.property::<i32>("bitrate"), 192_000);
    }

    #[test]
    fn mov_recording_uses_qtmux_and_a_hidden_atomic_path() {
        gst::init().expect("GStreamer runtime");
        if gst::ElementFactory::find("qtmux").is_none() {
            return;
        }
        let plan = plan(OutputProfile::mov_h264_aac());
        let destination = ProductionDestination::Recording(PathBuf::from("capture.mov"));
        let pipeline_description = pipeline_description(&plan, &destination).expect("MOV graph");
        assert!(pipeline_description.description.contains("qtmux name=mux"));
        assert_eq!(
            pipeline_description.final_path,
            Some(PathBuf::from("capture.mov"))
        );
        assert_eq!(
            pipeline_description.temp_path,
            Some(PathBuf::from("capture.mov.part"))
        );
        assert!(pipeline_description.segmented_policy.is_none());
        gst::parse::launch_full(
            &pipeline_description.description,
            None,
            gst::ParseFlags::FATAL_ERRORS,
        )
        .expect("MOV pipeline parses");
    }

    #[test]
    fn fragmented_mp4_recording_uses_bounded_fragmented_muxing() {
        gst::init().expect("GStreamer runtime");
        if gst::ElementFactory::find("mp4mux").is_none() {
            return;
        }
        let plan = plan(OutputProfile::fragmented_mp4_h264_aac());
        let destination = ProductionDestination::Recording(PathBuf::from("capture.mp4"));
        let pipeline_description =
            pipeline_description(&plan, &destination).expect("fragmented MP4 graph");
        assert!(pipeline_description
            .description
            .contains("fragment-duration=1000"));
        assert!(pipeline_description.description.contains("streamable=true"));
        assert_eq!(
            pipeline_description.final_path,
            Some(PathBuf::from("capture.mp4"))
        );
        assert_eq!(
            pipeline_description.temp_path,
            Some(PathBuf::from("capture.mp4.part"))
        );
        assert!(pipeline_description.segmented_policy.is_none());
        gst::parse::launch_full(
            &pipeline_description.description,
            None,
            gst::ParseFlags::FATAL_ERRORS,
        )
        .expect("fragmented MP4 pipeline parses");
    }

    #[test]
    fn segmented_mp4_recording_uses_bounded_split_muxing_and_hidden_paths() {
        gst::init().expect("GStreamer runtime");
        if gst::ElementFactory::find("splitmuxsink").is_none()
            || gst::ElementFactory::find("mp4mux").is_none()
        {
            return;
        }
        let policy = SegmentedRecordingPolicy::new(1_000_000, std::time::Duration::from_secs(5), 3)
            .expect("segment policy");
        let base_path = std::env::temp_dir().join("obs-rs-segmented-capture.mp4");
        let destination = ProductionDestination::SegmentedRecording {
            base_path: base_path.clone(),
            policy,
        };
        let plan = plan(OutputProfile::mp4_h264_aac());
        let pipeline_description =
            pipeline_description(&plan, &destination).expect("segmented MP4 graph");
        assert!(pipeline_description
            .description
            .contains("splitmuxsink name=output_sink"));
        assert!(pipeline_description
            .description
            .contains("muxer-factory=mp4mux"));
        assert!(pipeline_description
            .description
            .contains("max-size-time=5000000000"));
        assert!(pipeline_description
            .description
            .contains("max-size-bytes=1000000"));
        assert!(pipeline_description.description.contains("max-files=3"));
        assert_eq!(pipeline_description.final_path, Some(base_path.clone()));
        assert!(pipeline_description.temp_path.is_none());
        assert_eq!(pipeline_description.segmented_policy, Some(policy));

        let element = gst::parse::launch_full(
            &pipeline_description.description,
            None,
            gst::ParseFlags::FATAL_ERRORS,
        )
        .expect("segmented MP4 pipeline parses");
        let pipeline = element.downcast::<gst::Pipeline>().expect("pipeline type");
        configure_sink(&pipeline, &destination, None).expect("segmented sink path");
        let sink = pipeline.by_name("output_sink").expect("split sink");
        assert_eq!(
            sink.property::<String>("location"),
            std::env::temp_dir()
                .join("obs-rs-segmented-capture-%05d.mp4.part")
                .to_string_lossy()
        );
    }

    #[test]
    fn native_segmented_mp4_rolls_over_and_publishes_bounded_files() {
        gst::init().expect("GStreamer runtime");
        if ["splitmuxsink", "mp4mux", "openh264enc", "avenc_aac"]
            .iter()
            .any(|element| gst::ElementFactory::find(element).is_none())
        {
            return;
        }
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let base_path = std::env::temp_dir().join(format!("obs-rs-native-segmented-{token}.mp4"));
        let policy =
            SegmentedRecordingPolicy::new(1_000_000, std::time::Duration::from_millis(500), 3)
                .expect("segment policy");
        let destination = ProductionDestination::SegmentedRecording {
            base_path: base_path.clone(),
            policy,
        };
        let mut plan = plan(OutputProfile::mp4_h264_aac());
        plan.video_config.keyframe_interval_secs = 1;
        let video = VideoFormat::new(
            64,
            64,
            obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
        )
        .expect("video format");
        let audio = AudioFormat::new(48_000, 2).expect("audio format");
        let mut session = GStreamerOutputSession::start(&plan, &destination, video, audio)
            .expect("segmented native session");
        for index in 0_u64..180 {
            let timestamp = Timestamp::from_nanos(index * 1_000_000_000 / 30);
            session
                .push_video(VideoFrame::solid(video, timestamp, [24, 96, 180, 255]))
                .expect("video submission");
            session
                .push_audio(AudioBuffer::silence(audio, timestamp, 1_600).expect("silence"))
                .expect("audio submission");
        }
        session.close().expect("segmented close");

        let mut published = 0_usize;
        for index in 1..=policy.max_segments() {
            let (final_path, temp_path) =
                segmented_recording_paths(&base_path, index).expect("segment paths");
            if final_path.exists() {
                published = published.saturating_add(1);
                assert!(
                    std::fs::metadata(&final_path)
                        .expect("segment metadata")
                        .len()
                        > 0
                );
            }
            assert!(
                !temp_path.exists(),
                "temporary segment must be hidden/cleaned"
            );
            let _ = std::fs::remove_file(final_path);
        }
        assert!(published >= 2, "expected muxer rollover, got {published}");
    }

    #[test]
    fn native_matroska_remux_publishes_mp4_without_replacing_existing_output() {
        gst::init().expect("GStreamer runtime");
        if [
            "matroskamux",
            "matroskademux",
            "mp4mux",
            "h264parse",
            "aacparse",
            "queue",
            "openh264enc",
            "avenc_aac",
            "filesrc",
            "filesink",
        ]
        .iter()
        .any(|element| gst::ElementFactory::find(element).is_none())
        {
            return;
        }
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let source = std::env::temp_dir().join(format!("obs-rs-remux-source-{token}.mkv"));
        let destination =
            std::env::temp_dir().join(format!("obs-rs-remux-destination-{token}.mp4"));
        let temporary = destination.with_extension("mp4.part");
        let plan = plan(OutputProfile::matroska_h264_aac());
        let video = VideoFormat::new(
            64,
            64,
            obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
        )
        .expect("video format");
        let audio = AudioFormat::new(48_000, 2).expect("audio format");
        let source_destination = ProductionDestination::Recording(source.clone());
        let mut session = GStreamerOutputSession::start(&plan, &source_destination, video, audio)
            .expect("Matroska source session");
        for index in 0_u64..60 {
            let timestamp = Timestamp::from_nanos(index * 1_000_000_000 / 30);
            session
                .push_video(VideoFrame::solid(video, timestamp, [24, 96, 180, 255]))
                .expect("video submission");
            session
                .push_audio(AudioBuffer::silence(audio, timestamp, 1_600).expect("silence"))
                .expect("audio submission");
        }
        session.close().expect("Matroska source close");
        assert!(source.is_file());

        let bytes = remux_matroska_to_mp4(&source, &destination).expect("remux Matroska");
        let persisted = std::fs::read(&destination).expect("read remuxed MP4");
        assert_eq!(persisted.len(), bytes);
        assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
        assert!(!temporary.exists());

        let existing = persisted.clone();
        let error = remux_matroska_to_mp4(&source, &destination)
            .expect_err("remux must not replace an existing destination");
        assert!(error.to_string().contains("without replacing"));
        assert_eq!(
            std::fs::read(&destination).expect("read preserved MP4"),
            existing
        );
        assert!(!temporary.exists());
        std::fs::remove_file(source).expect("remove Matroska source");
        std::fs::remove_file(destination).expect("remove remuxed MP4");
    }

    #[test]
    fn stale_recording_artifact_cleanup_is_bounded_and_typed() {
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let stale = std::env::temp_dir().join(format!("obs-rs-native-recovery-{token}.mkv.part"));
        std::fs::write(&stale, [1, 2, 3]).expect("write stale artifact");
        recover_stale_recording_artifact(Some(&stale)).expect("remove stale artifact");
        assert!(!stale.exists());
        recover_stale_recording_artifact(Some(&stale)).expect("missing artifact is harmless");

        std::fs::create_dir(&stale).expect("create invalid artifact");
        let error = recover_stale_recording_artifact(Some(&stale))
            .expect_err("directory must not be treated as a recording artifact");
        assert!(error
            .to_string()
            .contains("remove stale production recording artifact"));
        std::fs::remove_dir(&stale).expect("remove invalid artifact");
    }

    #[test]
    fn native_publication_is_no_clobber_and_rejects_empty_artifacts() {
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir();
        let temp = directory.join(format!("obs-rs-native-publish-{token}.part"));
        let final_path = directory.join(format!("obs-rs-native-publish-{token}.mp4"));
        std::fs::write(&temp, [1, 2, 3]).expect("write temporary recording");
        publish_recording_artifact(&temp, &final_path).expect("publish recording");
        assert_eq!(
            std::fs::read(&final_path).expect("read recording"),
            [1, 2, 3]
        );
        assert!(!temp.exists());

        std::fs::write(&temp, [4, 5, 6]).expect("write second temporary recording");
        let error = publish_recording_artifact(&temp, &final_path)
            .expect_err("existing final path must not be replaced");
        assert!(error.to_string().contains("without replacing"));
        assert_eq!(
            std::fs::read(&temp).expect("read preserved temporary"),
            [4, 5, 6]
        );
        std::fs::remove_file(&temp).expect("remove preserved temporary");

        let empty_final = directory.join(format!("obs-rs-native-publish-{token}-empty.mp4"));
        std::fs::write(&temp, []).expect("write empty recording");
        let error = publish_recording_artifact(&temp, &empty_final)
            .expect_err("empty final artifact must be rejected");
        assert!(error.to_string().contains("empty production recording"));
        std::fs::remove_file(&temp).expect("remove empty temporary");
        std::fs::remove_file(&final_path).expect("remove published recording");
    }

    #[test]
    fn hls_and_rist_sinks_receive_bounded_typed_configuration() {
        gst::init().expect("GStreamer runtime");
        let hls = ProductionDestination::Hls {
            directory: PathBuf::from("hls-output"),
            segment_duration_secs: 3,
            playlist_size: 5,
            low_latency: false,
        };
        let mut hls_plan = plan(OutputProfile::hls_h264_aac());
        hls_plan.atomic_recording = false;
        let hls_description = pipeline_description(&hls_plan, &hls).expect("HLS graph");
        assert!(hls_description.description.contains("hlssink2"));

        let rist = ProductionDestination::Rist {
            host: "127.0.0.1".to_owned(),
            port: 5_000,
            sender_buffer_ms: 750,
            shared_secret: None,
        };
        let rist_plan = plan(OutputProfile::rist_mpeg_ts_h264_aac());
        let rist_description = pipeline_description(&rist_plan, &rist).expect("RIST graph");
        assert!(rist_description.description.contains("mpegtsmux"));
        assert!(rist_description.description.contains("rtpmp2tpay"));
        let pipeline = gst::parse::launch(&rist_description.description)
            .expect("RIST pipeline")
            .downcast::<gst::Pipeline>()
            .expect("pipeline type");
        configure_sink(&pipeline, &rist, None).expect("RIST configuration");
        let sink = pipeline.by_name("output_sink").expect("RIST sink");
        assert_eq!(sink.property::<String>("address"), "127.0.0.1");
        assert_eq!(sink.property::<u32>("port"), 5_000);
        assert_eq!(sink.property::<u32>("sender-buffer"), 750);
    }

    #[test]
    fn live_session_is_live_and_enforces_reconnect_budget() {
        let plan = plan(OutputProfile::rtmp_h264_aac());
        let destination = ProductionDestination::Rtmp {
            endpoint: "rtmp://127.0.0.1:9/live/key".to_owned(),
        };
        let video = VideoFormat::new(
            16,
            16,
            obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
        )
        .expect("video format");
        let audio = AudioFormat::new(48_000, 2).expect("audio format");
        let mut session = GStreamerOutputSession::start_with_reconnect_policy(
            &plan,
            &destination,
            video,
            audio,
            ReconnectPolicy::immediate(1),
        )
        .expect("live session");

        assert!(session.video.is_live());
        assert!(session.audio.is_live());
        StreamingTransport::reconnect(&mut session).expect("first reconnect");
        assert_eq!(session.telemetry().reconnects(), 1);
        assert!(StreamingTransport::reconnect(&mut session).is_err());
        assert_eq!(session.state(), NativeOutputState::Failed);
    }

    #[test]
    fn native_reconnect_honors_a_bounded_deferred_retry() {
        let plan = plan(OutputProfile::rtmp_h264_aac());
        let destination = ProductionDestination::Rtmp {
            endpoint: "rtmp://127.0.0.1:9/live/key".to_owned(),
        };
        let video = VideoFormat::new(
            16,
            16,
            obs_rs_media::FrameRate::new(30, 1).expect("frame rate"),
        )
        .expect("video format");
        let audio = AudioFormat::new(48_000, 2).expect("audio format");
        let policy = ReconnectPolicy::with_backoff(
            2,
            std::time::Duration::from_millis(100),
            std::time::Duration::from_millis(250),
        );
        let mut session = GStreamerOutputSession::start_with_reconnect_policy(
            &plan,
            &destination,
            video,
            audio,
            policy,
        )
        .expect("live session");
        let now = Instant::now();
        session.schedule_reconnect(now);

        assert_eq!(
            session.reconnect_live_at(now),
            Ok(ReconnectOutcome::Deferred {
                retry_after: std::time::Duration::from_millis(100),
            })
        );
        assert_eq!(session.state(), NativeOutputState::Retrying);
        assert_eq!(
            session.reconnect_live_at(now + std::time::Duration::from_millis(100)),
            Ok(ReconnectOutcome::Reconnected)
        );
        assert_eq!(session.state(), NativeOutputState::Ready);
    }
}
