#![cfg(target_os = "linux")]

use std::{
    io::Read,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use crate::portable::parse_format;
use obs_rs_capture::{
    x11_monitors, CaptureError, CaptureKind, SimulatedCaptureDevice, VideoCaptureDevice,
    X11CaptureDevice, X11_SCREEN_CAPTURE_SOURCE_KIND,
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
        let monitor = monitor_setting(settings);
        let device = match x11_device(name, &display, format, monitor.as_deref()) {
            Ok(device) => X11CaptureBackend::Native(device),
            Err(_) => fallback_backend(name, &display, format, monitor.as_deref(), None)?,
        };
        Ok(Box::new(X11CaptureSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            display,
            monitor,
            format,
            device,
            fallback_frames: 0,
        }))
    }
}

/// Reads the selected `RandR` monitor name, if any.
///
/// An absent or empty value keeps the historical behaviour of capturing the
/// whole root window, so projects written before monitor selection existed load
/// unchanged.
fn monitor_setting(settings: &Config) -> Option<String> {
    settings
        .get("monitor")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(target_os = "linux")]
struct X11CaptureSource {
    kind: Identifier,
    name: String,
    display: String,
    /// The selected `RandR` monitor; `None` captures the whole desktop.
    monitor: Option<String>,
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
        let monitor = monitor_setting(settings);
        let device = match x11_device(&self.name, &display, format, monitor.as_deref()) {
            Ok(device) => X11CaptureBackend::Native(device),
            Err(_) => fallback_backend(&self.name, &display, format, monitor.as_deref(), None)?,
        };
        self.display = display;
        self.monitor = monitor;
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
                    self.monitor.as_deref(),
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
                        self.monitor.as_deref(),
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
/// The reader keeps only the newest complete frame, so rendering never waits
/// for the display server or the camera-like `x11grab` cadence.
struct X11GrabDevice {
    format: VideoFormat,
    child: Option<Child>,
    running: Arc<AtomicBool>,
    latest_frame: Arc<Mutex<Option<Vec<u8>>>>,
    read_error: Arc<Mutex<Option<String>>>,
    reader: Option<JoinHandle<()>>,
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
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|error| CaptureError::Io {
            message: format!("start ffmpeg x11grab for {display}: {error}"),
        })?;
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CaptureError::Io {
                message: "ffmpeg x11grab did not expose stdout".to_owned(),
            });
        };
        let running = Arc::new(AtomicBool::new(true));
        let latest_frame = Arc::new(Mutex::new(None));
        let read_error = Arc::new(Mutex::new(None));
        let reader_running = Arc::clone(&running);
        let reader_frames = Arc::clone(&latest_frame);
        let reader_error = Arc::clone(&read_error);
        let frame_bytes = format.rgba_bytes();
        let reader = thread::Builder::new()
            .name("obs-rs-x11grab".to_owned())
            .spawn(move || {
                let mut pixels = vec![0_u8; frame_bytes];
                while reader_running.load(Ordering::Acquire) {
                    if let Err(error) = stdout.read_exact(&mut pixels) {
                        if reader_running.load(Ordering::Acquire) {
                            if let Ok(mut target) = reader_error.lock() {
                                *target = Some(error.to_string());
                            }
                        }
                        break;
                    }
                    if let Ok(mut target) = reader_frames.lock() {
                        *target = Some(pixels.clone());
                    } else {
                        break;
                    }
                }
            })
            .map_err(|error| CaptureError::Io {
                message: format!("start x11grab frame reader: {error}"),
            });
        let reader = match reader {
            Ok(reader) => reader,
            Err(error) => {
                running.store(false, Ordering::Release);
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self {
            format,
            child: Some(child),
            running,
            latest_frame,
            read_error,
            reader: Some(reader),
        })
    }

    fn next_frame(
        &mut self,
        timestamp: obs_rs_media::Timestamp,
    ) -> Result<Option<VideoFrame>, CaptureError> {
        if let Ok(error) = self.read_error.lock() {
            if let Some(error) = error.as_ref() {
                return Err(CaptureError::Io {
                    message: format!("read x11grab frame: {error}"),
                });
            }
        }
        let pixels = self
            .latest_frame
            .lock()
            .ok()
            .and_then(|frame| frame.as_ref().cloned());
        let Some(pixels) = pixels else {
            return Ok(None);
        };
        VideoFrame::new(self.format, timestamp, pixels)
            .map(Some)
            .map_err(CaptureError::Media)
    }
}

impl Drop for X11GrabDevice {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
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

fn fallback_backend(
    name: &str,
    display: &str,
    format: VideoFormat,
    monitor: Option<&str>,
    size: Option<(u32, u32)>,
) -> Result<X11CaptureBackend, SourceError> {
    let region = monitor_region(display, monitor)
        .or_else(|| size.map(|(width, height)| (0, 0, width, height)));
    match X11GrabDevice::new(display, format, region) {
        Ok(device) => Ok(X11CaptureBackend::Ffmpeg(device)),
        Err(_) => fallback_device(name, format).map(X11CaptureBackend::Fallback),
    }
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
    monitor: Option<&str>,
) -> Result<X11CaptureDevice, SourceError> {
    let mut device = X11CaptureDevice::connect(display, "x11-root", name)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    device
        .select_monitor(monitor)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    device
        .start(format)
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
    Ok(device)
}
