//! A bounded reader for a child process that writes raw RGBA frames.
//!
//! Every process-backed capture adapter — the V4L2 camera, the X11 `x11grab`
//! fallback, and the Wayland `PipeWire` reader — has the same shape: spawn a
//! command that writes fixed-size RGBA frames to stdout, keep only the newest
//! complete frame, and surface a read failure as a typed error. Sharing one
//! implementation keeps that contract, and its buffer reuse, in a single place.
//! It is public because the built-in source plugins run the same pattern.

use std::{
    io::Read,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use crate::error::CaptureError;

/// The newest complete frame the reader has published.
///
/// The reader publishes immutable shared storage and reclaims the previous
/// allocation whenever no consumer still owns it. The frame stays in the slot,
/// so a consumer polling faster than the camera sees the current image.
#[derive(Default)]
struct FrameSlot {
    latest: Option<Arc<Vec<u8>>>,
}

/// A child process writing raw RGBA frames, with a newest-frame slot.
pub struct RawFrameReader {
    child: Option<Child>,
    running: Arc<AtomicBool>,
    slot: Arc<Mutex<FrameSlot>>,
    read_error: Arc<Mutex<Option<String>>>,
    reader: Option<JoinHandle<()>>,
}

impl RawFrameReader {
    /// Spawns `command` and starts reading `frame_bytes`-sized frames from it.
    ///
    /// `what` names the capture in error messages.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Io`] when the process or its reader thread
    /// cannot be started.
    pub fn spawn(
        mut command: Command,
        frame_bytes: usize,
        what: &str,
    ) -> Result<Self, CaptureError> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|error| CaptureError::Io {
            message: format!("start {what}: {error}"),
        })?;
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CaptureError::Io {
                message: format!("{what} did not expose stdout"),
            });
        };

        let running = Arc::new(AtomicBool::new(true));
        let slot = Arc::new(Mutex::new(FrameSlot::default()));
        let read_error = Arc::new(Mutex::new(None));
        let reader_running = Arc::clone(&running);
        let reader_slot = Arc::clone(&slot);
        let reader_error = Arc::clone(&read_error);
        let reader = thread::Builder::new()
            .name("obs-rs-raw-frames".to_owned())
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
                    let Ok(mut slot) = reader_slot.lock() else {
                        break;
                    };
                    // Publish shared immutable storage. Reclaim the previous
                    // allocation whenever every consumer has released it.
                    let previous = slot.latest.replace(Arc::new(pixels));
                    pixels = previous
                        .and_then(|buffer| Arc::try_unwrap(buffer).ok())
                        .filter(|buffer| buffer.len() == frame_bytes)
                        .unwrap_or_else(|| vec![0_u8; frame_bytes]);
                }
            })
            .map_err(|error| CaptureError::Io {
                message: format!("start the {what} frame reader: {error}"),
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
            child: Some(child),
            running,
            slot,
            read_error,
            reader: Some(reader),
        })
    }

    /// Returns a copy of the newest frame, or `None` before the first arrives.
    ///
    /// The frame stays available, so a consumer polling faster than the capture
    /// keeps seeing the current image instead of gaps.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Io`] once the process or the pipe has failed.
    pub fn latest_frame(&self, what: &str) -> Result<Option<Vec<u8>>, CaptureError> {
        Ok(self
            .latest_shared_frame(what)?
            .map(|pixels| pixels.as_ref().clone()))
    }

    /// Returns shared ownership of the newest complete frame.
    ///
    /// This is the live capture path: acquiring a frame increments a reference
    /// count instead of copying the complete RGBA buffer.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Io`] once the process or pipe has failed.
    pub fn latest_shared_frame(&self, what: &str) -> Result<Option<Arc<Vec<u8>>>, CaptureError> {
        if let Ok(error) = self.read_error.lock() {
            if let Some(error) = error.as_ref() {
                return Err(CaptureError::Io {
                    message: format!("read a frame from {what}: {error}"),
                });
            }
        }
        Ok(self.slot.lock().ok().and_then(|slot| slot.latest.clone()))
    }
}

impl Drop for RawFrameReader {
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
