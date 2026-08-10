use std::{sync::mpsc::Receiver, thread::JoinHandle, time::Duration};

use obs_rs_capture::CaptureError;
use obs_rs_media::VideoFrame;

/// Header for the bounded line-oriented sandbox manifest.
pub const SANDBOX_MANIFEST_MAGIC: &str = "OBSRPLUGIN1";
/// Maximum UTF-8 size accepted for one sandbox manifest.
pub const MAX_SANDBOX_MANIFEST_BYTES: usize = 32 * 1024;
/// Maximum source kinds declared by one sandbox plugin.
pub const MAX_SANDBOX_SOURCE_KINDS: usize = 64;
/// Maximum command arguments accepted by one sandbox plugin.
pub const MAX_SANDBOX_ARGUMENTS: usize = 64;
/// Maximum UTF-8 size accepted for one command argument.
pub const MAX_SANDBOX_ARGUMENT_BYTES: usize = 4 * 1024;
/// Maximum number of decoded frames allowed to wait between the sandbox
/// process and the render thread.
pub const MAX_SANDBOX_QUEUED_FRAMES: usize = 2;
/// Maximum time a render request waits for a sandbox frame.
pub const SANDBOX_FRAME_DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);
/// Argument used by a subprocess manifest probe.
pub const SANDBOX_MANIFEST_ARGUMENT: &str = "--obs-rs-manifest";

pub(crate) type FrameResult = Result<Option<VideoFrame>, CaptureError>;
pub(crate) type FrameReceiver = Receiver<FrameResult>;
pub(crate) type FrameReader = JoinHandle<()>;
