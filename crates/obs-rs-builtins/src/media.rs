//! Native media-file playback source.
//!
//! The portable built-ins decode still images themselves, but a general media
//! source needs a demuxer, audio/video clocks, and codecs. Those are supplied
//! by the optional native GStreamer boundary. Audio is consumed by a fakesink
//! here while the engine owns the live audio routes separately.

use std::path::Path;

use crate::portable::parse_format;
use obs_rs_config::Config;
#[cfg(feature = "production-gstreamer")]
use obs_rs_media::Timestamp;
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

/// Stable kind identifier for the native media-file source.
pub const MEDIA_SOURCE_KIND: &str = "media_source";

const MAX_MEDIA_PATH_BYTES: usize = 4 * 1024;

pub(crate) struct MediaSourceFactory {
    kind: Identifier,
}

impl MediaSourceFactory {
    pub(crate) fn new() -> Result<Self, PluginError> {
        Ok(Self {
            kind: Identifier::new(MEDIA_SOURCE_KIND).map_err(PluginError::InvalidIdentifier)?,
        })
    }
}

impl SourceFactory for MediaSourceFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        Ok(Box::new(MediaSource::from_settings(
            self.kind.clone(),
            name,
            settings,
        )?))
    }
}

struct MediaSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    path: String,
    loop_media: bool,
    #[cfg(feature = "production-gstreamer")]
    playback: Option<Playback>,
}

impl MediaSource {
    fn from_settings(kind: Identifier, name: &str, settings: &Config) -> Result<Self, SourceError> {
        if name.trim().is_empty() {
            return Err(SourceError::invalid_setting("name", "source name is empty"));
        }
        let format = parse_format(settings)?;
        let (path, loop_media) = parse_media_settings(settings)?;
        #[cfg(feature = "production-gstreamer")]
        let playback = if path.is_empty() {
            None
        } else {
            Some(Playback::open(Path::new(&path), format)?)
        };
        Ok(Self {
            kind,
            name: name.to_owned(),
            format,
            path,
            loop_media,
            #[cfg(feature = "production-gstreamer")]
            playback,
        })
    }
}

impl Source for MediaSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_format(settings)?;
        let (path, loop_media) = parse_media_settings(settings)?;
        #[cfg(feature = "production-gstreamer")]
        let playback = if path.is_empty() {
            None
        } else {
            // Open the replacement before changing the live source. A failed
            // path, missing decoder, or invalid media file leaves the current
            // playback untouched.
            Some(Playback::open(Path::new(&path), format)?)
        };
        self.format = format;
        self.path = path;
        self.loop_media = loop_media;
        #[cfg(feature = "production-gstreamer")]
        {
            self.playback = playback;
        }
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        if self.path.is_empty() {
            return Ok(None);
        }

        #[cfg(feature = "production-gstreamer")]
        {
            let Some(playback) = self.playback.as_mut() else {
                return Ok(None);
            };
            return playback.next_frame(self.format, request.timestamp(), self.loop_media);
        }
        #[cfg(not(feature = "production-gstreamer"))]
        {
            Err(SourceError::Unavailable(
                "native media playback requires the production-gstreamer feature and runtime"
                    .to_owned(),
            ))
        }
    }
}

fn parse_media_settings(settings: &Config) -> Result<(String, bool), SourceError> {
    let path = settings.get("path").unwrap_or("").trim();
    if path.len() > MAX_MEDIA_PATH_BYTES || path.chars().any(char::is_control) {
        return Err(SourceError::invalid_setting(
            "path",
            format!("path must be at most {MAX_MEDIA_PATH_BYTES} bytes and contain no controls"),
        ));
    }
    if !path.is_empty() && !Path::new(path).is_file() {
        return Err(SourceError::invalid_setting(
            "path",
            "media path must name an existing file",
        ));
    }
    let loop_media = settings
        .get("loop")
        .unwrap_or("true")
        .parse::<bool>()
        .map_err(|error| SourceError::invalid_setting("loop", error.to_string()))?;
    Ok((path.to_owned(), loop_media))
}

#[cfg(feature = "production-gstreamer")]
use gstreamer as gst;
#[cfg(feature = "production-gstreamer")]
use gstreamer::prelude::*;
#[cfg(feature = "production-gstreamer")]
use gstreamer_app as gst_app;

#[cfg(feature = "production-gstreamer")]
struct Playback {
    pipeline: gst::Pipeline,
    playbin: gst::Element,
    sink: gst_app::AppSink,
    eos: bool,
}

#[cfg(feature = "production-gstreamer")]
impl Playback {
    fn open(path: &Path, format: VideoFormat) -> Result<Self, SourceError> {
        gst::init().map_err(|error| native_error("initialize GStreamer", error))?;
        let path = path
            .canonicalize()
            .map_err(|error| native_error("resolve media path", error))?;
        let uri = gst::glib::filename_to_uri(&path, None)
            .map_err(|error| native_error("convert media path to URI", error))?;
        let caps = video_caps(format)?;

        let converter = gst::ElementFactory::make("videoconvert")
            .build()
            .map_err(|error| native_error("create video converter", error))?;
        let scaler = gst::ElementFactory::make("videoscale")
            .build()
            .map_err(|error| native_error("create video scaler", error))?;
        let rate = gst::ElementFactory::make("videorate")
            .build()
            .map_err(|error| native_error("create video rate converter", error))?;
        let caps_filter = gst::ElementFactory::make("capsfilter")
            .property("caps", &caps)
            .build()
            .map_err(|error| native_error("create media caps filter", error))?;
        let sink = gst_app::AppSink::builder()
            .caps(&caps)
            .max_buffers(2)
            .drop(true)
            .sync(false)
            .wait_on_eos(false)
            .build();
        let sink_element = sink.clone().upcast::<gst::Element>();
        let video_bin = gst::Bin::with_name("obs_rs_media_video_sink");
        video_bin
            .add_many([
                converter.clone(),
                scaler.clone(),
                rate.clone(),
                caps_filter.clone(),
                sink_element,
            ])
            .map_err(|error| native_error("assemble media video sink", error))?;
        gst::Element::link_many([
            converter.clone(),
            scaler,
            rate,
            caps_filter,
            sink.clone().upcast::<gst::Element>(),
        ])
        .map_err(|error| native_error("link media video sink", error))?;
        let input_pad = converter
            .static_pad("sink")
            .ok_or_else(|| native_error("find media video sink pad", "pad is missing"))?;
        let ghost_pad = gst::GhostPad::with_target(&input_pad)
            .map_err(|error| native_error("create media video ghost pad", error))?;
        video_bin
            .add_pad(&ghost_pad)
            .map_err(|error| native_error("publish media video sink pad", error))?;

        let audio_sink = gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .build()
            .map_err(|error| native_error("create media audio sink", error))?;
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", uri.as_str())
            .property("video-sink", &video_bin)
            .property("audio-sink", &audio_sink)
            .build()
            .map_err(|error| native_error("create media playback", error))?;
        let pipeline = gst::Pipeline::new();
        pipeline
            .add(&playbin)
            .map_err(|error| native_error("attach media playback", error))?;
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|error| native_error("start media playback", error))?;
        Ok(Self {
            pipeline,
            playbin,
            sink,
            eos: false,
        })
    }

    fn next_frame(
        &mut self,
        format: VideoFormat,
        timestamp: Timestamp,
        loop_media: bool,
    ) -> Result<Option<VideoFrame>, SourceError> {
        self.poll_bus()?;
        if self.eos {
            if !loop_media {
                return Ok(None);
            }
            self.playbin
                .seek_simple(
                    gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                    gst::ClockTime::ZERO,
                )
                .map_err(|error| native_error("rewind media playback", error))?;
            self.eos = false;
        }

        let mut pixels = None;
        while let Some(sample) = self.sink.try_pull_sample(gst::ClockTime::ZERO) {
            let Some(buffer) = sample.buffer() else {
                continue;
            };
            let mapped = buffer
                .map_readable()
                .map_err(|error| native_error("map decoded media frame", error))?;
            pixels = Some(mapped.as_slice().to_vec());
        }
        self.poll_bus()?;
        let Some(pixels) = pixels else {
            return Ok(None);
        };
        VideoFrame::new(format, timestamp, pixels)
            .map(Some)
            .map_err(|error| native_error("validate decoded media frame", error))
    }

    fn poll_bus(&mut self) -> Result<(), SourceError> {
        let Some(bus) = self.pipeline.bus() else {
            return Err(native_error("read media pipeline bus", "bus is missing"));
        };
        while let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::ZERO,
            &[gst::MessageType::Eos, gst::MessageType::Error],
        ) {
            match message.view() {
                gst::MessageView::Eos(_) => self.eos = true,
                gst::MessageView::Error(error) => {
                    return Err(native_error("decode media file", error.error()));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(feature = "production-gstreamer")]
impl Drop for Playback {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(feature = "production-gstreamer")]
fn video_caps(format: VideoFormat) -> Result<gst::Caps, SourceError> {
    let width = i32::try_from(format.width())
        .map_err(|error| native_error("build media width caps", error))?;
    let height = i32::try_from(format.height())
        .map_err(|error| native_error("build media height caps", error))?;
    let numerator = i32::try_from(format.frame_rate().numerator())
        .map_err(|error| native_error("build media frame-rate caps", error))?;
    let denominator = i32::try_from(format.frame_rate().denominator())
        .map_err(|error| native_error("build media frame-rate caps", error))?;
    Ok(gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .field("width", width)
        .field("height", height)
        .field("framerate", gst::Fraction::new(numerator, denominator))
        .build())
}

#[cfg(feature = "production-gstreamer")]
fn native_error(context: &str, error: impl std::fmt::Display) -> SourceError {
    SourceError::Unavailable(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_media_path_is_an_idle_source_setting() {
        let settings = Config::new();
        assert_eq!(parse_media_settings(&settings), Ok((String::new(), true)));
    }

    #[test]
    fn media_loop_setting_is_strictly_boolean() {
        let mut settings = Config::new();
        settings.set("loop", "sometimes").expect("setting is valid");
        assert!(matches!(
            parse_media_settings(&settings),
            Err(SourceError::InvalidSetting { key, .. }) if key == "loop"
        ));
    }

    #[test]
    fn media_paths_reject_controls_and_missing_files() {
        let mut controls = Config::new();
        controls
            .set("path", "video\nfile.mp4")
            .expect("setting is valid");
        assert!(matches!(
            parse_media_settings(&controls),
            Err(SourceError::InvalidSetting { key, .. }) if key == "path"
        ));

        let mut missing = Config::new();
        missing
            .set("path", r"C:\definitely\missing\obs-rs-media.mp4")
            .expect("setting is valid");
        assert!(matches!(
            parse_media_settings(&missing),
            Err(SourceError::InvalidSetting { key, .. }) if key == "path"
        ));
    }
}
