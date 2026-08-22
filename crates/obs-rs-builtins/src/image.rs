use std::{
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use crate::{portable::parse_format, IMAGE_SOURCE_KIND};
use image::{
    codecs::gif::GifDecoder, imageops::FilterType, AnimationDecoder, ImageDecoder, ImageFormat,
    ImageReader, Limits,
};
use obs_rs_config::Config;
use obs_rs_media::{FrameTransition, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

/// Maximum encoded image payload read by the portable image source.
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum dimension accepted by an image decoder before resizing to the source frame.
const MAX_IMAGE_DIMENSION: u32 = 16_384;
/// Maximum decoder allocation budget, including the decoded image buffer.
const MAX_IMAGE_DECODE_BYTES: u64 = 128 * 1024 * 1024;
/// Maximum decoded frames retained by one animated image source.
const MAX_ANIMATED_IMAGE_FRAMES: usize = 256;
/// Maximum resized RGBA storage retained by one animated image source.
const MAX_ANIMATED_IMAGE_MEMORY_BYTES: usize = 256 * 1024 * 1024;
/// GIFs with a zero delay still advance at a bounded visible cadence.
const MIN_ANIMATED_FRAME_TIME_NANOS: u64 = 10_000_000;
/// Maximum number of entries held by the portable slideshow source.
const MAX_SLIDESHOW_FILES: usize = 64;
/// Maximum directory entries inspected while expanding one slideshow path.
const MAX_SLIDESHOW_DIRECTORY_ENTRIES: usize = 4_096;
/// Maximum resident RGBA storage retained by one slideshow source.
const MAX_SLIDESHOW_MEMORY_BYTES: usize = 256 * 1024 * 1024;
/// OBS's lower and upper automatic slideshow interval bounds.
const MIN_SLIDE_TIME_MS: u64 = 50;
const MAX_SLIDE_TIME_MS: u64 = 3_600_000;
/// Maximum duration of the optional slideshow cross-fade.
const MAX_SLIDE_TRANSITION_MS: u64 = 60_000;

pub(crate) struct ImageSourceFactory {
    kind: Identifier,
}

impl ImageSourceFactory {
    pub(crate) fn new() -> Result<Self, PluginError> {
        let kind = Identifier::new(IMAGE_SOURCE_KIND).map_err(PluginError::InvalidIdentifier)?;
        Ok(Self { kind })
    }
}

impl SourceFactory for ImageSourceFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        let source = ImageSource::from_settings(self.kind.clone(), name, settings)?;
        Ok(Box::new(source))
    }
}

struct AnimatedImageFrame {
    frame: VideoFrame,
    duration_nanos: u64,
}

struct ImageSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    frames: Vec<AnimatedImageFrame>,
    animation_duration_nanos: u64,
}

impl ImageSource {
    fn from_settings(kind: Identifier, name: &str, settings: &Config) -> Result<Self, SourceError> {
        if name.trim().is_empty() {
            return Err(SourceError::invalid_setting("name", "source name is empty"));
        }
        let format = parse_format(settings)?;
        let (frames, animation_duration_nanos) =
            load_image_frames(format, settings.get("path").unwrap_or(""))?;
        Ok(Self {
            kind,
            name: name.to_owned(),
            format,
            frames,
            animation_duration_nanos,
        })
    }
}

impl Source for ImageSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        // Decode before changing the live frame, so a failed file update leaves
        // the last valid image visible instead of tearing the source down.
        let format = parse_format(settings)?;
        let (frames, animation_duration_nanos) =
            load_image_frames(format, settings.get("path").unwrap_or(""))?;
        self.format = format;
        self.frames = frames;
        self.animation_duration_nanos = animation_duration_nanos;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        if self.frames.is_empty() {
            return Ok(None);
        }
        let position = if self.frames.len() == 1 || self.animation_duration_nanos == 0 {
            0
        } else {
            request.timestamp().as_nanos() % self.animation_duration_nanos
        };
        let mut remaining = position;
        let Some(last) = self.frames.last() else {
            return Ok(None);
        };
        let selected = self
            .frames
            .iter()
            .find(|frame| {
                if remaining < frame.duration_nanos {
                    true
                } else {
                    remaining = remaining.saturating_sub(frame.duration_nanos);
                    false
                }
            })
            .unwrap_or(last);
        Ok(Some(selected.frame.at_timestamp(request.timestamp())))
    }
}

pub(crate) struct ImageSlideshowSourceFactory {
    kind: Identifier,
}

impl ImageSlideshowSourceFactory {
    pub(crate) fn new() -> Result<Self, PluginError> {
        let kind = Identifier::new(crate::IMAGE_SLIDESHOW_SOURCE_KIND)
            .map_err(PluginError::InvalidIdentifier)?;
        Ok(Self { kind })
    }
}

impl SourceFactory for ImageSlideshowSourceFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        Ok(Box::new(ImageSlideshowSource::from_settings(
            self.kind.clone(),
            name,
            settings,
        )?))
    }
}

struct ImageSlideshowSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    frames: Vec<VideoFrame>,
    slide_time_nanos: u64,
    loop_slides: bool,
    fade_transition: bool,
    transition_nanos: u64,
}

impl ImageSlideshowSource {
    fn from_settings(kind: Identifier, name: &str, settings: &Config) -> Result<Self, SourceError> {
        if name.trim().is_empty() {
            return Err(SourceError::invalid_setting("name", "source name is empty"));
        }
        let format = parse_format(settings)?;
        let (frames, slide_time_nanos, loop_slides, fade_transition, transition_nanos) =
            load_slideshow(format, settings)?;
        Ok(Self {
            kind,
            name: name.to_owned(),
            format,
            frames,
            slide_time_nanos,
            loop_slides,
            fade_transition,
            transition_nanos,
        })
    }
}

impl Source for ImageSlideshowSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_format(settings)?;
        let (frames, slide_time_nanos, loop_slides, fade_transition, transition_nanos) =
            load_slideshow(format, settings)?;
        self.format = format;
        self.frames = frames;
        self.slide_time_nanos = slide_time_nanos;
        self.loop_slides = loop_slides;
        self.fade_transition = fade_transition;
        self.transition_nanos = transition_nanos;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        let Some(count) = u64::try_from(self.frames.len()).ok() else {
            return Ok(None);
        };
        if count == 0 {
            return Ok(None);
        }
        let timestamp = request.timestamp();
        let elapsed_nanos = timestamp.as_nanos();
        let elapsed_slide = elapsed_nanos / self.slide_time_nanos;
        let index = if self.loop_slides {
            elapsed_slide % count
        } else {
            elapsed_slide.min(count - 1)
        };
        let index = usize::try_from(index).unwrap_or(0);
        let Some(current) = self.frames.get(index) else {
            return Ok(None);
        };
        if !self.fade_transition || self.transition_nanos == 0 || count < 2 {
            return Ok(Some(current.at_timestamp(timestamp)));
        }

        let position = elapsed_nanos % self.slide_time_nanos;
        let transition_start = self.slide_time_nanos - self.transition_nanos;
        let next_index = if position >= transition_start {
            if self.loop_slides {
                Some((index + 1) % self.frames.len())
            } else if index + 1 < self.frames.len() {
                Some(index + 1)
            } else {
                None
            }
        } else {
            None
        };
        let Some(next_index) = next_index else {
            return Ok(Some(current.at_timestamp(timestamp)));
        };
        let Some(next) = self.frames.get(next_index) else {
            return Ok(Some(current.at_timestamp(timestamp)));
        };
        let progress_nanos = position.saturating_sub(transition_start);
        let progress_milli = progress_nanos
            .saturating_mul(1_000)
            .checked_div(self.transition_nanos)
            .unwrap_or(1_000)
            .min(1_000);
        let progress_milli = u16::try_from(progress_milli).unwrap_or(1_000);
        VideoFrame::transitioned(
            &current.at_timestamp(timestamp),
            next.at_timestamp(timestamp),
            FrameTransition::CrossFade { progress_milli },
        )
        .map(Some)
        .map_err(|error| SourceError::Unavailable(format!("slideshow transition failed: {error}")))
    }
}

fn load_slideshow(
    format: VideoFormat,
    settings: &Config,
) -> Result<(Vec<VideoFrame>, u64, bool, bool, u64), SourceError> {
    let slide_time_ms = settings
        .get("slide_time_ms")
        .unwrap_or("8000")
        .parse::<u64>()
        .map_err(|error| SourceError::invalid_setting("slide_time_ms", error.to_string()))?;
    if !(MIN_SLIDE_TIME_MS..=MAX_SLIDE_TIME_MS).contains(&slide_time_ms) {
        return Err(SourceError::invalid_setting(
            "slide_time_ms",
            format!(
                "value must be between {MIN_SLIDE_TIME_MS} and {MAX_SLIDE_TIME_MS} milliseconds"
            ),
        ));
    }
    let fade_transition = settings
        .get("fade")
        .unwrap_or("false")
        .parse::<bool>()
        .map_err(|error| SourceError::invalid_setting("fade", error.to_string()))?;
    let transition_ms = settings
        .get("transition_ms")
        .unwrap_or("500")
        .parse::<u64>()
        .map_err(|error| SourceError::invalid_setting("transition_ms", error.to_string()))?;
    if transition_ms > MAX_SLIDE_TRANSITION_MS || transition_ms > slide_time_ms {
        return Err(SourceError::invalid_setting(
            "transition_ms",
            format!(
                "value must be between 0 and {MAX_SLIDE_TRANSITION_MS} milliseconds and no longer than slide_time_ms"
            ),
        ));
    }
    let loop_slides = settings
        .get("loop")
        .unwrap_or("true")
        .parse::<bool>()
        .map_err(|error| SourceError::invalid_setting("loop", error.to_string()))?;
    let randomize = settings
        .get("randomize")
        .unwrap_or("false")
        .parse::<bool>()
        .map_err(|error| SourceError::invalid_setting("randomize", error.to_string()))?;
    let mut paths = expand_slideshow_paths(settings)?;
    if randomize {
        shuffle_slideshow_paths(&mut paths);
    }
    let mut frames = Vec::new();
    let mut resident_bytes = 0_usize;
    for path in paths {
        let path = path.to_str().ok_or_else(|| {
            SourceError::invalid_setting("paths", "slideshow paths must be valid UTF-8")
        })?;
        let frame = load_frame(format, path)?.ok_or_else(|| {
            SourceError::invalid_setting("paths", "slideshow entries must name an image file")
        })?;
        resident_bytes = resident_bytes.saturating_add(frame.pixels().len());
        if resident_bytes > MAX_SLIDESHOW_MEMORY_BYTES {
            return Err(SourceError::invalid_setting(
                "paths",
                format!(
                    "decoded slideshow frames exceed the {MAX_SLIDESHOW_MEMORY_BYTES}-byte limit"
                ),
            ));
        }
        frames.push(frame);
    }
    let slide_time_nanos = slide_time_ms.saturating_mul(1_000_000);
    let transition_nanos = transition_ms.saturating_mul(1_000_000);
    Ok((
        frames,
        slide_time_nanos,
        loop_slides,
        fade_transition,
        transition_nanos,
    ))
}

fn expand_slideshow_paths(settings: &Config) -> Result<Vec<PathBuf>, SourceError> {
    let mut paths = Vec::new();
    for configured in settings
        .get("paths")
        .or_else(|| settings.get("path"))
        .into_iter()
        .flat_map(str::lines)
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        let path = PathBuf::from(configured);
        if !path.is_dir() {
            if paths.len() >= MAX_SLIDESHOW_FILES {
                return Err(SourceError::invalid_setting(
                    "paths",
                    format!("slideshow is limited to {MAX_SLIDESHOW_FILES} files"),
                ));
            }
            paths.push(path);
            continue;
        }

        let entries = std::fs::read_dir(&path).map_err(|error| {
            SourceError::invalid_setting("paths", format!("cannot read directory: {error}"))
        })?;
        let mut directory_paths = Vec::new();
        for (index, entry) in entries.enumerate() {
            if index >= MAX_SLIDESHOW_DIRECTORY_ENTRIES {
                return Err(SourceError::invalid_setting(
                    "paths",
                    format!(
                        "directory expansion is limited to {MAX_SLIDESHOW_DIRECTORY_ENTRIES} entries"
                    ),
                ));
            }
            let entry = entry.map_err(|error| {
                SourceError::invalid_setting("paths", format!("cannot inspect directory: {error}"))
            })?;
            let candidate = entry.path();
            let is_file = entry
                .file_type()
                .map_err(|error| {
                    SourceError::invalid_setting(
                        "paths",
                        format!("cannot inspect slideshow entry: {error}"),
                    )
                })?
                .is_file();
            if !is_file || !is_supported_image_path(&candidate) {
                continue;
            }
            if paths.len().saturating_add(directory_paths.len()) >= MAX_SLIDESHOW_FILES {
                return Err(SourceError::invalid_setting(
                    "paths",
                    format!("slideshow is limited to {MAX_SLIDESHOW_FILES} files"),
                ));
            }
            directory_paths.push(candidate);
        }
        directory_paths
            .sort_unstable_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
        paths.extend(directory_paths);
    }
    Ok(paths)
}

fn is_supported_image_path(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    [
        "gif", "jpeg", "jpg", "pam", "pbm", "pgm", "png", "pnm", "ppm", "webp",
    ]
    .iter()
    .any(|supported| extension.eq_ignore_ascii_case(supported))
}

fn shuffle_slideshow_paths(paths: &mut [PathBuf]) {
    if paths.len() < 2 {
        return;
    }
    let original = paths.to_vec();
    let mut state = paths.iter().fold(0xcbf2_9ce4_8422_2325_u64, |state, path| {
        path.to_string_lossy().bytes().fold(state, |state, byte| {
            state
                .wrapping_mul(0x0000_0100_0000_01b3)
                .wrapping_add(u64::from(byte))
        })
    });
    if state == 0 {
        state = 0x9e37_79b9_7f4a_7c15;
    }
    for index in (1..paths.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let bound = u64::try_from(index + 1).unwrap_or(u64::MAX);
        let swap = usize::try_from(state % bound).unwrap_or(0);
        paths.swap(index, swap);
    }
    if paths == original.as_slice() {
        paths.swap(0, 1);
    }
}

fn load_image_frames(
    format: VideoFormat,
    path: &str,
) -> Result<(Vec<AnimatedImageFrame>, u64), SourceError> {
    if path.trim().is_empty() {
        return Ok((Vec::new(), 0));
    }

    let bytes = read_bounded_file(path)?;
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| SourceError::invalid_setting("path", error.to_string()))?;
    if reader.format() != Some(ImageFormat::Gif) {
        let frame = decode_static_frame(format, reader)?
            .ok_or_else(|| SourceError::invalid_setting("path", "image has no frame"))?;
        return Ok((
            vec![AnimatedImageFrame {
                frame,
                duration_nanos: 0,
            }],
            0,
        ));
    }

    let mut decoder = GifDecoder::new(reader.into_inner())
        .map_err(|error| SourceError::invalid_setting("path", error.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_DECODE_BYTES);
    decoder
        .set_limits(limits)
        .map_err(|error| SourceError::invalid_setting("path", error.to_string()))?;

    let mut frames = Vec::new();
    let mut resident_bytes = 0_usize;
    let mut animation_duration_nanos = 0_u64;
    for (index, frame_result) in decoder.into_frames().enumerate() {
        if index >= MAX_ANIMATED_IMAGE_FRAMES {
            return Err(SourceError::invalid_setting(
                "path",
                format!("animated image is limited to {MAX_ANIMATED_IMAGE_FRAMES} frames"),
            ));
        }
        let gif_frame = frame_result
            .map_err(|error| SourceError::invalid_setting("path", error.to_string()))?;
        let duration_nanos = animated_frame_duration_nanos(gif_frame.delay());
        let rgba = image::DynamicImage::ImageRgba8(gif_frame.into_buffer())
            .resize_exact(format.width(), format.height(), FilterType::Triangle)
            .into_rgba8();
        resident_bytes = resident_bytes.saturating_add(rgba.as_raw().len());
        if resident_bytes > MAX_ANIMATED_IMAGE_MEMORY_BYTES {
            return Err(SourceError::invalid_setting(
                "path",
                format!(
                    "decoded animated frames exceed the {MAX_ANIMATED_IMAGE_MEMORY_BYTES}-byte limit"
                ),
            ));
        }
        let frame = VideoFrame::new(format, Timestamp::ZERO, rgba.into_raw())
            .map_err(|error| SourceError::invalid_setting("path", error.to_string()))?;
        animation_duration_nanos = animation_duration_nanos.saturating_add(duration_nanos);
        frames.push(AnimatedImageFrame {
            frame,
            duration_nanos,
        });
    }

    if frames.is_empty() {
        return Err(SourceError::invalid_setting(
            "path",
            "animated image has no frames",
        ));
    }
    Ok((frames, animation_duration_nanos))
}

fn animated_frame_duration_nanos(delay: image::Delay) -> u64 {
    let (numerator, denominator) = delay.numer_denom_ms();
    u64::from(numerator)
        .saturating_mul(1_000_000)
        .checked_div(u64::from(denominator))
        .unwrap_or(0)
        .max(MIN_ANIMATED_FRAME_TIME_NANOS)
}

fn load_frame(format: VideoFormat, path: &str) -> Result<Option<VideoFrame>, SourceError> {
    if path.trim().is_empty() {
        // An empty path is a valid newly-created placeholder. The source stays
        // in the project and becomes live when its properties receive a path.
        return Ok(None);
    }

    let bytes = read_bounded_file(path)?;
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| SourceError::invalid_setting("path", error.to_string()))?;
    decode_static_frame(format, reader)
}

fn decode_static_frame(
    format: VideoFormat,
    mut reader: ImageReader<Cursor<Vec<u8>>>,
) -> Result<Option<VideoFrame>, SourceError> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_DECODE_BYTES);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|error| SourceError::invalid_setting("path", error.to_string()))?;
    let rgba = decoded
        .resize_exact(format.width(), format.height(), FilterType::Triangle)
        .into_rgba8();
    VideoFrame::new(format, Timestamp::ZERO, rgba.into_raw())
        .map(Some)
        .map_err(|error| SourceError::invalid_setting("path", error.to_string()))
}

fn read_bounded_file(path: &str) -> Result<Vec<u8>, SourceError> {
    let file = File::open(path)
        .map_err(|error| SourceError::invalid_setting("path", error.to_string()))?;
    let read_limit = u64::try_from(MAX_IMAGE_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::new();
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|error| SourceError::invalid_setting("path", error.to_string()))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(SourceError::invalid_setting(
            "path",
            format!("image exceeds the {MAX_IMAGE_BYTES}-byte limit"),
        ));
    }
    Ok(bytes)
}
