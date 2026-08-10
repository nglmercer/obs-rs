use obs_rs_config::Config;
use obs_rs_media::{FrameRate, VideoFormat};

use super::{error::SandboxError, validation::invalid_manifest};

pub(crate) fn parse_video_format(settings: &Config) -> Result<VideoFormat, SandboxError> {
    let width = setting_u32(settings, "width")?;
    let height = setting_u32(settings, "height")?;
    let numerator = setting_u32_or(settings, "fps_numerator", 30)?;
    let denominator = setting_u32_or(settings, "fps_denominator", 1)?;
    let rate = FrameRate::new(numerator, denominator)?;
    VideoFormat::new(width, height, rate).map_err(SandboxError::Media)
}

fn setting_u32(settings: &Config, key: &str) -> Result<u32, SandboxError> {
    let value = settings
        .get(key)
        .ok_or_else(|| invalid_manifest(format!("sandbox source setting {key} is required")))?;
    value
        .parse::<u32>()
        .map_err(|_| invalid_manifest(format!("sandbox source setting {key} is invalid")))
}

fn setting_u32_or(settings: &Config, key: &str, default: u32) -> Result<u32, SandboxError> {
    settings.get(key).map_or(Ok(default), |value| {
        value
            .parse::<u32>()
            .map_err(|_| invalid_manifest(format!("sandbox source setting {key} is invalid")))
    })
}
