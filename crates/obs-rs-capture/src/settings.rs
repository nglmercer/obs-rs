use obs_rs_config::Config;
use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_plugin_api::SourceError;

use super::types::{CameraMode, CameraPixelFormat};

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

/// Reads an optional exact native camera mode from source settings.
///
/// `width` and `height` remain the normalized source output for compatibility
/// with the existing source API. Native camera selection uses its own keys so
/// changing the scene canvas cannot accidentally claim that a webcam supports
/// that size.
pub(crate) fn parse_camera_mode(settings: &Config) -> Result<Option<CameraMode>, SourceError> {
    let keys = [
        "capture_width",
        "capture_height",
        "capture_fps",
        "capture_pixel_format",
    ];
    let present = keys.iter().any(|key| settings.get(key).is_some());
    if !present {
        return Ok(None);
    }
    let width = parse_u32(settings, "capture_width")?;
    let height = parse_u32(settings, "capture_height")?;
    let fps = settings
        .get("capture_fps")
        .ok_or_else(|| SourceError::invalid_setting("capture_fps", "setting is required"))?;
    let pixel_format = settings
        .get("capture_pixel_format")
        .and_then(CameraPixelFormat::parse)
        .ok_or_else(|| {
            SourceError::invalid_setting(
                "capture_pixel_format",
                "expected a supported camera pixel format",
            )
        })?;
    let frame_rate = parse_frame_rate(fps)?;
    CameraMode::new(pixel_format, width, height, frame_rate)
        .map(Some)
        .map_err(|error| SourceError::invalid_setting("camera mode", error.to_string()))
}

fn parse_frame_rate(value: &str) -> Result<FrameRate, SourceError> {
    let (numerator, denominator) = value.split_once('/').map_or_else(
        || (value, "1"),
        |(numerator, denominator)| (numerator, denominator),
    );
    let numerator = numerator
        .parse::<u32>()
        .map_err(|error| SourceError::invalid_setting("capture_fps", error.to_string()))?;
    let denominator = denominator
        .parse::<u32>()
        .map_err(|error| SourceError::invalid_setting("capture_fps", error.to_string()))?;
    FrameRate::new(numerator, denominator)
        .map_err(|error| SourceError::invalid_setting("capture_fps", error.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_camera_mode_uses_dedicated_settings() {
        let mut settings = Config::new();
        settings.set("width", "1920").expect("width");
        settings.set("height", "1080").expect("height");
        settings
            .set("capture_width", "1280")
            .expect("capture width");
        settings
            .set("capture_height", "720")
            .expect("capture height");
        settings.set("capture_fps", "60").expect("capture fps");
        settings
            .set("capture_pixel_format", "MJPEG")
            .expect("capture format");

        let mode = parse_camera_mode(&settings)
            .expect("mode parses")
            .expect("mode is present");
        assert_eq!(mode.width(), 1280);
        assert_eq!(mode.height(), 720);
        assert_eq!(mode.frame_rate(), FrameRate::new(60, 1).expect("rate"));
        assert_eq!(mode.pixel_format(), CameraPixelFormat::Mjpeg);
        assert_eq!(
            parse_format(&settings).expect("output format").width(),
            1920
        );
    }

    #[test]
    fn native_camera_mode_is_optional_for_legacy_documents() {
        let mut settings = Config::new();
        settings.set("width", "640").expect("width");
        settings.set("height", "360").expect("height");
        assert_eq!(parse_camera_mode(&settings).expect("optional mode"), None);
    }
}
