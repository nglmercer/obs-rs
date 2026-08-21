use std::{
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use crate::{portable::parse_format, IMAGE_SOURCE_KIND};
use image::{imageops::FilterType, ImageReader, Limits};
use obs_rs_config::Config;
use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

/// Maximum encoded image payload read by the portable image source.
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum dimension accepted by an image decoder before resizing to the source frame.
const MAX_IMAGE_DIMENSION: u32 = 16_384;
/// Maximum decoder allocation budget, including the decoded image buffer.
const MAX_IMAGE_DECODE_BYTES: u64 = 128 * 1024 * 1024;
/// Maximum number of entries held by the portable slideshow source.
const MAX_SLIDESHOW_FILES: usize = 64;
/// Maximum directory entries inspected while expanding one slideshow path.
const MAX_SLIDESHOW_DIRECTORY_ENTRIES: usize = 4_096;
/// Maximum resident RGBA storage retained by one slideshow source.
const MAX_SLIDESHOW_MEMORY_BYTES: usize = 256 * 1024 * 1024;
/// OBS's lower and upper automatic slideshow interval bounds.
const MIN_SLIDE_TIME_MS: u64 = 50;
const MAX_SLIDE_TIME_MS: u64 = 3_600_000;

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

struct ImageSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    frame: Option<VideoFrame>,
}

impl ImageSource {
    fn from_settings(kind: Identifier, name: &str, settings: &Config) -> Result<Self, SourceError> {
        if name.trim().is_empty() {
            return Err(SourceError::invalid_setting("name", "source name is empty"));
        }
        let format = parse_format(settings)?;
        let frame = load_frame(format, settings.get("path").unwrap_or(""))?;
        Ok(Self {
            kind,
            name: name.to_owned(),
            format,
            frame,
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
        let frame = load_frame(format, settings.get("path").unwrap_or(""))?;
        self.format = format;
        self.frame = frame;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        Ok(self
            .frame
            .as_ref()
            .map(|frame| frame.at_timestamp(request.timestamp())))
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
}

impl ImageSlideshowSource {
    fn from_settings(kind: Identifier, name: &str, settings: &Config) -> Result<Self, SourceError> {
        if name.trim().is_empty() {
            return Err(SourceError::invalid_setting("name", "source name is empty"));
        }
        let format = parse_format(settings)?;
        let (frames, slide_time_nanos, loop_slides) = load_slideshow(format, settings)?;
        Ok(Self {
            kind,
            name: name.to_owned(),
            format,
            frames,
            slide_time_nanos,
            loop_slides,
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
        let (frames, slide_time_nanos, loop_slides) = load_slideshow(format, settings)?;
        self.format = format;
        self.frames = frames;
        self.slide_time_nanos = slide_time_nanos;
        self.loop_slides = loop_slides;
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
        let elapsed_slide = request.timestamp().as_nanos() / self.slide_time_nanos;
        let index = if self.loop_slides {
            elapsed_slide % count
        } else {
            elapsed_slide.min(count - 1)
        };
        let index = usize::try_from(index).unwrap_or(0);
        Ok(self
            .frames
            .get(index)
            .map(|frame| frame.at_timestamp(request.timestamp())))
    }
}

fn load_slideshow(
    format: VideoFormat,
    settings: &Config,
) -> Result<(Vec<VideoFrame>, u64, bool), SourceError> {
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
    Ok((frames, slide_time_nanos, loop_slides))
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

fn load_frame(format: VideoFormat, path: &str) -> Result<Option<VideoFrame>, SourceError> {
    if path.trim().is_empty() {
        // An empty path is a valid newly-created placeholder. The source stays
        // in the project and becomes live when its properties receive a path.
        return Ok(None);
    }

    let bytes = read_bounded_file(path)?;
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| SourceError::invalid_setting("path", error.to_string()))?;
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
