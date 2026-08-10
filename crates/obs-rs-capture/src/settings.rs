use obs_rs_config::Config;
use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_plugin_api::SourceError;

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
