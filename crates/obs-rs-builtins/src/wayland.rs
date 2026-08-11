#![cfg(target_os = "linux")]

//! The Wayland screen source, backed by the desktop portal.
//!
//! A Wayland compositor never lets a client read the screen directly, so this
//! source asks the portal for a `PipeWire` stream. The first session shows the
//! compositor's own "share your screen" dialog; the token it returns is stored
//! in the source settings, so later sessions reopen the same screen silently.

use crate::portable::parse_format;
use obs_rs_capture::{
    wayland_session_available, VideoCaptureDevice, WaylandCaptureDevice,
    WAYLAND_SCREEN_CAPTURE_SOURCE_KIND,
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
            device: None,
            failure: None,
        };
        source.reopen();
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

struct WaylandCaptureSource {
    kind: Identifier,
    name: String,
    format: VideoFormat,
    restore_token: Option<String>,
    device: Option<WaylandCaptureDevice>,
    /// Why the last attempt to open the portal failed, for the render error.
    failure: Option<String>,
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
        // Without a token the portal would raise its dialog from whichever
        // thread is rendering and block it until someone answers, so the source
        // waits for the UI to run the picker and store one instead.
        if self.restore_token.is_none() {
            self.failure =
                Some("no screen has been shared yet; choose one with Select display".to_owned());
            return;
        }
        match WaylandCaptureDevice::open(
            "wayland-screen",
            &self.name,
            self.restore_token.as_deref(),
        ) {
            Ok(mut device) => match device.start(self.format) {
                Ok(()) => {
                    self.failure = None;
                    self.device = Some(device);
                }
                Err(error) => self.failure = Some(error.to_string()),
            },
            Err(error) => self.failure = Some(error.to_string()),
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
        let token = restore_token(settings);
        // Reopening shows the portal dialog again when no token is stored, so
        // it only happens when something the stream depends on actually moved.
        if format == self.format && token == self.restore_token && self.device.is_some() {
            return Ok(());
        }
        self.format = format;
        self.restore_token = token;
        self.reopen();
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        let Some(device) = self.device.as_mut() else {
            return Err(SourceError::Unavailable(
                self.failure
                    .clone()
                    .unwrap_or_else(|| "the screen-cast portal is not open".to_owned()),
            ));
        };
        device
            .next_frame(request.timestamp())
            .map_err(|error| SourceError::Unavailable(error.to_string()))
    }
}
