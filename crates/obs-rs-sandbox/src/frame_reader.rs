use std::{
    io::BufReader,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{sync_channel, Receiver, SyncSender},
    thread::{self, JoinHandle},
};

use obs_rs_capture::{
    CaptureDeviceInfo, CaptureError, CaptureKind, CapturePermission, StreamCaptureDevice,
    VideoCaptureDevice,
};
use obs_rs_config::Config;
use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};
use obs_rs_util::Identifier;

use super::{
    error::SandboxError,
    protocol::{
        FrameReader, FrameReceiver, FrameResult, MAX_SANDBOX_QUEUED_FRAMES,
        SANDBOX_FRAME_DELIVERY_TIMEOUT,
    },
};

pub(crate) struct ProcessFrameDevice {
    info: CaptureDeviceInfo,
    kind: Identifier,
    command: PathBuf,
    arguments: Vec<String>,
    settings: Config,
    child: Option<Child>,
    frames: Option<Receiver<Result<Option<VideoFrame>, CaptureError>>>,
    reader_thread: Option<JoinHandle<()>>,
    format: Option<VideoFormat>,
}

impl ProcessFrameDevice {
    pub(crate) fn new(
        name: &str,
        kind: Identifier,
        command: PathBuf,
        arguments: Vec<String>,
        settings: Config,
    ) -> Result<Self, SandboxError> {
        let info = CaptureDeviceInfo::new("sandbox_process", name, CaptureKind::External)?;
        Ok(Self {
            info,
            kind,
            command,
            arguments,
            settings,
            child: None,
            frames: None,
            reader_thread: None,
            format: None,
        })
    }

    fn spawn_stream(
        &self,
        format: VideoFormat,
    ) -> Result<(Child, FrameReceiver, FrameReader), SandboxError> {
        let mut command = Command::new(&self.command);
        command
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("OBS_RS_PROTOCOL", "OBSFRM01")
            .env("OBS_RS_SOURCE_KIND", self.kind.as_str())
            .env("OBS_RS_SOURCE_NAME", self.info.name())
            .env("OBS_RS_SETTINGS", self.settings.serialize())
            .env("OBS_RS_WIDTH", format.width().to_string())
            .env("OBS_RS_HEIGHT", format.height().to_string())
            .env(
                "OBS_RS_FPS_NUMERATOR",
                format.frame_rate().numerator().to_string(),
            )
            .env(
                "OBS_RS_FPS_DENOMINATOR",
                format.frame_rate().denominator().to_string(),
            );
        let mut child = command
            .spawn()
            .map_err(|error| SandboxError::InvalidCommand {
                reason: error.to_string(),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::InvalidCommand {
                reason: "sandbox child did not expose stdout".to_owned(),
            })?;
        let mut stream = StreamCaptureDevice::new(
            "sandbox_process",
            self.info.name(),
            CaptureKind::External,
            BufReader::new(stdout),
        )?;
        if let Err(error) = stream.start(format) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        let (sender, receiver) = sync_channel(MAX_SANDBOX_QUEUED_FRAMES);
        let reader_thread = spawn_frame_reader(stream, sender);
        Ok((child, receiver, reader_thread))
    }
}

impl VideoCaptureDevice for ProcessFrameDevice {
    fn info(&self) -> &CaptureDeviceInfo {
        &self.info
    }

    fn start(&mut self, format: VideoFormat) -> Result<(), CaptureError> {
        if self.format.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        match self.info.permission() {
            CapturePermission::Granted => {}
            CapturePermission::PromptRequired => return Err(CaptureError::PermissionRequired),
            CapturePermission::Denied => return Err(CaptureError::PermissionDenied),
            CapturePermission::Unavailable => return Err(CaptureError::PermissionUnavailable),
        }
        let (child, frames, reader_thread) =
            self.spawn_stream(format).map_err(|error| match error {
                SandboxError::Capture(error) => error,
                SandboxError::Media(error) => CaptureError::Media(error),
                other => CaptureError::PlatformUnavailable {
                    message: other.to_string(),
                },
            })?;
        self.child = Some(child);
        self.frames = Some(frames);
        self.reader_thread = Some(reader_thread);
        self.format = Some(format);
        Ok(())
    }

    fn stop(&mut self) {
        self.frames = None;
        self.format = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
    }

    fn is_running(&self) -> bool {
        self.format.is_some()
    }

    fn next_frame(&mut self, _timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let Some(format) = self.format else {
            return Err(CaptureError::NotRunning);
        };
        let receiver = self.frames.as_ref().ok_or(CaptureError::NotRunning)?;
        let result = receiver.recv_timeout(SANDBOX_FRAME_DELIVERY_TIMEOUT);
        let frame = match result {
            Ok(frame) => frame?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.stop();
                return Err(CaptureError::Io {
                    message: format!(
                        "sandbox process did not deliver a frame within {SANDBOX_FRAME_DELIVERY_TIMEOUT:?}"
                    ),
                });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CaptureError::Io {
                    message: "sandbox frame reader disconnected".to_owned(),
                });
            }
        };
        if frame.is_none() {
            if let Some(child) = self.child.as_mut() {
                if let Some(status) = child.try_wait().map_err(|error| CaptureError::Io {
                    message: error.to_string(),
                })? {
                    return Err(CaptureError::Io {
                        message: format!("sandbox process exited with {status}"),
                    });
                }
            }
        }
        if let Some(frame) = &frame {
            if frame.format() != format {
                return Err(CaptureError::FrameFormatMismatch {
                    expected: format,
                    actual: frame.format(),
                });
            }
        }
        Ok(frame)
    }
}

fn spawn_frame_reader(
    mut stream: StreamCaptureDevice<BufReader<std::process::ChildStdout>>,
    sender: SyncSender<FrameResult>,
) -> JoinHandle<()> {
    thread::spawn(move || loop {
        match stream.next_frame(Timestamp::ZERO) {
            Ok(frame) => {
                let end_of_stream = frame.is_none();
                if sender.send(Ok(frame)).is_err() || end_of_stream {
                    break;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error));
                break;
            }
        }
    })
}

impl Drop for ProcessFrameDevice {
    fn drop(&mut self) {
        self.stop();
    }
}
