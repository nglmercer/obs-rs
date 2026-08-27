use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use obs_rs_media::Timestamp;

use super::{
    error::OutputError,
    types::{EncodedPacket, OutputState, PacketKind},
    MAX_RECORDING_BYTES, PACKET_HEADER_BYTES,
};

mod atomic;

pub use atomic::{AtomicPacketFileWriter, AtomicRawFileWriter, AtomicY4mFileWriter};

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
