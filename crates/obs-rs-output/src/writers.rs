use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{
    audio::Y4mRecording,
    error::OutputError,
    recording::RawRecording,
    types::{EncodedPacket, OutputState, PacketKind},
    MAX_RECORDING_BYTES, MAX_RECORDING_FRAMES, PACKET_HEADER_BYTES, PACKET_MAGIC,
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
/// [`crate::MemoryMuxer`]. It is an inspectable Rust-native container fixture, not a
/// claim of compatibility with a broadcast container such as Matroska or MPEG-TS.
pub struct AtomicPacketFileWriter {
    writer: Option<BufWriter<File>>,
    final_path: PathBuf,
    temp_path: PathBuf,
    state: OutputState,
    committed_bytes: Option<usize>,
    packet_count: usize,
    encoded_bytes: usize,
    last_timestamp: Option<obs_rs_media::Timestamp>,
}

impl AtomicPacketFileWriter {
    /// Starts an open writer with explicit final and temporary paths.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidPaths`] when either path is empty or both
    /// paths are equal, or [`OutputError::Write`] when the temporary stream
    /// cannot be opened and initialized.
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
        let mut writer = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(&temp_path)
                .map_err(|error| {
                    OutputError::Write(format!("open temporary packet file: {error}"))
                })?,
        );
        if let Err(error) = writer
            .write_all(PACKET_MAGIC)
            .and_then(|()| writer.write_all(&0_u64.to_le_bytes()))
        {
            drop(writer);
            let _ = fs::remove_file(&temp_path);
            return Err(OutputError::Write(format!(
                "initialize temporary packet file: {error}"
            )));
        }
        Ok(Self {
            writer: Some(writer),
            final_path,
            temp_path,
            state: OutputState::Open,
            committed_bytes: None,
            packet_count: 0,
            encoded_bytes: PACKET_HEADER_BYTES,
            last_timestamp: None,
        })
    }

    /// Appends one encoded packet while the writer is open.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] after finalization or abort, or a
    /// packet-container validation error.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "preserves the consuming writer API and prevents accidental packet reuse"
    )]
    pub fn push(&mut self, packet: EncodedPacket) -> Result<(), OutputError> {
        self.ensure_open("push a packet")?;
        if self.packet_count >= MAX_RECORDING_FRAMES {
            return Err(OutputError::TooManyFrames {
                frames: self.packet_count as u64 + 1,
            });
        }
        if let Some(previous) = self.last_timestamp {
            if packet.timestamp() < previous {
                return Err(OutputError::NonMonotonicTimestamp {
                    previous,
                    actual: packet.timestamp(),
                });
            }
        }
        let packet_bytes = 18_usize
            .checked_add(packet.byte_len())
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let encoded_bytes = self
            .encoded_bytes
            .checked_add(packet_bytes)
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        if encoded_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::TooLarge {
                bytes: encoded_bytes as u64,
            });
        }

        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| OutputError::Write("temporary packet file is unavailable".to_owned()))?;
        let write_result = writer
            .write_all(&[packet.kind().tag(), u8::from(packet.is_keyframe())])
            .and_then(|()| writer.write_all(&packet.timestamp().as_nanos().to_le_bytes()))
            .and_then(|()| writer.write_all(&(packet.byte_len() as u64).to_le_bytes()))
            .and_then(|()| writer.write_all(packet.payload()));
        if let Err(error) = write_result {
            return Err(OutputError::Write(format!(
                "append temporary packet file: {error}"
            )));
        }
        self.packet_count += 1;
        self.encoded_bytes = encoded_bytes;
        self.last_timestamp = Some(packet.timestamp());
        Ok(())
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
        let mut writer = self
            .writer
            .take()
            .ok_or_else(|| OutputError::Write("temporary packet file is unavailable".to_owned()))?;
        let finalize_stream = writer
            .flush()
            .and_then(|()| {
                writer
                    .seek(SeekFrom::Start(PACKET_MAGIC.len() as u64))
                    .map(|_| ())
            })
            .and_then(|()| writer.write_all(&(self.packet_count as u64).to_le_bytes()))
            .and_then(|()| writer.flush())
            .and_then(|()| writer.get_ref().sync_all());
        if let Err(error) = finalize_stream {
            self.writer = Some(writer);
            return Err(OutputError::Write(format!(
                "finalize temporary packet file: {error}"
            )));
        }
        drop(writer);
        if let Err(error) = fs::rename(&self.temp_path, &self.final_path) {
            self.writer = reopen_packet_stream(&self.temp_path).ok();
            return Err(OutputError::Write(format!(
                "rename packet container: {error}"
            )));
        }

        self.committed_bytes = Some(self.encoded_bytes);
        self.state = OutputState::Finalized;
        Ok(self.encoded_bytes)
    }

    /// Aborts the writer and removes an uncommitted temporary file if present.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open or
    /// [`OutputError::Write`] when the temporary file cannot be removed.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.ensure_open("abort")?;
        self.writer = None;
        if let Err(error) = fs::remove_file(&self.temp_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(OutputError::Write(format!(
                    "remove temporary file: {error}"
                )));
            }
        }
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
    pub const fn packet_count(&self) -> usize {
        self.packet_count
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

fn reopen_packet_stream(path: &std::path::Path) -> Result<BufWriter<File>, std::io::Error> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    file.seek(SeekFrom::End(0))?;
    Ok(BufWriter::new(file))
}

/// Maximum number of atomically published segments one split recording may own.
pub const MAX_RECORDING_SEGMENTS: usize = 1_024;

/// Largest duration accepted by one split-recording policy.
pub const MAX_SEGMENT_DURATION: Duration = Duration::from_hours(24);

/// Result of removing bounded incomplete recording artifacts after a crash.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecordingRecoveryReport {
    removed_files: usize,
    removed_bytes: usize,
}

impl RecordingRecoveryReport {
    /// Returns the number of incomplete artifacts removed.
    #[must_use]
    pub const fn removed_files(self) -> usize {
        self.removed_files
    }

    /// Returns the total size of incomplete artifacts removed.
    #[must_use]
    pub const fn removed_bytes(self) -> usize {
        self.removed_bytes
    }
}

/// Removes only known hidden packet-recording artifacts for one base path.
///
/// Published recordings are never touched. The scan is bounded by
/// [`MAX_RECORDING_SEGMENTS`], so a malformed directory cannot turn startup
/// recovery into an unbounded directory walk.
///
/// # Errors
///
/// Returns [`OutputError::InvalidPaths`] when the base path does not name a
/// UTF-8 file, or [`OutputError::Write`] when an artifact cannot be inspected
/// or removed.
pub fn recover_stale_packet_files(
    base_path: impl AsRef<Path>,
) -> Result<RecordingRecoveryReport, OutputError> {
    let base_path = base_path.as_ref();
    if base_path.as_os_str().is_empty() || base_path.file_name().is_none() {
        return Err(OutputError::InvalidPaths {
            reason: "recovery base path must name a file".to_owned(),
        });
    }
    let file_name = base_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| OutputError::InvalidPaths {
            reason: "recovery base path must have a UTF-8 file name".to_owned(),
        })?;
    let mut report = RecordingRecoveryReport::default();
    remove_recovery_artifact(
        &base_path.with_file_name(format!("{file_name}.tmp")),
        &mut report,
    )?;
    for index in 1..=MAX_RECORDING_SEGMENTS {
        let (_, temp_path) = segment_paths(base_path, index)?;
        remove_recovery_artifact(&temp_path, &mut report)?;
    }
    Ok(report)
}

fn remove_recovery_artifact(
    path: &Path,
    report: &mut RecordingRecoveryReport,
) -> Result<(), OutputError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(OutputError::Write(format!(
                "inspect stale recording artifact: {error}"
            )))
        }
    };
    fs::remove_file(path)
        .map_err(|error| OutputError::Write(format!("remove stale recording artifact: {error}")))?;
    report.removed_files = report.removed_files.saturating_add(1);
    report.removed_bytes = report
        .removed_bytes
        .saturating_add(usize::try_from(metadata.len()).unwrap_or(usize::MAX));
    Ok(())
}

/// Policy for bounded keyframe-aligned packet recording segments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SegmentedRecordingPolicy {
    segment_bytes: usize,
    segment_duration: Duration,
    segments: usize,
}

impl SegmentedRecordingPolicy {
    /// Creates a policy with byte, duration, and segment-count bounds.
    ///
    /// The byte target includes the packet-container header. A segment may
    /// exceed the target until the next video keyframe so every published
    /// segment starts at a random-access point.
    ///
    /// # Errors
    ///
    /// Returns `OutputError::InvalidSegmentPolicy` when a bound is zero or
    /// exceeds the repository-wide safety limit.
    pub fn new(
        max_segment_bytes: usize,
        max_segment_duration: Duration,
        max_segments: usize,
    ) -> Result<Self, OutputError> {
        let minimum_bytes = PACKET_HEADER_BYTES.saturating_add(19);
        if max_segment_bytes < minimum_bytes || max_segment_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::InvalidSegmentPolicy {
                reason: format!(
                    "segment bytes must be between {minimum_bytes} and {MAX_RECORDING_BYTES}"
                ),
            });
        }
        if max_segment_duration.is_zero() || max_segment_duration > MAX_SEGMENT_DURATION {
            return Err(OutputError::InvalidSegmentPolicy {
                reason: format!(
                    "segment duration must be between 1 ns and {MAX_SEGMENT_DURATION:?}"
                ),
            });
        }
        if max_segments == 0 || max_segments > MAX_RECORDING_SEGMENTS {
            return Err(OutputError::InvalidSegmentPolicy {
                reason: format!("segment count must be between 1 and {MAX_RECORDING_SEGMENTS}"),
            });
        }
        Ok(Self {
            segment_bytes: max_segment_bytes,
            segment_duration: max_segment_duration,
            segments: max_segments,
        })
    }

    /// Returns the target byte size for each segment.
    #[must_use]
    pub const fn max_segment_bytes(self) -> usize {
        self.segment_bytes
    }

    /// Returns the target duration for each segment.
    #[must_use]
    pub const fn max_segment_duration(self) -> Duration {
        self.segment_duration
    }

    /// Returns the maximum number of segments.
    #[must_use]
    pub const fn max_segments(self) -> usize {
        self.segments
    }
}

/// One atomically published split-recording segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordingSegment {
    path: PathBuf,
    bytes: usize,
    packets: usize,
}

impl RecordingSegment {
    /// Returns the published segment path.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Returns the committed container size, including its header.
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }

    /// Returns the number of encoded packets in this segment.
    #[must_use]
    pub const fn packets(&self) -> usize {
        self.packets
    }
}

/// A bounded packet writer that publishes independent keyframe-aligned files.
pub struct SegmentedPacketFileWriter {
    base_path: PathBuf,
    policy: SegmentedRecordingPolicy,
    current: Option<AtomicPacketFileWriter>,
    current_bytes: usize,
    current_packets: usize,
    current_start: Option<Timestamp>,
    next_index: usize,
    last_timestamp: Option<Timestamp>,
    segments: Vec<RecordingSegment>,
    total_bytes: usize,
    state: OutputState,
}

impl SegmentedPacketFileWriter {
    /// Starts a split writer whose files are named stem-NNNN.extension.
    ///
    /// Each segment has its own hidden .part file and becomes visible only
    /// after its packet header is patched, synchronized, and atomically renamed.
    ///
    /// # Errors
    ///
    /// Returns `OutputError::InvalidPaths` for a path without a file name or
    /// `OutputError::Write` when the first temporary segment cannot open.
    pub fn new(
        base_path: impl Into<PathBuf>,
        policy: SegmentedRecordingPolicy,
    ) -> Result<Self, OutputError> {
        let base_path = base_path.into();
        if base_path.as_os_str().is_empty() {
            return Err(OutputError::InvalidPaths {
                reason: "split base path must be non-empty".to_owned(),
            });
        }
        if base_path.file_name().is_none() {
            return Err(OutputError::InvalidPaths {
                reason: "split base path must name a file".to_owned(),
            });
        }
        let current = Self::open_segment(&base_path, 1)?;
        Ok(Self {
            base_path,
            policy,
            current: Some(current),
            current_bytes: PACKET_HEADER_BYTES,
            current_packets: 0,
            current_start: None,
            next_index: 1,
            last_timestamp: None,
            segments: Vec::with_capacity(policy.max_segments()),
            total_bytes: 0,
            state: OutputState::Open,
        })
    }

    /// Appends one packet and rotates before a video keyframe when a target is
    /// due. Packets after the target remain in the current segment until that
    /// keyframe arrives, preserving independent decoder entry points.
    ///
    /// # Errors
    ///
    /// Returns a timestamp, policy, segment-count, filesystem, or packet
    /// container error without discarding an open segment.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "matches AtomicPacketFileWriter::push and keeps packet ownership single-use"
    )]
    pub fn push(&mut self, packet: EncodedPacket) -> Result<(), OutputError> {
        self.ensure_open("push a split packet")?;
        let timestamp = packet.timestamp();
        if let Some(previous) = self.last_timestamp {
            if timestamp < previous {
                return Err(OutputError::NonMonotonicTimestamp {
                    previous,
                    actual: timestamp,
                });
            }
        }
        let packet_bytes = 18_usize
            .checked_add(packet.byte_len())
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let full_packet_bytes = PACKET_HEADER_BYTES
            .checked_add(packet_bytes)
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        if full_packet_bytes > self.policy.max_segment_bytes() {
            return Err(OutputError::SegmentPacketDoesNotFit {
                packet_bytes: full_packet_bytes,
                max_bytes: self.policy.max_segment_bytes(),
            });
        }
        if self.should_rotate(timestamp, packet.kind(), packet.is_keyframe(), packet_bytes) {
            self.rotate()?;
        }
        let current = self
            .current
            .as_mut()
            .ok_or_else(|| OutputError::Write("split segment is unavailable".to_owned()))?;
        current.push(packet)?;
        self.current_bytes = self.current_bytes.saturating_add(packet_bytes);
        self.current_packets = self.current_packets.saturating_add(1);
        if self.current_start.is_none() {
            self.current_start = Some(timestamp);
        }
        self.last_timestamp = Some(timestamp);
        Ok(())
    }

    /// Flushes and atomically publishes the final segment.
    ///
    /// # Errors
    ///
    /// Returns `OutputError::InvalidState` when already finalized or aborted,
    /// or the filesystem error from the current segment.
    pub fn finalize(&mut self) -> Result<usize, OutputError> {
        self.ensure_open("finalize split recording")?;
        let mut current = self
            .current
            .take()
            .ok_or_else(|| OutputError::Write("split segment is unavailable".to_owned()))?;
        let path = current.final_path().to_owned();
        let packets = current.packet_count();
        match current.finalize() {
            Ok(bytes) => {
                self.record_segment(path, bytes, packets);
                self.state = OutputState::Finalized;
                Ok(self.total_bytes)
            }
            Err(error) => {
                self.current = Some(current);
                Err(error)
            }
        }
    }

    /// Aborts the split recording and removes every known segment.
    ///
    /// # Errors
    ///
    /// Returns the first filesystem removal error while still transitioning
    /// the writer to `OutputState::Aborted`.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.ensure_open("abort split recording")?;
        let mut first_error = None;
        if let Some(mut current) = self.current.take() {
            if let Err(error) = current.abort() {
                first_error = Some(error);
            }
        }
        for segment in &self.segments {
            if let Err(error) = fs::remove_file(&segment.path) {
                if error.kind() != std::io::ErrorKind::NotFound && first_error.is_none() {
                    first_error =
                        Some(OutputError::Write(format!("remove split segment: {error}")));
                }
            }
        }
        self.segments.clear();
        self.state = OutputState::Aborted;
        first_error.map_or(Ok(()), Err)
    }

    /// Returns the lifecycle state of the split writer.
    #[must_use]
    pub const fn state(&self) -> OutputState {
        self.state
    }

    /// Returns the base path used to derive numbered segment names.
    #[must_use]
    pub fn base_path(&self) -> &std::path::Path {
        &self.base_path
    }

    /// Returns completed, atomically published segments in order.
    #[must_use]
    pub fn segments(&self) -> &[RecordingSegment] {
        &self.segments
    }

    /// Returns the number of packets in the open segment.
    #[must_use]
    pub const fn current_segment_packets(&self) -> usize {
        self.current_packets
    }

    /// Returns the bytes accepted by the open segment, including its header.
    #[must_use]
    pub const fn current_segment_bytes(&self) -> usize {
        self.current_bytes
    }

    fn should_rotate(
        &self,
        timestamp: Timestamp,
        kind: PacketKind,
        keyframe: bool,
        packet_bytes: usize,
    ) -> bool {
        if self.current_packets == 0 || kind != PacketKind::Video || !keyframe {
            return false;
        }
        let over_bytes =
            self.current_bytes.saturating_add(packet_bytes) > self.policy.max_segment_bytes();
        let over_duration = self.current_start.is_some_and(|start| {
            u128::from(timestamp.as_nanos().saturating_sub(start.as_nanos()))
                >= self.policy.max_segment_duration().as_nanos()
        });
        over_bytes || over_duration
    }

    fn rotate(&mut self) -> Result<(), OutputError> {
        if self.segments.len().saturating_add(1) >= self.policy.max_segments() {
            return Err(OutputError::TooManySegments {
                segments: self.policy.max_segments(),
            });
        }
        let next_index = self.next_index.saturating_add(1);
        let next = Self::open_segment(&self.base_path, next_index)?;
        let mut current = self
            .current
            .take()
            .ok_or_else(|| OutputError::Write("split segment is unavailable".to_owned()))?;
        let path = current.final_path().to_owned();
        let packets = current.packet_count();
        match current.finalize() {
            Ok(bytes) => {
                self.record_segment(path, bytes, packets);
                self.current = Some(next);
                self.next_index = next_index;
                self.current_bytes = PACKET_HEADER_BYTES;
                self.current_packets = 0;
                self.current_start = None;
                Ok(())
            }
            Err(error) => {
                let mut next = next;
                let _ = next.abort();
                self.current = Some(current);
                Err(error)
            }
        }
    }

    fn open_segment(
        base_path: &std::path::Path,
        index: usize,
    ) -> Result<AtomicPacketFileWriter, OutputError> {
        let (final_path, temp_path) = segment_paths(base_path, index)?;
        AtomicPacketFileWriter::new(final_path, temp_path)
    }

    fn record_segment(&mut self, path: PathBuf, bytes: usize, packets: usize) {
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.segments.push(RecordingSegment {
            path,
            bytes,
            packets,
        });
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

fn segment_paths(
    base_path: &std::path::Path,
    index: usize,
) -> Result<(PathBuf, PathBuf), OutputError> {
    let stem = base_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OutputError::InvalidPaths {
            reason: "split base path must have a UTF-8 file stem".to_owned(),
        })?;
    let extension = base_path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("obsr");
    let name = format!("{stem}-{index:04}.{extension}");
    let final_path = base_path.with_file_name(&name);
    let temp_path = base_path.with_file_name(format!("{name}.part"));
    Ok((final_path, temp_path))
}
