use std::{fs, path::PathBuf, time::Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use obs_rs_audio::{AudioBuffer, AudioFormat};
use obs_rs_media::{PixelFormat, RawVideoFrame, Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::{EncoderPreset, OutputTransport, RateControl, VideoCodec};

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

/// Timestamp, queue/drop, reconnect, drift, keyframe, and submit timing data.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OutputSessionTelemetry {
    video_submitted: u64,
    audio_submitted: u64,
    dropped: u64,
    reconnects: u64,
    keyframes: u64,
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
    final_path: Option<PathBuf>,
    temp_path: Option<PathBuf>,
    transport: OutputTransport,
    video_duration: gst::ClockTime,
    maximum_reconnects: u32,
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
        gst::init().map_err(native_error)?;
        destination.validate_for(plan.profile())?;
        let (description, final_path, temp_path) = pipeline_description(plan, destination)?;
        let element = gst::parse::launch_full(&description, None, gst::ParseFlags::FATAL_ERRORS)
            .map_err(native_error)?;
        let pipeline = element.downcast::<gst::Pipeline>().map_err(|_| {
            GStreamerError::Native("GStreamer did not create a pipeline".to_owned())
        })?;
        if plan.profile().transport() != OutputTransport::WebRtc {
            configure_encoders(&pipeline, plan, video_format)?;
        }
        configure_sink(&pipeline, destination, temp_path.as_deref())?;
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
            final_path,
            temp_path,
            transport: plan.profile().transport(),
            video_duration,
            maximum_reconnects,
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
            OutputTransport::Matroska | OutputTransport::Hls
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
        if let (Some(temp), Some(final_path)) = (&self.temp_path, &self.final_path) {
            fs::rename(temp, final_path).map_err(|error| {
                GStreamerError::Native(format!("publish Matroska recording: {error}"))
            })?;
        }
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
            OutputTransport::Matroska | OutputTransport::Hls
        ) {
            self.state = NativeOutputState::Failed;
            return Err(GStreamerError::Native(
                "recording pipeline reported an asynchronous error".to_owned(),
            ));
        }
        self.reconnect_live()
    }

    /// Rebuilds a live transport after an application/network loss signal.
    ///
    /// # Errors
    ///
    /// Rejects recording sessions and reports a failed `GStreamer` state change.
    pub fn reconnect_live(&mut self) -> Result<(), GStreamerError> {
        if matches!(
            self.transport,
            OutputTransport::Matroska | OutputTransport::Hls
        ) {
            return Err(GStreamerError::Native(
                "file outputs cannot reconnect".to_owned(),
            ));
        }
        if self.telemetry.reconnects >= u64::from(self.maximum_reconnects) {
            self.state = NativeOutputState::Failed;
            return Err(GStreamerError::Native(
                "live output reconnect limit reached".to_owned(),
            ));
        }
        self.state = NativeOutputState::Retrying;
        self.pipeline
            .set_state(gst::State::Null)
            .map_err(native_error)?;
        if let Err(error) = self.pipeline.set_state(gst::State::Playing) {
            self.state = NativeOutputState::Failed;
            return Err(native_error(error));
        }
        self.telemetry.reconnects = self.telemetry.reconnects.saturating_add(1);
        self.state = NativeOutputState::Ready;
        Ok(())
    }

    fn ensure_ready(&self) -> Result<(), GStreamerError> {
        (self.state == NativeOutputState::Ready)
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
}

impl Drop for GStreamerOutputSession {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        if self.state != NativeOutputState::Closed {
            if let Some(temp) = &self.temp_path {
                let _ = fs::remove_file(temp);
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
        source.set_is_live(plan.profile().transport() != OutputTransport::Matroska);
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

fn pipeline_description(
    plan: &ProductionPipelinePlan,
    destination: &ProductionDestination,
) -> Result<(String, Option<PathBuf>, Option<PathBuf>), GStreamerError> {
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
    match (plan.profile().transport(), destination) {
        (OutputTransport::Matroska, ProductionDestination::Recording(final_path)) => {
            let temp = final_path.with_extension("mkv.part");
            let parser = match plan.video_config().codec {
                VideoCodec::H264 => "h264parse",
                VideoCodec::Hevc => "h265parse",
                VideoCodec::Av1 => "av1parse",
                codec => {
                    return Err(GStreamerError::Native(format!(
                        "unsupported Matroska video codec {codec:?}"
                    )))
                }
            };
            Ok((format!("{v}{parser} ! mux. {a}aacparse ! mux. matroskamux name=mux ! filesink name=output_sink"), Some(final_path.clone()), Some(temp)))
        }
        (OutputTransport::Rtmp, ProductionDestination::Rtmp { .. })
        | (OutputTransport::Rtmps, ProductionDestination::Rtmps { .. }) => {
            let sink = plan.rtmp_sink().ok_or_else(|| {
                GStreamerError::Native("negotiated RTMP sink is missing".to_owned())
            })?;
            Ok((format!("{v}h264parse config-interval=-1 ! mux. {a}aacparse ! mux. flvmux name=mux streamable=true ! {sink} name=output_sink"), None, None))
        }
        (OutputTransport::SrtMpegTs, ProductionDestination::Srt { .. }) =>
            Ok((format!("{v}h264parse config-interval=-1 ! mux. {a}aacparse ! mux. mpegtsmux name=mux ! srtsink name=output_sink"), None, None)),
        (OutputTransport::WebRtc, ProductionDestination::WebRtc { .. }) =>
            Ok((format!("whipclientsink name=output_sink appsrc name=video_source ! queue max-size-bytes={queue} leaky=downstream ! videoconvert ! output_sink. appsrc name=audio_source ! queue max-size-bytes={queue} leaky=downstream ! audioconvert ! audioresample ! output_sink."), None, None)),
        (OutputTransport::Hls, ProductionDestination::Hls { .. }) =>
            Ok((format!("hlssink2 name=output_sink {v}h264parse ! output_sink.video {a}aacparse ! output_sink.audio"), None, None)),
        (OutputTransport::RistMpegTs, ProductionDestination::Rist { .. }) =>
            Ok((format!("{v}h264parse config-interval=-1 ! mux. {a}aacparse ! mux. mpegtsmux name=mux ! rtpmp2tpay ! ristsink name=output_sink"), None, None)),
        _ => Err(GStreamerError::InvalidEndpoint("destination does not match pipeline".to_owned())),
    }
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

fn native_error(error: impl std::fmt::Display) -> GStreamerError {
    GStreamerError::Native(error.to_string())
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
            let (description, final_path, temp_path) =
                pipeline_description(&plan, &destination).expect("pipeline description");
            assert!(description.contains(mux));
            assert!(description.contains(sink));
            assert!(final_path.is_none() && temp_path.is_none());
            let element =
                gst::parse::launch_full(&description, None, gst::ParseFlags::FATAL_ERRORS)
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
        let (description, _, _) = pipeline_description(&plan, &destination).expect("description");
        let pipeline = gst::parse::launch(&description)
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
        let (description, _, _) = pipeline_description(&hls_plan, &hls).expect("HLS graph");
        assert!(description.contains("hlssink2"));

        let rist = ProductionDestination::Rist {
            host: "127.0.0.1".to_owned(),
            port: 5_000,
            sender_buffer_ms: 750,
            shared_secret: None,
        };
        let rist_plan = plan(OutputProfile::rist_mpeg_ts_h264_aac());
        let (description, _, _) = pipeline_description(&rist_plan, &rist).expect("RIST graph");
        assert!(description.contains("mpegtsmux"));
        assert!(description.contains("rtpmp2tpay"));
        let pipeline = gst::parse::launch(&description)
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
        let mut session = GStreamerOutputSession::start_with_reconnect_limit(
            &plan,
            &destination,
            video,
            audio,
            1,
        )
        .expect("live session");

        assert!(session.video.is_live());
        assert!(session.audio.is_live());
        session.reconnect_live().expect("first reconnect");
        assert_eq!(session.telemetry().reconnects(), 1);
        assert!(session.reconnect_live().is_err());
        assert_eq!(session.state(), NativeOutputState::Failed);
    }
}
