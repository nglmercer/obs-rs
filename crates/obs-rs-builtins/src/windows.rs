#![cfg(target_os = "windows")]

//! Windows Graphics Capture source factories.

use crate::portable::parse_format;
use obs_rs_capture::{
    CaptureLifecycleState, CaptureRequest, PlatformCaptureAdapter, ThreadedCaptureDevice,
    SCREEN_CAPTURE_SOURCE_KIND, WINDOW_CAPTURE_SOURCE_KIND,
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

const WINDOWS_CAPTURE_RETRY_FRAMES: u32 = 30;

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
        let capture_cursor = capture_cursor_setting(settings);
        let device = open_capture_device(name, format, &device_id, capture_cursor);
        Ok(Box::new(WindowsCaptureSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            target: self.target,
            device_id,
            format,
            capture_cursor,
            device,
            failure: None,
            retry_countdown: WINDOWS_CAPTURE_RETRY_FRAMES,
            shutdown_blocked: false,
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

fn capture_cursor_setting(settings: &Config) -> bool {
    settings
        .get("capture_cursor")
        .and_then(|value| value.trim().parse::<bool>().ok())
        .unwrap_or(true)
}

fn open_capture_device(
    name: &str,
    format: VideoFormat,
    device_id: &str,
    capture_cursor: bool,
) -> ThreadedCaptureDevice {
    let adapter = WindowsCaptureAdapter::default().with_capture_cursor(capture_cursor);
    let stable_id = device_id.to_owned();
    ThreadedCaptureDevice::open(CaptureRequest::output(format), name, move || {
        adapter.open(&stable_id)
    })
}

struct WindowsCaptureSource {
    kind: Identifier,
    name: String,
    target: WindowsTarget,
    device_id: String,
    format: VideoFormat,
    capture_cursor: bool,
    device: ThreadedCaptureDevice,
    failure: Option<String>,
    retry_countdown: u32,
    shutdown_blocked: bool,
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
        let capture_cursor = capture_cursor_setting(settings);
        if format == self.format
            && device_id == self.device_id
            && capture_cursor == self.capture_cursor
        {
            return Ok(());
        }
        if !self.device.shutdown() {
            self.shutdown_blocked = true;
            return Err(SourceError::Unavailable(
                "the previous Windows capture helper is still shutting down".to_owned(),
            ));
        }
        self.device = open_capture_device(&self.name, format, &device_id, capture_cursor);
        self.device_id = device_id;
        self.format = format;
        self.capture_cursor = capture_cursor;
        self.failure = None;
        self.retry_countdown = WINDOWS_CAPTURE_RETRY_FRAMES;
        self.shutdown_blocked = false;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        if matches!(
            self.device.state(),
            CaptureLifecycleState::Lost | CaptureLifecycleState::Denied
        ) {
            let failure = self.device.failure().map_or_else(
                || "Windows capture helper stopped".to_owned(),
                |error| error.to_string(),
            );
            self.failure = Some(failure.clone());
            if self.device.state() == CaptureLifecycleState::Lost {
                self.retry_countdown = self.retry_countdown.saturating_sub(1);
                if self.retry_countdown == 0 {
                    self.reopen();
                }
            }
            return Err(SourceError::Unavailable(
                self.failure.clone().unwrap_or(failure),
            ));
        }
        self.device
            .poll_frame(request.timestamp())
            .map_err(|error| {
                self.failure = Some(error.to_string());
                SourceError::Unavailable(error.to_string())
            })
    }
}

impl WindowsCaptureSource {
    fn reopen(&mut self) {
        if self.shutdown_blocked {
            return;
        }
        if !self.device.shutdown() {
            self.shutdown_blocked = true;
            self.failure = Some(
                "the previous Windows capture helper is still shutting down; retry postponed"
                    .to_owned(),
            );
            return;
        }
        let adapter = WindowsCaptureAdapter::default().with_capture_cursor(self.capture_cursor);
        let stable_id = self.device_id.clone();
        self.device = ThreadedCaptureDevice::open(
            CaptureRequest::output(self.format),
            &self.name,
            move || adapter.open(&stable_id),
        );
        self.failure = Some("reconnecting Windows capture helper".to_owned());
        self.retry_countdown = WINDOWS_CAPTURE_RETRY_FRAMES;
    }
}
