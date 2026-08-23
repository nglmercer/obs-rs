#![cfg(target_os = "linux")]

//! The Wayland screen source, backed by the desktop portal.
//!
//! A Wayland compositor never lets a client read the screen directly, so this
//! source asks the portal for a `PipeWire` stream. The first session shows the
//! compositor's own "share your screen" dialog; the token it returns is stored
//! in the source settings, so later sessions reopen the same screen silently.

use crate::portable::parse_format;
use obs_rs_capture::{
    take_wayland_portal_handoff, wayland_session_available, AsyncCaptureDevice,
    CaptureCancellation, CaptureLifecycleState, VideoCaptureDevice, WaylandCaptureDevice,
    WAYLAND_PORTAL_HANDOFF_SETTING, WAYLAND_SCREEN_CAPTURE_SOURCE_KIND,
};
use obs_rs_config::Config;
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

pub(crate) struct WaylandCaptureFactory {
    kind: Identifier,
}

impl WaylandCaptureFactory {
    pub(crate) fn new() -> Result<Self, PluginError> {
        Ok(Self {
            kind: Identifier::new(WAYLAND_SCREEN_CAPTURE_SOURCE_KIND)
                .map_err(PluginError::InvalidIdentifier)?,
        })
    }
}

impl SourceFactory for WaylandCaptureFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        let format = parse_format(settings)?;
        let mut source = WaylandCaptureSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            format,
            restore_token: restore_token(settings),
            settings: without_handoff(settings),
            device: None,
            failure: None,
            retry_countdown: 0,
            pending_settings: None,
        };
        if let Some(handoff_id) = portal_handoff(settings) {
            if let Some(device) = take_wayland_portal_handoff(&handoff_id) {
                source.adopt(device);
            } else {
                source.failure = Some(
                    "the selected screen session expired before the source could adopt it"
                        .to_owned(),
                );
            }
        } else if source.restore_token.is_some() {
            source.reopen();
        } else {
            source.failure = Some(
                "screen sharing has not been approved; choose a display from the screen picker"
                    .to_owned(),
            );
        }
        Ok(Box::new(source))
    }
}

/// Reads the stored portal token, if the project carries one.
fn restore_token(settings: &Config) -> Option<String> {
    settings
        .get("restore_token")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn portal_handoff(settings: &Config) -> Option<String> {
    settings
        .get(WAYLAND_PORTAL_HANDOFF_SETTING)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn without_handoff(settings: &Config) -> Config {
    let mut settings = settings.clone();
    settings.remove(WAYLAND_PORTAL_HANDOFF_SETTING);
    settings
}

struct WaylandCaptureSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    restore_token: Option<String>,
    settings: Config,
    device: Option<AsyncCaptureDevice>,
    /// Why the last attempt to open the portal failed, for the render error.
    failure: Option<String>,
    retry_countdown: u16,
    pending_settings: Option<Config>,
}

impl WaylandCaptureSource {
    /// Opens a portal session and starts its reader.
    ///
    /// Failures are recorded rather than returned: a source whose portal is
    /// momentarily unavailable stays in the scene and retries, which is what
    /// keeps a project loadable on a host with no Wayland session.
    fn reopen(&mut self) {
        self.device = None;
        if !wayland_session_available() {
            self.failure =
                Some("this is not a Wayland session; use the X11 screen source instead".to_owned());
            return;
        }
        let name = self.name.clone();
        let restore_token = self.restore_token.clone();
        self.failure = None;
        self.retry_countdown = 0;
        self.device = Some(AsyncCaptureDevice::open_cancellable(
            self.format,
            move |cancel| {
                WaylandCaptureDevice::open_cancellable(
                    "wayland-screen",
                    &name,
                    restore_token.as_deref(),
                    cancel,
                )
                .map(|device| Box::new(device) as Box<dyn VideoCaptureDevice>)
            },
        ));
    }

    /// Replaces the current source with the session selected by the explicit
    /// portal picker. The session is already live, so this path never performs
    /// a second CreateSession/SelectSources/Start handshake.
    fn adopt(&mut self, device: WaylandCaptureDevice) {
        self.device = None;
        let name = self.name.clone();
        // The session itself is authoritative. In particular, the token
        // returned by Start replaces the one supplied to SelectSources, so a
        // recovery opener must capture the token from this newly adopted
        // session rather than the stale project value.
        let device_token = device.restore_token().map(str::to_owned);
        let restore_token = device_token.clone().or_else(|| self.restore_token.clone());
        match AsyncCaptureDevice::ready(
            self.format,
            Box::new(device),
            move |cancel: &CaptureCancellation| {
                WaylandCaptureDevice::open_cancellable(
                    "wayland-screen",
                    &name,
                    restore_token.as_deref(),
                    cancel,
                )
                .map(|device| Box::new(device) as Box<dyn VideoCaptureDevice>)
            },
        ) {
            Ok(device) => {
                self.device = Some(device);
                self.failure = None;
                self.retry_countdown = 0;
                if let Some(token) = device_token {
                    self.persist_restore_token(&token);
                } else {
                    self.harvest_restore_token();
                }
            }
            Err(error) => {
                self.failure = Some(error.to_string());
                self.retry_countdown = 0;
            }
        }
    }

    /// Copies the replacement token into the source's settings stream. The UI
    /// later persists this update in the project document.
    fn harvest_restore_token(&mut self) {
        let Some(token) = self
            .device
            .as_ref()
            .and_then(AsyncCaptureDevice::restore_token)
            .filter(|token| !token.trim().is_empty())
            .map(str::to_owned)
        else {
            return;
        };
        if self.restore_token.as_deref() == Some(token.as_str()) {
            return;
        }
        self.persist_restore_token(&token);
    }

    fn persist_restore_token(&mut self, token: &str) {
        self.restore_token = Some(token.to_owned());
        if self.settings.set("restore_token", token).is_ok() {
            self.pending_settings = Some(self.settings.clone());
        }
    }
}

impl Source for WaylandCaptureSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_format(settings)?;
        let handoff = portal_handoff(settings);
        let settings = without_handoff(settings);
        let token = restore_token(&settings);
        if let Some(handoff_id) = handoff {
            self.format = format;
            self.settings = settings.clone();
            // The handoff contains a live session and supersedes any stale
            // token that happened to be in the project. `adopt` will persist
            // the token returned by this session.
            self.restore_token = None;
            if let Some(device) = take_wayland_portal_handoff(&handoff_id) {
                self.adopt(device);
            } else {
                self.device = None;
                self.failure = Some(
                    "the selected screen session expired before the source could adopt it"
                        .to_owned(),
                );
                self.retry_countdown = 0;
            }
            return Ok(());
        }
        // Reopening shows the portal dialog again when no token is stored, so
        // it only happens when something the stream depends on actually moved.
        if format == self.format && token == self.restore_token && self.device.is_some() {
            self.settings = settings.clone();
            return Ok(());
        }
        self.format = format;
        self.settings = settings.clone();
        self.restore_token = token;
        if self.restore_token.is_some() {
            self.reopen();
        } else {
            self.device = None;
            self.failure = Some(
                "screen sharing has not been approved; choose a display from the screen picker"
                    .to_owned(),
            );
            self.retry_countdown = 0;
        }
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        if self.device.is_none() {
            return Err(SourceError::Unavailable(
                self.failure
                    .clone()
                    .unwrap_or_else(|| "the screen-cast portal is not open".to_owned()),
            ));
        }
        let result = self
            .device
            .as_mut()
            .ok_or_else(|| {
                SourceError::Unavailable("the screen-cast portal is not open".to_owned())
            })?
            .poll_frame(request.timestamp());
        self.harvest_restore_token();
        match result {
            Ok(frame) => {
                self.failure = None;
                Ok(frame)
            }
            Err(error) => {
                self.failure = Some(error.to_string());
                if self
                    .device
                    .as_ref()
                    .is_some_and(|device| device.state() == CaptureLifecycleState::Lost)
                {
                    if self.retry_countdown == 0 {
                        if let Some(device) = self.device.as_mut() {
                            let _ = device.retry();
                        }
                        self.retry_countdown = 120;
                    } else {
                        self.retry_countdown -= 1;
                    }
                    Ok(None)
                } else {
                    Err(SourceError::Unavailable(error.to_string()))
                }
            }
        }
    }

    fn take_settings_update(&mut self) -> Option<Config> {
        // The portal token may become available after the asynchronous opener
        // completes but before the source renders its first frame. Harvest it
        // at the persistence boundary too, so a low-demand/hidden window does
        // not lose the replacement token.
        self.harvest_restore_token();
        self.pending_settings.take()
    }
}
