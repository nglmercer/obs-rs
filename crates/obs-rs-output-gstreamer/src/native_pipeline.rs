use std::{
    fs,
    path::{Path, PathBuf},
};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use obs_rs_audio::AudioFormat;
use obs_rs_media::{PixelFormat, VideoFormat};
use obs_rs_output::{
    EncoderPreset, OutputProfileKind, OutputTransport, RateControl, SegmentedRecordingPolicy,
    VideoCodec,
};

use super::super::{GStreamerError, ProductionDestination, ProductionPipelinePlan};
use super::{native_error, segmented_recording_paths, segmented_recording_pattern};

pub(super) fn appsrc(
    pipeline: &gst::Pipeline,
    name: &str,
) -> Result<gst_app::AppSrc, GStreamerError> {
    pipeline
        .by_name(name)
        .ok_or_else(|| GStreamerError::Native(format!("{name} is missing")))?
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| GStreamerError::Native(format!("{name} is not appsrc")))
}

pub(super) fn configure_sources(
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

pub(super) fn video_caps(
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

pub(super) struct PipelineDescription {
    pub(super) description: String,
    pub(super) final_path: Option<PathBuf>,
    pub(super) temp_path: Option<PathBuf>,
    pub(super) remux_final_path: Option<PathBuf>,
    pub(super) segmented_policy: Option<SegmentedRecordingPolicy>,
}

impl PipelineDescription {
    fn live(description: String) -> Self {
        Self {
            description,
            final_path: None,
            temp_path: None,
            remux_final_path: None,
            segmented_policy: None,
        }
    }

    fn recording(description: String, final_path: PathBuf, temp_path: PathBuf) -> Self {
        Self {
            description,
            final_path: Some(final_path),
            temp_path: Some(temp_path),
            remux_final_path: None,
            segmented_policy: None,
        }
    }

    fn remux(description: String, final_path: PathBuf) -> Self {
        Self {
            description,
            final_path: Some(final_path.clone()),
            temp_path: Some(final_path.with_extension("mkv.part")),
            remux_final_path: Some(final_path),
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
            remux_final_path: None,
            segmented_policy: Some(policy),
        }
    }
}

pub(super) fn pipeline_description(
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
            recording_pipeline_description(plan, &v, &a, final_path, false)
        }
        ProductionDestination::RemuxRecording { final_path } => {
            recording_pipeline_description(plan, &v, &a, final_path, true)
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
    remux: bool,
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
    if remux {
        Ok(PipelineDescription::remux(
            description,
            final_path.to_owned(),
        ))
    } else {
        Ok(PipelineDescription::recording(
            description,
            final_path.to_owned(),
            final_path.with_extension(format!("{extension}.part")),
        ))
    }
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
        (OutputTransport::WebRtc, ProductionDestination::WebRtc { .. }) => Ok(
            PipelineDescription::live(webrtc_pipeline_description(plan.bounded_queue_bytes())),
        ),
        (OutputTransport::Hls, ProductionDestination::Hls { .. }) =>
            Ok(PipelineDescription::live(format!("hlssink2 name=output_sink {video}h264parse ! output_sink.video {audio}aacparse ! output_sink.audio"))),
        (OutputTransport::RistMpegTs, ProductionDestination::Rist { .. }) =>
            Ok(PipelineDescription::live(format!("{video}h264parse config-interval=-1 ! mux. {audio}aacparse ! mux. mpegtsmux name=mux ! rtpmp2tpay ! ristsink name=output_sink"))),
        _ => Err(GStreamerError::InvalidEndpoint("destination does not match pipeline".to_owned())),
    }
}

/// Builds the raw audio/video graph consumed by `whipclientsink`.
///
/// The sink exposes request pads named `video_%u` and `audio_%u`.  Naming the
/// first request pads explicitly keeps the graph deterministic and avoids
/// relying on the shorthand `output_sink.` syntax, which is ambiguous for a
/// sink with multiple media pad templates.
fn webrtc_pipeline_description(queue: usize) -> String {
    format!(
        "whipclientsink name=output_sink "
            "appsrc name=video_source ! queue max-size-bytes={queue} leaky=downstream "
            "! videoconvert ! output_sink.video_0 "
            "appsrc name=audio_source ! queue max-size-bytes={queue} leaky=downstream "
            "! audioconvert ! audioresample ! output_sink.audio_0"
    )
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

pub(super) fn configure_encoders(
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

fn set_first_child_string_property(element: &gst::Element, names: &[&str], value: &str) -> bool {
    for name in names {
        if name.contains("::") {
            if element.lookup(name).is_ok() {
                element.set_child_property_from_str(name, value);
                return true;
            }
        } else if element.find_property(name).is_some() {
            element.set_property_from_str(name, value);
            return true;
        }
    }
    false
}

pub(super) fn configure_sink(
    pipeline: &gst::Pipeline,
    destination: &ProductionDestination,
    temp_path: Option<&std::path::Path>,
) -> Result<(), GStreamerError> {
    let sink = pipeline
        .by_name("output_sink")
        .ok_or_else(|| GStreamerError::Native("output sink is missing".to_owned()))?;
    match destination {
        ProductionDestination::Recording(_) | ProductionDestination::RemuxRecording { .. } => {
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
            if !set_first_child_string_property(
                &sink,
                &["signaller::whip-endpoint", "whip-endpoint", "endpoint"],
                signaling_endpoint,
            ) {
                return Err(GStreamerError::Native(
                    "WHIP sink does not expose a signaling endpoint property".to_owned(),
                ));
            }
            if let Some(token) = bearer_token {
                if !set_first_child_string_property(
                    &sink,
                    &["signaller::auth-token", "auth-token", "bearer-token"],
                    token,
                ) {
                    return Err(GStreamerError::Native(
                        "WHIP sink does not expose an authentication-token property".to_owned(),
                    ));
                }
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

pub(super) fn configure_segmented_location_callback(
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
