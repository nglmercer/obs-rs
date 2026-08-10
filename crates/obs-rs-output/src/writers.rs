use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use obs_rs_media::{VideoFormat, VideoFrame};

use super::{
    audio::Y4mRecording,
    error::OutputError,
    muxer::{encode_packets, MemoryMuxer},
    recording::RawRecording,
    stream::PacketMuxer,
    types::{EncodedPacket, OutputState},
};

pub struct AtomicRawFileWriter {
    recording: RawRecording,
    final_path: PathBuf,
    temp_path: PathBuf,
    state: OutputState,
    committed_bytes: Option<usize>,
}

impl AtomicRawFileWriter {
    /// Starts an open writer with explicit final and temporary paths.
    ///
    /// The temporary path must differ from the final path. The final path is not
    /// touched until [`Self::finalize`] has written and synchronized the temporary
    /// file successfully.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidPaths`] when both paths are equal or empty.
    pub fn new(
        final_path: impl Into<PathBuf>,
        temp_path: impl Into<PathBuf>,
        format: VideoFormat,
    ) -> Result<Self, OutputError> {
        let final_path = final_path.into();
        let temp_path = temp_path.into();
        if final_path.as_os_str().is_empty() || temp_path.as_os_str().is_empty() {
            return Err(OutputError::InvalidPaths {
                reason: "paths must be non-empty".to_owned(),
            });
        }
        if final_path == temp_path {
            return Err(OutputError::InvalidPaths {
                reason: "temporary and final paths must differ".to_owned(),
            });
        }
        Ok(Self {
            recording: RawRecording::new(format),
            final_path,
            temp_path,
            state: OutputState::Open,
            committed_bytes: None,
        })
    }

    /// Appends one frame while the writer is open.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] after finalization or abort, or a
    /// recording validation error for the frame itself.
    pub fn push(&mut self, frame: VideoFrame) -> Result<(), OutputError> {
        self.ensure_open("push a frame")?;
        self.recording.push(frame)
    }

    /// Writes the temporary file, synchronizes it, then atomically renames it to
    /// the final path.
    ///
    /// On any filesystem failure the temporary file is removed on a best-effort
    /// basis and the final path is left untouched.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open,
    /// [`OutputError::Write`] for filesystem failures, or a recording encoding
    /// error before any path is changed.
    pub fn finalize(&mut self) -> Result<usize, OutputError> {
        self.ensure_open("finalize")?;
        let bytes = self.recording.encode()?;
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.temp_path)
                .map_err(|error| OutputError::Write(format!("open temporary file: {error}")))?;
            file.write_all(&bytes)
                .map_err(|error| OutputError::Write(format!("write temporary file: {error}")))?;
            file.sync_all()
                .map_err(|error| OutputError::Write(format!("sync temporary file: {error}")))?;
            fs::rename(&self.temp_path, &self.final_path)
                .map_err(|error| OutputError::Write(format!("rename recording: {error}")))?;
            Ok::<(), OutputError>(())
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&self.temp_path);
            return Err(error);
        }

        self.committed_bytes = Some(bytes.len());
        self.state = OutputState::Finalized;
        Ok(bytes.len())
    }

    /// Aborts the writer and removes an uncommitted temporary file if present.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.ensure_open("abort")?;
        if let Err(error) = fs::remove_file(&self.temp_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(OutputError::Write(format!(
                    "remove temporary file: {error}"
                )));
            }
        }
        self.recording.frames.clear();
        self.state = OutputState::Aborted;
        Ok(())
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> OutputState {
        self.state
    }

    /// Returns the final path selected for this writer.
    #[must_use]
    pub fn final_path(&self) -> &std::path::Path {
        &self.final_path
    }

    /// Returns the temporary path selected for this writer.
    #[must_use]
    pub fn temp_path(&self) -> &std::path::Path {
        &self.temp_path
    }

    /// Returns the number of bytes committed by a successful finalization.
    #[must_use]
    pub const fn committed_bytes(&self) -> Option<usize> {
        self.committed_bytes
    }

    fn ensure_open(&self, operation: &'static str) -> Result<(), OutputError> {
        if self.state == OutputState::Open {
            Ok(())
        } else {
            Err(OutputError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}

/// A crash-safe YUV4MPEG2 writer using temp-file plus rename finalization.
///
/// The writer keeps the standard Y4M stream in memory while frames are being
/// validated, then publishes it only after the complete file has been written
/// and synchronized. This keeps a cancelled recording from becoming a file
/// that looks complete to another process.
pub struct AtomicY4mFileWriter {
    recording: Y4mRecording,
    final_path: PathBuf,
    temp_path: PathBuf,
    state: OutputState,
    committed_bytes: Option<usize>,
}

impl AtomicY4mFileWriter {
    /// Starts an open Y4M writer with explicit final and temporary paths.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidPaths`] when either path is empty or both
    /// paths are equal.
    pub fn new(
        final_path: impl Into<PathBuf>,
        temp_path: impl Into<PathBuf>,
        format: VideoFormat,
    ) -> Result<Self, OutputError> {
        let final_path = final_path.into();
        let temp_path = temp_path.into();
        if final_path.as_os_str().is_empty() || temp_path.as_os_str().is_empty() {
            return Err(OutputError::InvalidPaths {
                reason: "paths must be non-empty".to_owned(),
            });
        }
        if final_path == temp_path {
            return Err(OutputError::InvalidPaths {
                reason: "temporary and final paths must differ".to_owned(),
            });
        }
        Ok(Self {
            recording: Y4mRecording::new(format),
            final_path,
            temp_path,
            state: OutputState::Open,
            committed_bytes: None,
        })
    }

    /// Appends one frame while the writer is open.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] after finalization or abort, or a
    /// Y4M validation error for the frame itself.
    pub fn push(&mut self, frame: VideoFrame) -> Result<(), OutputError> {
        self.ensure_open("push a frame")?;
        self.recording.push(frame)
    }

    /// Writes, synchronizes, and atomically renames the complete Y4M stream.
    ///
    /// On a filesystem failure the temporary path is removed on a best-effort
    /// basis and the final path is left untouched.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open,
    /// [`OutputError::Write`] for filesystem failures, or a Y4M encoding error
    /// before either path is changed.
    pub fn finalize(&mut self) -> Result<usize, OutputError> {
        self.ensure_open("finalize")?;
        let bytes = self.recording.encode()?;
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.temp_path)
                .map_err(|error| OutputError::Write(format!("open temporary Y4M file: {error}")))?;
            file.write_all(&bytes).map_err(|error| {
                OutputError::Write(format!("write temporary Y4M file: {error}"))
            })?;
            file.sync_all()
                .map_err(|error| OutputError::Write(format!("sync temporary Y4M file: {error}")))?;
            fs::rename(&self.temp_path, &self.final_path)
                .map_err(|error| OutputError::Write(format!("rename Y4M recording: {error}")))?;
            Ok::<(), OutputError>(())
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&self.temp_path);
            return Err(error);
        }

        self.committed_bytes = Some(bytes.len());
        self.state = OutputState::Finalized;
        Ok(bytes.len())
    }

    /// Aborts the writer and removes an uncommitted temporary file if present.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open or
    /// [`OutputError::Write`] when the temporary file cannot be removed.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.ensure_open("abort")?;
        if let Err(error) = fs::remove_file(&self.temp_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(OutputError::Write(format!(
                    "remove temporary Y4M file: {error}"
                )));
            }
        }
        self.recording.frames.clear();
        self.recording.encoded_bytes = 0;
        self.recording.last_timestamp = None;
        self.state = OutputState::Aborted;
        Ok(())
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> OutputState {
        self.state
    }

    /// Returns the number of accepted frames.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.recording.len()
    }

    /// Returns the final path selected for this writer.
    #[must_use]
    pub fn final_path(&self) -> &std::path::Path {
        &self.final_path
    }

    /// Returns the temporary path selected for this writer.
    #[must_use]
    pub fn temp_path(&self) -> &std::path::Path {
        &self.temp_path
    }

    /// Returns the number of bytes committed by a successful finalization.
    #[must_use]
    pub const fn committed_bytes(&self) -> Option<usize> {
        self.committed_bytes
    }

    fn ensure_open(&self, operation: &'static str) -> Result<(), OutputError> {
        if self.state == OutputState::Open {
            Ok(())
        } else {
            Err(OutputError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}

/// A crash-safe packet-container writer using temp-file plus rename finalization.
///
/// The bytes use the deterministic `OBSRPKT1` packet container emitted by
/// [`MemoryMuxer`]. It is an inspectable Rust-native container fixture, not a
/// claim of compatibility with a broadcast container such as Matroska or MPEG-TS.
pub struct AtomicPacketFileWriter {
    muxer: MemoryMuxer,
    final_path: PathBuf,
    temp_path: PathBuf,
    state: OutputState,
    committed_bytes: Option<usize>,
}

impl AtomicPacketFileWriter {
    /// Starts an open writer with explicit final and temporary paths.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidPaths`] when either path is empty or both
    /// paths are equal.
    pub fn new(
        final_path: impl Into<PathBuf>,
        temp_path: impl Into<PathBuf>,
    ) -> Result<Self, OutputError> {
        let final_path = final_path.into();
        let temp_path = temp_path.into();
        if final_path.as_os_str().is_empty() || temp_path.as_os_str().is_empty() {
            return Err(OutputError::InvalidPaths {
                reason: "paths must be non-empty".to_owned(),
            });
        }
        if final_path == temp_path {
            return Err(OutputError::InvalidPaths {
                reason: "temporary and final paths must differ".to_owned(),
            });
        }
        Ok(Self {
            muxer: MemoryMuxer::new(),
            final_path,
            temp_path,
            state: OutputState::Open,
            committed_bytes: None,
        })
    }

    /// Appends one encoded packet while the writer is open.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] after finalization or abort, or a
    /// packet-container validation error.
    pub fn push(&mut self, packet: EncodedPacket) -> Result<(), OutputError> {
        self.ensure_open("push a packet")?;
        self.muxer.push(packet)
    }

    /// Writes, synchronizes, and atomically renames the packet container.
    ///
    /// On a filesystem failure the temporary path is removed on a best-effort
    /// basis and the final path is left untouched.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open,
    /// [`OutputError::Write`] for filesystem failures, or a container encoding
    /// error before any path is changed.
    pub fn finalize(&mut self) -> Result<usize, OutputError> {
        self.ensure_open("finalize")?;
        let bytes = encode_packets(self.muxer.packets())?;
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.temp_path)
                .map_err(|error| OutputError::Write(format!("open temporary file: {error}")))?;
            file.write_all(&bytes)
                .map_err(|error| OutputError::Write(format!("write temporary file: {error}")))?;
            file.sync_all()
                .map_err(|error| OutputError::Write(format!("sync temporary file: {error}")))?;
            fs::rename(&self.temp_path, &self.final_path)
                .map_err(|error| OutputError::Write(format!("rename packet container: {error}")))?;
            Ok::<(), OutputError>(())
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&self.temp_path);
            return Err(error);
        }

        self.committed_bytes = Some(bytes.len());
        self.state = OutputState::Finalized;
        Ok(bytes.len())
    }

    /// Aborts the writer and removes an uncommitted temporary file if present.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open or
    /// [`OutputError::Write`] when the temporary file cannot be removed.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.ensure_open("abort")?;
        if let Err(error) = fs::remove_file(&self.temp_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(OutputError::Write(format!(
                    "remove temporary file: {error}"
                )));
            }
        }
        self.muxer.abort()?;
        self.state = OutputState::Aborted;
        Ok(())
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> OutputState {
        self.state
    }

    /// Returns the number of packets accepted by the writer.
    #[must_use]
    pub fn packet_count(&self) -> usize {
        self.muxer.packets().len()
    }

    /// Returns the final path selected for this writer.
    #[must_use]
    pub fn final_path(&self) -> &std::path::Path {
        &self.final_path
    }

    /// Returns the temporary path selected for this writer.
    #[must_use]
    pub fn temp_path(&self) -> &std::path::Path {
        &self.temp_path
    }

    /// Returns the number of bytes committed by a successful finalization.
    #[must_use]
    pub const fn committed_bytes(&self) -> Option<usize> {
        self.committed_bytes
    }

    fn ensure_open(&self, operation: &'static str) -> Result<(), OutputError> {
        if self.state == OutputState::Open {
            Ok(())
        } else {
            Err(OutputError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}
