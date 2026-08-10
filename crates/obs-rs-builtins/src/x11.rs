#![cfg(target_os = "linux")]

use crate::portable::parse_format;
use obs_rs_capture::{VideoCaptureDevice, X11CaptureDevice, X11_SCREEN_CAPTURE_SOURCE_KIND};
use obs_rs_config::Config;
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

pub(crate) struct X11CaptureFactory {
    kind: Identifier,
}

#[cfg(target_os = "linux")]
impl X11CaptureFactory {
    pub(crate) fn new() -> Result<Self, PluginError> {
        Ok(Self {
            kind: Identifier::new(X11_SCREEN_CAPTURE_SOURCE_KIND)
                .map_err(PluginError::InvalidIdentifier)?,
        })
    }
}

#[cfg(target_os = "linux")]
impl SourceFactory for X11CaptureFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        let format = parse_format(settings)?;
        let display = settings.get("display").unwrap_or(":0").to_owned();
        let device = x11_device(name, &display, format)?;
        Ok(Box::new(X11CaptureSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            format,
            device,
        }))
    }
}

#[cfg(target_os = "linux")]
struct X11CaptureSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    device: X11CaptureDevice,
}

#[cfg(target_os = "linux")]
impl Source for X11CaptureSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_format(settings)?;
        let display = settings.get("display").unwrap_or(":0").to_owned();
        let device = x11_device(&self.name, &display, format)?;
        self.format = format;
        self.device = device;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        self.device
            .next_frame(request.timestamp())
            .map_err(|error| SourceError::Unavailable(error.to_string()))
    }
}

#[cfg(target_os = "linux")]
fn x11_device(
    name: &str,
    display: &str,
    format: VideoFormat,
) -> Result<X11CaptureDevice, SourceError> {
    let mut device = X11CaptureDevice::connect(display, "x11-root", name)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    device
        .start(format)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    Ok(device)
}
