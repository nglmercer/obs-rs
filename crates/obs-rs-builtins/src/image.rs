use std::{
    fs::File,
    io::{Cursor, Read},
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
