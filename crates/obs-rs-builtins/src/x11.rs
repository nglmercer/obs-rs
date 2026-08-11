#![cfg(target_os = "linux")]

use crate::portable::parse_format;
use obs_rs_capture::{
    CaptureKind, SimulatedCaptureDevice, VideoCaptureDevice, X11CaptureDevice,
    X11_SCREEN_CAPTURE_SOURCE_KIND,
};
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
        let device = x11_device(name, &display, format)
            .map(X11CaptureBackend::Native)
            .or_else(|_| fallback_device(name, format).map(X11CaptureBackend::Fallback))?;
        Ok(Box::new(X11CaptureSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            display,
            format,
            device,
            fallback_frames: 0,
        }))
    }
}

#[cfg(target_os = "linux")]
struct X11CaptureSource {
    kind: Identifier,
    name: String,
    display: String,
    format: VideoFormat,
    device: X11CaptureBackend,
    fallback_frames: u64,
}

enum X11CaptureBackend {
    Native(X11CaptureDevice),
    Fallback(SimulatedCaptureDevice),
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
        let device = x11_device(&self.name, &display, format)
            .map(X11CaptureBackend::Native)
            .or_else(|_| fallback_device(&self.name, format).map(X11CaptureBackend::Fallback))?;
        self.display = display;
        self.format = format;
        self.device = device;
        self.fallback_frames = 0;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        if matches!(&self.device, X11CaptureBackend::Fallback(_)) {
            self.fallback_frames = self.fallback_frames.saturating_add(1);
            if self.fallback_frames.is_multiple_of(120) {
                if let Ok(device) = x11_device(&self.name, &self.display, self.format) {
                    self.device = X11CaptureBackend::Native(device);
                }
            }
        }

        for attempt in 0..2 {
            let result = match &mut self.device {
                X11CaptureBackend::Native(device) => device.next_frame(request.timestamp()),
                X11CaptureBackend::Fallback(device) => device.next_frame(request.timestamp()),
            };
            match result {
                Ok(frame) => return Ok(frame),
                Err(_error) if attempt == 0 => {
                    self.device =
                        X11CaptureBackend::Fallback(fallback_device(&self.name, self.format)?);
                }
                Err(error) => return Err(SourceError::Unavailable(error.to_string())),
            }
        }
        Err(SourceError::Unavailable(
            "X11 capture did not produce a frame".to_owned(),
        ))
    }
}

fn fallback_device(name: &str, format: VideoFormat) -> Result<SimulatedCaptureDevice, SourceError> {
    let mut device =
        SimulatedCaptureDevice::new(X11_SCREEN_CAPTURE_SOURCE_KIND, name, CaptureKind::Screen)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    device
        .start(format)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    Ok(device)
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
