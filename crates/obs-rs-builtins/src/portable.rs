use crate::COLOR_SOURCE_KIND;
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

pub(crate) struct ColorSourceFactory {
    kind: Identifier,
}

impl ColorSourceFactory {
    pub(crate) fn new() -> Result<Self, PluginError> {
        let kind = Identifier::new(COLOR_SOURCE_KIND).map_err(PluginError::InvalidIdentifier)?;
        Ok(Self { kind })
    }
}

impl SourceFactory for ColorSourceFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        let source = ColorSource::from_settings(self.kind.clone(), name, settings)?;
        Ok(Box::new(source))
    }
}

pub(crate) struct ColorSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    frame: VideoFrame,
}

impl ColorSource {
    fn from_settings(kind: Identifier, name: &str, settings: &Config) -> Result<Self, SourceError> {
        if name.trim().is_empty() {
            return Err(SourceError::invalid_setting("name", "source name is empty"));
        }

        let format = parse_format(settings)?;
        let color = parse_color(settings.get("color").unwrap_or("#000000FF"))?;

        let frame = VideoFrame::solid(format, obs_rs_media::Timestamp::ZERO, color);
        Ok(Self {
            kind,
            name: name.to_owned(),
            format,
            frame,
        })
    }
}

impl Source for ColorSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_format(settings)?;
        let color = parse_color(settings.get("color").unwrap_or("#000000FF"))?;
        self.format = format;
        self.frame = VideoFrame::solid(format, obs_rs_media::Timestamp::ZERO, color);
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }

        Ok(Some(self.frame.at_timestamp(request.timestamp())))
    }
}

pub(crate) fn parse_format(settings: &Config) -> Result<VideoFormat, SourceError> {
    let width = parse_u32(settings, "width")?;
    let height = parse_u32(settings, "height")?;
    let numerator = parse_u32_with_default(settings, "fps_numerator", 30)?;
    let denominator = parse_u32_with_default(settings, "fps_denominator", 1)?;
    let frame_rate = FrameRate::new(numerator, denominator)
        .map_err(|error| SourceError::invalid_setting("fps", error.to_string()))?;
    VideoFormat::new(width, height, frame_rate)
        .map_err(|error| SourceError::invalid_setting("format", error.to_string()))
}

fn parse_u32(settings: &Config, key: &str) -> Result<u32, SourceError> {
    let value = settings
        .get(key)
        .ok_or_else(|| SourceError::invalid_setting(key, "setting is required"))?;
    value
        .parse::<u32>()
        .map_err(|error| SourceError::invalid_setting(key, error.to_string()))
}

fn parse_u32_with_default(settings: &Config, key: &str, default: u32) -> Result<u32, SourceError> {
    let Some(value) = settings.get(key) else {
        return Ok(default);
    };
    value
        .parse::<u32>()
        .map_err(|error| SourceError::invalid_setting(key, error.to_string()))
}

pub(crate) fn parse_color(value: &str) -> Result<[u8; 4], SourceError> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    if digits.len() != 6 && digits.len() != 8 {
        return Err(SourceError::invalid_setting(
            "color",
            "expected #RRGGBB or #RRGGBBAA",
        ));
    }

    let mut color = [0_u8; 4];
    for (index, chunk) in digits.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk)
            .map_err(|error| SourceError::invalid_setting("color", error.to_string()))?;
        color[index] = u8::from_str_radix(text, 16)
            .map_err(|error| SourceError::invalid_setting("color", error.to_string()))?;
    }
    if digits.len() == 6 {
        color[3] = 255;
    }
    Ok(color)
}
