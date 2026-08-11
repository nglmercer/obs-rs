#![cfg(target_os = "linux")]

use std::process::Command;

use crate::portable::parse_format;
use obs_rs_capture::{
    x11_monitors, x11_windows, CaptureError, CaptureKind, RawFrameReader, SimulatedCaptureDevice,
    VideoCaptureDevice, X11CaptureDevice, X11_SCREEN_CAPTURE_SOURCE_KIND,
    X11_WINDOW_CAPTURE_SOURCE_KIND,
};
use obs_rs_config::Config;
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_plugin_api::{PluginError, Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

/// What an X11 source is pointed at.
///
/// Both targets share one connection, one decoder, and one fallback chain; they
/// differ only in which rectangle of the root window is read, so they are one
/// source implementation parameterized by target rather than two.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum X11Target {
    /// The whole desktop, or one `RandR` monitor of it.
    Screen,
    /// One tracked window, followed as it moves and resizes.
    Window,
}

pub(crate) struct X11CaptureFactory {
    kind: Identifier,
    target: X11Target,
}

#[cfg(target_os = "linux")]
impl X11CaptureFactory {
    pub(crate) fn new() -> Result<Self, PluginError> {
        Self::for_target(X11_SCREEN_CAPTURE_SOURCE_KIND, X11Target::Screen)
    }

    pub(crate) fn for_windows() -> Result<Self, PluginError> {
        Self::for_target(X11_WINDOW_CAPTURE_SOURCE_KIND, X11Target::Window)
    }

    fn for_target(kind: &str, target: X11Target) -> Result<Self, PluginError> {
        Ok(Self {
            kind: Identifier::new(kind).map_err(PluginError::InvalidIdentifier)?,
            target,
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
        let selection = selection_setting(settings, self.target);
        let device = match x11_device(name, &display, format, self.target, selection.as_deref()) {
            Ok(device) => X11CaptureBackend::Native(device),
            Err(_) => fallback_backend(
                name,
                &display,
                format,
                self.target,
                selection.as_deref(),
                None,
            )?,
        };
        Ok(Box::new(X11CaptureSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            display,
            target: self.target,
            selection,
            format,
            device,
            fallback_frames: 0,
        }))
    }
}

/// Reads the target this source is pointed at, if one was chosen.
///
/// An absent or empty value keeps the historical behaviour of capturing the
/// whole root window, so projects written before selection existed load
/// unchanged. A window source with no selection is equally a full-desktop
/// capture rather than an error, because a source has to render something while
/// the user is still deciding which window they want.
fn selection_setting(settings: &Config, target: X11Target) -> Option<String> {
    let key = match target {
        X11Target::Screen => "monitor",
        X11Target::Window => "window",
    };
    settings
        .get(key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(target_os = "linux")]
struct X11CaptureSource {
    kind: Identifier,
    name: String,
    display: String,
    target: X11Target,
    /// The selected monitor or window; `None` captures the whole desktop.
    selection: Option<String>,
    format: VideoFormat,
    device: X11CaptureBackend,
    fallback_frames: u64,
}

enum X11CaptureBackend {
    Native(X11CaptureDevice),
    Ffmpeg(X11GrabDevice),
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
        let selection = selection_setting(settings, self.target);
        let device = match x11_device(
            &self.name,
            &display,
            format,
            self.target,
            selection.as_deref(),
        ) {
            Ok(device) => X11CaptureBackend::Native(device),
            Err(_) => fallback_backend(
                &self.name,
                &display,
                format,
                self.target,
                selection.as_deref(),
                None,
            )?,
        };
        self.display = display;
        self.selection = selection;
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
                if let Ok(device) = x11_device(
                    &self.name,
                    &self.display,
                    self.format,
                    self.target,
                    self.selection.as_deref(),
                ) {
                    self.device = X11CaptureBackend::Native(device);
                }
            }
        }

        for attempt in 0..2 {
            let result = match &mut self.device {
                X11CaptureBackend::Native(device) => device.next_frame(request.timestamp()),
                X11CaptureBackend::Ffmpeg(device) => device.next_frame(request.timestamp()),
                X11CaptureBackend::Fallback(device) => device.next_frame(request.timestamp()),
            };
            match result {
                Ok(frame) => return Ok(frame),
                Err(_error) if attempt == 0 => {
                    let size = match &self.device {
                        X11CaptureBackend::Native(device) => Some(device.capture_size()),
                        X11CaptureBackend::Ffmpeg(_) | X11CaptureBackend::Fallback(_) => None,
                    };
                    self.device = fallback_backend(
                        &self.name,
                        &self.display,
                        self.format,
                        self.target,
                        self.selection.as_deref(),
                        size,
                    )?;
                }
                Err(error) => return Err(SourceError::Unavailable(error.to_string())),
            }
        }
        Err(SourceError::Unavailable(
            "X11 capture did not produce a frame".to_owned(),
        ))
    }
}

/// Process-backed X11 capture used when a compositor rejects direct `GetImage`.
///
/// The reader keeps only the newest complete frame, so rendering never waits
/// for the display server or the camera-like `x11grab` cadence.
struct X11GrabDevice {
    format: VideoFormat,
    reader: RawFrameReader,
}

impl X11GrabDevice {
    fn new(
        display: &str,
        format: VideoFormat,
        region: Option<(u32, u32, u32, u32)>,
    ) -> Result<Self, CaptureError> {
        let frame_rate = format.frame_rate();
        let mut command = Command::new("ffmpeg");
        command.args(["-hide_banner", "-loglevel", "error", "-f", "x11grab"]);
        if let Some((_, _, width, height)) = region {
            command.args(["-video_size", &format!("{width}x{height}")]);
        }
        // x11grab addresses a monitor as `:0.0+x,y`, which is how the selected
        // monitor survives the drop to the process-backed backend.
        let input = match region {
            Some((x, y, _, _)) if x > 0 || y > 0 => format!("{display}+{x},{y}"),
            _ => display.to_owned(),
        };
        command.args([
            "-framerate",
            &format!("{}/{}", frame_rate.numerator(), frame_rate.denominator()),
            "-i",
            &input,
            "-vf",
            &format!("scale={}:{}", format.width(), format.height()),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-",
        ]);
        let reader = RawFrameReader::spawn(
            command,
            format.rgba_bytes(),
            &format!("ffmpeg x11grab for {display}"),
        )?;
        Ok(Self { format, reader })
    }

    fn next_frame(
        &mut self,
        timestamp: obs_rs_media::Timestamp,
    ) -> Result<Option<VideoFrame>, CaptureError> {
        let Some(pixels) = self.reader.latest_frame("x11grab")? else {
            return Ok(None);
        };
        VideoFrame::new(self.format, timestamp, pixels)
            .map(Some)
            .map_err(CaptureError::Media)
    }
}

fn fallback_device(
    name: &str,
    format: VideoFormat,
    target: X11Target,
) -> Result<SimulatedCaptureDevice, SourceError> {
    let (kind_name, kind) = match target {
        X11Target::Screen => (X11_SCREEN_CAPTURE_SOURCE_KIND, CaptureKind::Screen),
        X11Target::Window => (X11_WINDOW_CAPTURE_SOURCE_KIND, CaptureKind::Window),
    };
    let mut device = SimulatedCaptureDevice::new(kind_name, name, kind)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    device
        .start(format)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    Ok(device)
}

fn fallback_backend(
    name: &str,
    display: &str,
    format: VideoFormat,
    target: X11Target,
    selection: Option<&str>,
    size: Option<(u32, u32)>,
) -> Result<X11CaptureBackend, SourceError> {
    let region = selection_region(display, target, selection)
        .or_else(|| size.map(|(width, height)| (0, 0, width, height)));
    match X11GrabDevice::new(display, format, region) {
        Ok(device) => Ok(X11CaptureBackend::Ffmpeg(device)),
        Err(_) => fallback_device(name, format, target).map(X11CaptureBackend::Fallback),
    }
}

/// Resolves the selected target's rectangle for the process-backed backend.
///
/// `x11grab` can only follow a fixed rectangle, so a window that moves after
/// the drop to this backend keeps capturing the rectangle it was in. That is a
/// real limitation of the fallback and is why the direct adapter is preferred
/// whenever the server permits `GetImage`.
fn selection_region(
    display: &str,
    target: X11Target,
    selection: Option<&str>,
) -> Option<(u32, u32, u32, u32)> {
    match target {
        X11Target::Screen => monitor_region(display, selection),
        X11Target::Window => window_region(display, selection),
    }
}

fn window_region(display: &str, window: Option<&str>) -> Option<(u32, u32, u32, u32)> {
    let wanted = window.map(str::trim).filter(|value| !value.is_empty())?;
    let id = obs_rs_capture::parse_window_id(wanted)?;
    let windows = x11_windows(display).ok()?;
    let selected = windows.iter().find(|candidate| candidate.id() == id)?;
    // `x11_windows` reports sizes, not root origins, so the fallback captures a
    // rectangle of that size at the desktop origin rather than guessing.
    Some((0, 0, selected.width(), selected.height()))
}

/// Resolves the selected monitor's rectangle for the process-backed backend.
fn monitor_region(display: &str, monitor: Option<&str>) -> Option<(u32, u32, u32, u32)> {
    let wanted = monitor.map(str::trim).filter(|name| !name.is_empty())?;
    let monitors = x11_monitors(display).ok()?;
    let selected = monitors
        .iter()
        .find(|candidate| candidate.name() == wanted || candidate.device_id() == wanted)?;
    Some((
        u32::try_from(selected.x().max(0)).unwrap_or(0),
        u32::try_from(selected.y().max(0)).unwrap_or(0),
        selected.width(),
        selected.height(),
    ))
}

#[cfg(target_os = "linux")]
fn x11_device(
    name: &str,
    display: &str,
    format: VideoFormat,
    target: X11Target,
    selection: Option<&str>,
) -> Result<X11CaptureDevice, SourceError> {
    let mut device = X11CaptureDevice::connect(display, "x11-root", name)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    match target {
        X11Target::Screen => device.select_monitor(selection),
        X11Target::Window => device.select_window(selection),
    }
    .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    device
        .start(format)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    Ok(device)
}
