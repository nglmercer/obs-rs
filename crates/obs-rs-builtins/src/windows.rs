#![cfg(target_os = "windows")]

//! Windows Graphics Capture source factories.

use crate::portable::parse_format;
use obs_rs_capture::{
    CaptureLifecycleState, CaptureRequest, CaptureRetrySchedule, PlatformCaptureAdapter,
    ThreadedCaptureDevice, SCREEN_CAPTURE_SOURCE_KIND, WINDOW_CAPTURE_SOURCE_KIND,
};
use obs_rs_capture_windows::WindowsCaptureAdapter;
use obs_rs_config::Config;
use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsTarget {
    Screen,
    Window,
}

const WINDOWS_CAPTURE_RETRY_INTERVAL_NANOS: u64 = 500_000_000;

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
        let capture_border = capture_border_setting(settings);
        let device = open_capture_device(name, format, &device_id, capture_cursor, capture_border);
        Ok(Box::new(WindowsCaptureSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            target: self.target,
            device_id,
            format,
            capture_cursor,
            capture_border,
            device,
            failure: None,
            retry_schedule: CaptureRetrySchedule::new(WINDOWS_CAPTURE_RETRY_INTERVAL_NANOS),
            shutdown_blocked: false,
        }))
    }
}

fn selected_device(settings: &Config, target: WindowsTarget) -> String {
    // The Windows screen-properties dialog exposes `monitor` because it is the
    // stable display selector used by the monitor picker. Older Windows
    // settings only carried `device_id`, so retain that as a compatibility
    // fallback while making an explicit monitor selection authoritative.
    if target == WindowsTarget::Screen {
        if let Some(monitor) = settings
            .get("monitor")
            .map(str::trim)
            .filter(|value| value.starts_with("wgc-screen-"))
        {
            return monitor.to_owned();
        }
    }
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

fn capture_border_setting(settings: &Config) -> bool {
    settings
        .get("capture_border")
        .and_then(|value| value.trim().parse::<bool>().ok())
        .unwrap_or(false)
}

fn open_capture_device(
    name: &str,
    format: VideoFormat,
    device_id: &str,
    capture_cursor: bool,
    capture_border: bool,
) -> ThreadedCaptureDevice {
    let adapter = WindowsCaptureAdapter::default()
        .with_capture_cursor(capture_cursor)
        .with_capture_border(capture_border);
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
    capture_border: bool,
    device: ThreadedCaptureDevice,
    failure: Option<String>,
    retry_schedule: CaptureRetrySchedule,
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
        let capture_border = capture_border_setting(settings);
        if format == self.format
            && device_id == self.device_id
            && capture_cursor == self.capture_cursor
            && capture_border == self.capture_border
        {
            return Ok(());
        }
        if !self.device.shutdown() {
            self.shutdown_blocked = true;
            return Err(SourceError::Unavailable(
                "the previous Windows capture helper is still shutting down".to_owned(),
            ));
        }
        self.device = open_capture_device(
            &self.name,
            format,
            &device_id,
            capture_cursor,
            capture_border,
        );
        self.device_id = device_id;
        self.format = format;
        self.capture_cursor = capture_cursor;
        self.capture_border = capture_border;
        self.failure = None;
        self.retry_schedule.reset();
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
                let timestamp = request.timestamp();
                if self.retry_schedule.due(timestamp) {
                    self.reopen(timestamp);
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
    fn reopen(&mut self, timestamp: Timestamp) {
        // `shutdown` is retryable after a wedged helper eventually exits. The
        // old implementation permanently returned once the first bounded
        // shutdown timed out, which made a recoverable helper loss require a
        // source-settings edit before capture could return.
        if !self.device.shutdown() {
            self.shutdown_blocked = true;
            self.failure = Some(
                "the previous Windows capture helper is still shutting down; retry postponed"
                    .to_owned(),
            );
            self.retry_schedule.mark_attempt(timestamp);
            return;
        }
        let adapter = WindowsCaptureAdapter::default()
            .with_capture_cursor(self.capture_cursor)
            .with_capture_border(self.capture_border);
        let stable_id = self.device_id.clone();
        self.device = ThreadedCaptureDevice::open(
            CaptureRequest::output(self.format),
            &self.name,
            move || adapter.open(&stable_id),
        );
        self.failure = Some("reconnecting Windows capture helper".to_owned());
        self.retry_schedule.mark_attempt(timestamp);
        self.shutdown_blocked = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_screen_monitor_overrides_the_legacy_device_setting() {
        let mut settings = Config::new();
        settings
            .set("device_id", "wgc-screen-picker")
            .expect("device ID");
        settings
            .set("monitor", "wgc-screen-secondary")
            .expect("monitor");

        assert_eq!(
            selected_device(&settings, WindowsTarget::Screen),
            "wgc-screen-secondary"
        );
    }

    #[test]
    fn window_sources_keep_using_their_device_setting() {
        let mut settings = Config::new();
        settings
            .set("device_id", "wgc-window-editor")
            .expect("device ID");
        settings
            .set("monitor", "wgc-screen-secondary")
            .expect("monitor");

        assert_eq!(
            selected_device(&settings, WindowsTarget::Window),
            "wgc-window-editor"
        );
    }
}
