//! Source plugins that work without a platform or device dependency.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::sync::Arc;

use obs_rs_capture::{
    CaptureKind, SimulatedCaptureFactory, TestPatternFactory, CAMERA_CAPTURE_SOURCE_KIND,
    SCREEN_CAPTURE_SOURCE_KIND, WINDOW_CAPTURE_SOURCE_KIND,
};
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{
    Plugin, PluginError, PluginManifest, Source, SourceError, SourceFactory, VideoRequest,
};
use obs_rs_util::Identifier;

/// Stable kind identifier for the solid color source.
pub const COLOR_SOURCE_KIND: &str = "color_source";

/// Re-exported stable kind for the simulated camera source.
pub use obs_rs_capture::CAMERA_CAPTURE_SOURCE_KIND as BUILTIN_CAMERA_SOURCE_KIND;
/// Re-exported stable kind for the simulated screen source.
pub use obs_rs_capture::SCREEN_CAPTURE_SOURCE_KIND as BUILTIN_SCREEN_SOURCE_KIND;
/// Re-exported stable kind for the deterministic capture source.
pub use obs_rs_capture::TEST_PATTERN_SOURCE_KIND as BUILTIN_TEST_PATTERN_SOURCE_KIND;
/// Re-exported stable kind for the simulated window source.
pub use obs_rs_capture::WINDOW_CAPTURE_SOURCE_KIND as BUILTIN_WINDOW_SOURCE_KIND;

/// The built-in plugin bundle shipped with the headless engine.
pub struct BuiltinPlugin {
    manifest: PluginManifest,
    factories: Vec<Arc<dyn SourceFactory>>,
}

impl BuiltinPlugin {
    /// Creates the portable built-in plugin bundle.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] only if a built-in identifier or manifest cannot be
    /// constructed, which indicates a programming error in this crate.
    pub fn new() -> Result<Self, PluginError> {
        let manifest = PluginManifest::new("obs_rs_builtins", "OBS-RS built-in sources", "0.1.0")?;
        let color_factory = ColorSourceFactory::new()?;
        let test_pattern_factory = TestPatternFactory::new()?;
        let screen_factory =
            SimulatedCaptureFactory::new(SCREEN_CAPTURE_SOURCE_KIND, CaptureKind::Screen)?;
        let window_factory =
            SimulatedCaptureFactory::new(WINDOW_CAPTURE_SOURCE_KIND, CaptureKind::Window)?;
        let camera_factory =
            SimulatedCaptureFactory::new(CAMERA_CAPTURE_SOURCE_KIND, CaptureKind::Camera)?;

        Ok(Self {
            manifest,
            factories: vec![
                Arc::new(color_factory),
                Arc::new(test_pattern_factory),
                Arc::new(screen_factory),
                Arc::new(window_factory),
                Arc::new(camera_factory),
            ],
        })
    }
}

impl Plugin for BuiltinPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn source_factories(&self) -> Vec<Arc<dyn SourceFactory>> {
        self.factories.clone()
    }
}

struct ColorSourceFactory {
    kind: Identifier,
}

impl ColorSourceFactory {
    fn new() -> Result<Self, PluginError> {
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

struct ColorSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    color: [u8; 4],
}

impl ColorSource {
    fn from_settings(kind: Identifier, name: &str, settings: &Config) -> Result<Self, SourceError> {
        if name.trim().is_empty() {
            return Err(SourceError::invalid_setting("name", "source name is empty"));
        }

        let format = parse_format(settings)?;
        let color = parse_color(settings.get("color").unwrap_or("#000000FF"))?;

        Ok(Self {
            kind,
            name: name.to_owned(),
            format,
            color,
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
        self.color = color;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }

        Ok(Some(VideoFrame::solid(
            self.format,
            request.timestamp(),
            self.color,
        )))
    }
}

fn parse_format(settings: &Config) -> Result<VideoFormat, SourceError> {
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

fn parse_color(value: &str) -> Result<[u8; 4], SourceError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use obs_rs_media::Timestamp;
    use obs_rs_plugin_api::VideoRequest;

    fn settings(color: &str) -> Config {
        let mut config = Config::new();
        config.set("width", "2").expect("valid width");
        config.set("height", "2").expect("valid height");
        config.set("color", color).expect("valid color text");
        config
    }

    #[test]
    fn builtins_register_and_render_a_color_source() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");
        let factory = plugin
            .source_factories()
            .into_iter()
            .find(|factory| factory.kind().as_str() == COLOR_SOURCE_KIND)
            .expect("color factory");
        let mut source = factory
            .create("background", &settings("#102030FF"))
            .expect("valid source");
        let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("valid rate"))
            .expect("valid format");
        let frame = source
            .render(&VideoRequest::new(Timestamp::ZERO, format))
            .expect("render succeeds")
            .expect("color source always has a frame");

        assert_eq!(frame.pixel(0, 0), Some([0x10, 0x20, 0x30, 0xff]));
    }

    #[test]
    fn invalid_color_is_rejected_at_creation() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");
        let factory = plugin
            .source_factories()
            .into_iter()
            .find(|factory| factory.kind().as_str() == COLOR_SOURCE_KIND)
            .expect("color factory");

        assert!(matches!(
            factory.create("background", &settings("red")),
            Err(SourceError::InvalidSetting { key, .. }) if key == "color"
        ));
    }

    #[test]
    fn builtins_expose_the_capture_source_kind() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");

        assert!(plugin
            .source_factories()
            .iter()
            .any(|factory| factory.kind().as_str() == BUILTIN_TEST_PATTERN_SOURCE_KIND));
        assert_eq!(BUILTIN_TEST_PATTERN_SOURCE_KIND, "test_pattern");
    }

    #[test]
    fn builtins_expose_simulated_platform_capture_kinds() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");
        let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
        for kind in [
            BUILTIN_SCREEN_SOURCE_KIND,
            BUILTIN_WINDOW_SOURCE_KIND,
            BUILTIN_CAMERA_SOURCE_KIND,
        ] {
            let factory = plugin
                .source_factories()
                .into_iter()
                .find(|factory| factory.kind().as_str() == kind)
                .expect("capture factory");
            let mut source = factory
                .create("capture", &settings("#000000FF"))
                .expect("capture source");
            let frame = source
                .render(&VideoRequest::new(Timestamp::ZERO, format))
                .expect("render")
                .expect("frame");
            assert_eq!(frame.format(), format);
        }
    }
}
