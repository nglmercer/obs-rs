//! Bounded GStreamer-backed decoding for persistent Stinger resources.

use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use obs_rs_media::{
    MediaError, StingerClip, StingerLoadCancellation, StingerLoadRequest, StingerResourceFailure,
    StingerResourceLoader, VideoFrame, MAX_STINGER_FRAMES, MAX_STINGER_FRAME_DURATION_NANOS,
    MAX_STINGER_MEMORY_BYTES, MIN_STINGER_FRAME_DURATION_NANOS,
};

/// The loader never waits indefinitely for a broken native decoder.
const MAX_STINGER_DECODE_TIME: Duration = Duration::from_mins(2);
/// The bounded sample pull interval keeps cancellation responsive.
const STINGER_SAMPLE_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// One extra sample is pulled so a 257-frame resource is rejected instead of
/// silently truncated to the portable 256-frame clip limit.
const MAX_STINGER_SAMPLES: u32 = 257;

/// Native `GStreamer` implementation of [`StingerResourceLoader`].
///
/// The adapter is intentionally kept in the optional native crate. It opens
/// files and runs a decoder only on the caller's dedicated resource worker;
/// the returned [`StingerClip`] contains bounded RGBA frames and is safe to
/// publish to render/UI consumers.
#[derive(Clone, Copy, Debug, Default)]
pub struct GStreamerStingerLoader;

impl StingerResourceLoader for GStreamerStingerLoader {
    fn load(
        &mut self,
        request: &StingerLoadRequest,
        cancellation: &StingerLoadCancellation,
    ) -> Result<StingerClip, MediaError> {
        decode_stinger(request, cancellation)
    }
}

fn decode_stinger(
    request: &StingerLoadRequest,
    cancellation: &StingerLoadCancellation,
) -> Result<StingerClip, MediaError> {
    if cancellation.is_cancelled() {
        return Err(resource_error(StingerResourceFailure::Cancelled));
    }
    gst::init().map_err(|_| resource_error(StingerResourceFailure::DecoderUnavailable))?;

    let path = Path::new(request.spec().resource_path());
    let metadata =
        fs::metadata(path).map_err(|_| resource_error(StingerResourceFailure::Unreadable))?;
    if !metadata.is_file() {
        return Err(resource_error(StingerResourceFailure::Unreadable));
    }
    let location = path
        .to_str()
        .ok_or_else(|| resource_error(StingerResourceFailure::Unreadable))?;
    ensure_decoder_elements()?;

    let format = request.target_format();
    let caps = rgba_caps(format)?;
    let source = gst::ElementFactory::make("filesrc")
        .property("location", location)
        .build()
        .map_err(|_| resource_error(StingerResourceFailure::Decoder))?;
    let decoder = gst::ElementFactory::make("decodebin")
        .build()
        .map_err(|_| resource_error(StingerResourceFailure::Decoder))?;
    let converter = gst::ElementFactory::make("videoconvert")
        .build()
        .map_err(|_| resource_error(StingerResourceFailure::Decoder))?;
    let scaler = gst::ElementFactory::make("videoscale")
        .build()
        .map_err(|_| resource_error(StingerResourceFailure::Decoder))?;
    let framerate = gst::ElementFactory::make("videorate")
        .build()
        .map_err(|_| resource_error(StingerResourceFailure::Decoder))?;
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()
        .map_err(|_| resource_error(StingerResourceFailure::Decoder))?;
    let sink = gst_app::AppSink::builder()
        .caps(&caps)
        .max_buffers(MAX_STINGER_SAMPLES)
        .drop(false)
        .sync(false)
        .build();
    let sink_element = sink.clone().upcast::<gst::Element>();
    let pipeline = gst::Pipeline::new();
    pipeline
        .add_many([
            &source,
            &decoder,
            &converter,
            &scaler,
            &framerate,
            &capsfilter,
            &sink_element,
        ])
        .map_err(|_| resource_error(StingerResourceFailure::Decoder))?;
    gst::Element::link_many([&source, &decoder])
        .map_err(|_| resource_error(StingerResourceFailure::Decoder))?;
    gst::Element::link_many([&converter, &scaler, &framerate, &capsfilter, &sink_element])
        .map_err(|_| resource_error(StingerResourceFailure::Decoder))?;

    let converter_sink = converter
        .static_pad("sink")
        .ok_or_else(|| resource_error(StingerResourceFailure::Decoder))?;
    let link_error = Arc::new(Mutex::new(false));
    let link_error_for_callback = Arc::clone(&link_error);
    decoder.connect_pad_added(move |_decoder, source_pad| {
        if converter_sink.is_linked() {
            return;
        }
        let caps = source_pad
            .current_caps()
            .unwrap_or_else(|| source_pad.query_caps(None));
        let Some(structure) = caps.structure(0) else {
            return;
        };
        if !structure.name().starts_with("video/") {
            return;
        }
        if source_pad.link(&converter_sink).is_err() {
            if let Ok(mut failed) = link_error_for_callback.lock() {
                *failed = true;
            }
        }
    });

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|_| resource_error(StingerResourceFailure::Decoder))?;
    let decode_result = pull_frames(&sink, &pipeline, format, request, cancellation, &link_error);
    let stop_result = pipeline
        .set_state(gst::State::Null)
        .map_err(|_| resource_error(StingerResourceFailure::Decoder));
    match stop_result {
        Ok(_) => decode_result,
        Err(error) => Err(error),
    }
}

fn pull_frames(
    sink: &gst_app::AppSink,
    pipeline: &gst::Pipeline,
    format: obs_rs_media::VideoFormat,
    request: &StingerLoadRequest,
    cancellation: &StingerLoadCancellation,
    link_error: &Arc<Mutex<bool>>,
) -> Result<StingerClip, MediaError> {
    let fallback_duration = frame_duration_nanos(format);
    let deadline = Instant::now() + MAX_STINGER_DECODE_TIME;
    let mut frames = Vec::with_capacity(MAX_STINGER_FRAMES);
    let mut durations = Vec::with_capacity(MAX_STINGER_FRAMES);
    let mut resident_bytes = 0_usize;

    loop {
        if cancellation.is_cancelled() {
            return Err(resource_error(StingerResourceFailure::Cancelled));
        }
        if Instant::now() >= deadline {
            return Err(resource_error(StingerResourceFailure::Timeout));
        }
        if link_error.lock().is_ok_and(|failed| *failed) {
            return Err(resource_error(StingerResourceFailure::Decoder));
        }
        if let Some(message) = pipeline
            .bus()
            .and_then(|bus| bus.pop_filtered(&[gst::MessageType::Error]))
        {
            let _ = message;
            return Err(resource_error(StingerResourceFailure::Decoder));
        }

        let Some(sample) = sink.try_pull_sample(Some(gst::ClockTime::from_mseconds(
            u64::try_from(STINGER_SAMPLE_POLL_INTERVAL.as_millis()).unwrap_or(50),
        ))) else {
            if sink.is_eos() {
                break;
            }
            continue;
        };
        if frames.len() >= MAX_STINGER_FRAMES {
            return Err(MediaError::InvalidStingerFrameCount {
                count: frames.len().saturating_add(1),
            });
        }
        let buffer = sample
            .buffer()
            .ok_or_else(|| resource_error(StingerResourceFailure::InvalidFrame))?;
        let mapped = buffer
            .map_readable()
            .map_err(|_| resource_error(StingerResourceFailure::InvalidFrame))?;
        if mapped.len() != format.rgba_bytes() {
            return Err(resource_error(StingerResourceFailure::InvalidFrame));
        }
        resident_bytes = resident_bytes.saturating_add(mapped.len());
        if resident_bytes > MAX_STINGER_MEMORY_BYTES {
            return Err(MediaError::StingerTooLarge {
                bytes: resident_bytes,
            });
        }
        let pixels = mapped.as_slice().to_vec();
        drop(mapped);
        let timestamp = buffer.pts().map_or_else(
            || obs_rs_media::Timestamp::ZERO,
            |pts| obs_rs_media::Timestamp::from_nanos(pts.nseconds()),
        );
        let frame = VideoFrame::new(format, timestamp, pixels)?;
        let duration = buffer
            .duration()
            .map(gst::ClockTime::nseconds)
            .filter(|duration| *duration > 0)
            .unwrap_or(fallback_duration)
            .clamp(
                MIN_STINGER_FRAME_DURATION_NANOS,
                MAX_STINGER_FRAME_DURATION_NANOS,
            );
        frames.push(frame);
        durations.push(duration);
    }

    if frames.is_empty() {
        return Err(resource_error(StingerResourceFailure::NoVideoFrames));
    }
    StingerClip::new(frames, durations, request.spec().transition_point_milli())
}

fn ensure_decoder_elements() -> Result<(), MediaError> {
    for element in [
        "filesrc",
        "decodebin",
        "videoconvert",
        "videoscale",
        "videorate",
    ] {
        if gst::ElementFactory::find(element).is_none() {
            return Err(resource_error(StingerResourceFailure::DecoderUnavailable));
        }
    }
    Ok(())
}

fn rgba_caps(format: obs_rs_media::VideoFormat) -> Result<gst::Caps, MediaError> {
    Ok(gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .field(
            "width",
            i32::try_from(format.width()).map_err(|_| MediaError::FrameTooLarge)?,
        )
        .field(
            "height",
            i32::try_from(format.height()).map_err(|_| MediaError::FrameTooLarge)?,
        )
        .field(
            "framerate",
            gst::Fraction::new(
                i32::try_from(format.frame_rate().numerator())
                    .map_err(|_| MediaError::FrameTooLarge)?,
                i32::try_from(format.frame_rate().denominator())
                    .map_err(|_| MediaError::FrameTooLarge)?,
            ),
        )
        .build())
}

fn frame_duration_nanos(format: obs_rs_media::VideoFormat) -> u64 {
    1_000_000_000_u64
        .saturating_mul(u64::from(format.frame_rate().denominator()))
        .checked_div(u64::from(format.frame_rate().numerator()))
        .unwrap_or(MIN_STINGER_FRAME_DURATION_NANOS)
        .clamp(
            MIN_STINGER_FRAME_DURATION_NANOS,
            MAX_STINGER_FRAME_DURATION_NANOS,
        )
}

const fn resource_error(failure: StingerResourceFailure) -> MediaError {
    MediaError::StingerResource { failure }
}
