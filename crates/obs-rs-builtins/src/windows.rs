#![cfg(target_os = "windows")]

//! Windows Graphics Capture source factories.

use crate::portable::parse_format;
use obs_rs_capture::{
    CaptureKind, CaptureLifecycleState, CaptureRequest, PlatformCaptureAdapter,
    ThreadedCaptureDevice, VideoCaptureDevice, SCREEN_CAPTURE_SOURCE_KIND,
    WINDOW_CAPTURE_SOURCE_KIND,
};
use obs_rs_capture_windows::WindowsCaptureAdapter;
use obs_rs_config::Config;
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsTarget {
    Screen,
    Window,
}

pub(crate) struct WindowsCaptureFactory {
    kind: Identifier,
    target: WindowsTarget,
}

impl WindowsCaptureFactory {
    pub(crate) fn screen() -> Result<Self, PluginError> {
        Self::new(SCREEN_CAPTURE_SOURCE_KIND, WindowsTarget::Screen)
    }

    pub(crate) fn window() -> Result<Self, PluginError> {
        Self::new(WINDOW_CAPTURE_SOURCE_KIND, WindowsTarget::Window)
    }

    fn new(kind: &str, target: WindowsTarget) -> Result<Self, PluginError> {
        Ok(Self {
            kind: Identifier::new(kind).map_err(PluginError::InvalidIdentifier)?,
            target,
        })
    }
}

impl SourceFactory for WindowsCaptureFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        let format = parse_format(settings)?;
        let device_id = selected_device(settings, self.target);
        let device = open_capture_device(name, format, self.target, &device_id)?;
        Ok(Box::new(WindowsCaptureSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            target: self.target,
            device_id,
            format,
            device,
        }))
    }
}

fn selected_device(settings: &Config, target: WindowsTarget) -> String {
    settings
        .get("device_id")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(
            || match target {
                WindowsTarget::Screen => "wgc-screen-picker".to_owned(),
                WindowsTarget::Window => "wgc-window-picker".to_owned(),
            },
            str::to_owned,
        )
}

enum WindowsCaptureBackend {
    Native(ThreadedCaptureDevice),
    Fallback(obs_rs_capture::SimulatedCaptureDevice),
}

fn open_capture_device(
    name: &str,
    format: VideoFormat,
    target: WindowsTarget,
    device_id: &str,
) -> Result<WindowsCaptureBackend, SourceError> {
    let adapter = WindowsCaptureAdapter::default();
    if !adapter.helper().is_file() {
        return fallback_backend(name, format, target).map(WindowsCaptureBackend::Fallback);
    }
    let stable_id = device_id.to_owned();
    Ok(WindowsCaptureBackend::Native(ThreadedCaptureDevice::open(
        CaptureRequest::output(format),
        name,
        move || adapter.open(&stable_id),
    )))
}

struct WindowsCaptureSource {
    kind: Identifier,
    name: String,
    target: WindowsTarget,
    device_id: String,
    format: VideoFormat,
    device: WindowsCaptureBackend,
}

impl Source for WindowsCaptureSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_format(settings)?;
        let device_id = selected_device(settings, self.target);
        if format == self.format && device_id == self.device_id {
            return Ok(());
        }
        self.device = open_capture_device(&self.name, format, self.target, &device_id)?;
        self.device_id = device_id;
        self.format = format;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        let result = match &mut self.device {
            WindowsCaptureBackend::Native(device) => {
                if matches!(
                    device.state(),
                    CaptureLifecycleState::Lost | CaptureLifecycleState::Denied
                ) {
                    Err(device.failure().map_or_else(
                        || "Windows capture helper stopped".to_owned(),
                        |error| error.to_string(),
                    ))
                } else {
                    device
                        .poll_frame(request.timestamp())
                        .map_err(|error| error.to_string())
                }
            }
            WindowsCaptureBackend::Fallback(device) => device
                .next_frame(request.timestamp())
                .map_err(|error| error.to_string()),
        };
        match result {
            Ok(frame) => Ok(frame),
            Err(error) if matches!(&self.device, WindowsCaptureBackend::Native(_)) => {
                self.device = WindowsCaptureBackend::Fallback(fallback_backend(
                    &self.name,
                    self.format,
                    self.target,
                )?);
                match &mut self.device {
                    WindowsCaptureBackend::Fallback(device) => device
                        .next_frame(request.timestamp())
                        .map_err(|fallback| SourceError::Unavailable(fallback.to_string())),
                    WindowsCaptureBackend::Native(_) => Err(SourceError::Unavailable(error)),
                }
            }
            Err(error) => Err(SourceError::Unavailable(error)),
        }
    }
}

fn fallback_backend(
    name: &str,
    format: VideoFormat,
    target: WindowsTarget,
) -> Result<obs_rs_capture::SimulatedCaptureDevice, SourceError> {
    let (kind_name, kind) = match target {
        WindowsTarget::Screen => (SCREEN_CAPTURE_SOURCE_KIND, CaptureKind::Screen),
        WindowsTarget::Window => (WINDOW_CAPTURE_SOURCE_KIND, CaptureKind::Window),
    };
    let mut device = obs_rs_capture::SimulatedCaptureDevice::new(kind_name, name, kind)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    device
        .start(format)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    Ok(device)
}
