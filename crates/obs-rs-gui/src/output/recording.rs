use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime},
};

use obs_rs_engine::{OutputCapabilitiesSnapshot, OutputLifecycle, RemuxRecovery, ReplaySaveStatus};
use obs_rs_output::{OutputProfile, OutputProfileKind, SegmentedRecordingPolicy};

use crate::{
    settings::{
        recording_stamp, RecordingFormat, RECORDING_SPLIT_DURATION_MINUTES_RANGE,
        RECORDING_SPLIT_SEGMENTS_DEFAULT, RECORDING_SPLIT_SEGMENTS_RANGE,
        RECORDING_SPLIT_SIZE_MIB_RANGE, REPLAY_BUFFER_CAPACITY_MIB_RANGE,
        REPLAY_BUFFER_DURATION_RANGE,
    },
    AppSettings,
};

use super::OutputRuntime;

impl OutputRuntime {
    #[cfg(test)]
    pub(crate) fn start_recording(&mut self, path: &str) -> Result<(), Box<dyn Error>> {
        if let Some(policy) = self.segmented_recording_policy {
            validate_segmented_recording_path(path)?;
            self.worker.start_segmented_recording_configured(
                path,
                policy,
                self.recording_video_encoder.clone(),
                self.recording_audio_encoder.clone(),
            )?;
        } else if self.segmented_recording_requested && is_production_recording_path(path) {
            return Err(
                "configured production split recording is unavailable on this host"
                    .to_owned()
                    .into(),
            );
        } else if self.auto_remux_requested {
            if !self.auto_remux_enabled {
                return Err(
                    "automatic Matroska-to-MP4 remux is unavailable on this host"
                        .to_owned()
                        .into(),
                );
            }
            validate_auto_remux_path(path)?;
            self.worker.start_remux_recording_configured(
                path,
                self.recording_video_encoder.clone(),
                self.recording_audio_encoder.clone(),
            )?;
        } else if let Some(profile) = self.recording_profile {
            self.worker.start_recording_profile(
                path,
                profile,
                Some((
                    self.recording_video_encoder.clone(),
                    self.recording_audio_encoder.clone(),
                )),
            )?;
        } else if is_production_recording_path(path) {
            self.worker.start_recording_configured(
                path,
                self.recording_video_encoder.clone(),
                self.recording_audio_encoder.clone(),
            )?;
        } else {
            self.worker.start_recording(path)?;
        }
        self.recording_started_at = Some(Instant::now());
        Ok(())
    }

    /// Enqueues recording setup without waiting for container or encoder work.
    pub(crate) fn request_start_recording(&mut self, path: &str) -> Result<(), Box<dyn Error>> {
        if let Some(policy) = self.segmented_recording_policy {
            validate_segmented_recording_path(path)?;
            self.worker.try_start_segmented_recording_configured(
                path,
                policy,
                self.recording_video_encoder.clone(),
                self.recording_audio_encoder.clone(),
            )?;
        } else if self.segmented_recording_requested && is_production_recording_path(path) {
            return Err(
                "configured production split recording is unavailable on this host"
                    .to_owned()
                    .into(),
            );
        } else if self.auto_remux_requested {
            if !self.auto_remux_enabled {
                return Err(
                    "automatic Matroska-to-MP4 remux is unavailable on this host"
                        .to_owned()
                        .into(),
                );
            }
            validate_auto_remux_path(path)?;
            self.worker.try_start_remux_recording(
                path,
                Some((
                    self.recording_video_encoder.clone(),
                    self.recording_audio_encoder.clone(),
                )),
            )?;
        } else if let Some(profile) = self.recording_profile {
            self.worker.try_start_recording_profile(
                path,
                profile,
                Some((
                    self.recording_video_encoder.clone(),
                    self.recording_audio_encoder.clone(),
                )),
            )?;
        } else {
            let encoder_config = is_production_recording_path(path).then(|| {
                (
                    self.recording_video_encoder.clone(),
                    self.recording_audio_encoder.clone(),
                )
            });
            self.worker.try_start_recording(path, encoder_config)?;
        }
        self.recording_started_at = Some(Instant::now());
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn finish_recording(&mut self) -> Result<usize, Box<dyn Error>> {
        let bytes = self.worker.finish_recording()?;
        self.recording_started_at = None;
        Ok(bytes)
    }

    /// Enqueues recording finalization without waiting for container work.
    pub(crate) fn request_finish_recording(&mut self) -> Result<(), Box<dyn Error>> {
        self.worker.try_finish_recording()?;
        self.recording_started_at = None;
        Ok(())
    }

    /// Returns whether this host can perform the native H.264/AAC remux
    /// boundary used by the explicit recovery action.
    pub(crate) const fn remux_recovery_supported(&self) -> bool {
        self.capabilities.supports_remux()
    }

    /// Returns whether one recovery request is currently being processed.
    pub(crate) const fn remux_recovery_running(&self) -> bool {
        self.remux_recovery.is_some() || self.remux_candidates.is_some()
    }

    /// Enqueues a bounded scan of the final recording's directory.
    ///
    /// Discovery is kept separate from remuxing so the GUI can present every
    /// recoverable artifact and never guess which recording the operator meant.
    pub(crate) fn request_discover_interrupted_remux_candidates(
        &mut self,
        path: &str,
    ) -> Result<(), Box<dyn Error>> {
        validate_auto_remux_path(path)?;
        let path = Path::new(path);
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        self.request_discover_interrupted_remux_directory(directory)
    }

    /// Queues the one startup scan for interrupted automatic-remux artifacts.
    ///
    /// Startup recovery is deliberately explicit: a matching durable manifest
    /// can open the existing chooser, but the GUI never remuxes a recording or
    /// claims a configured `.mkv` path was a final destination on its own.
    /// The configured path is used only to select its output directory, so this
    /// remains useful after the user changes the recording format or filename.
    pub(crate) fn request_startup_remux_discovery(
        &mut self,
        configured_path: &str,
    ) -> Result<(), Box<dyn Error>> {
        if configured_path.trim().is_empty() {
            return Err("startup remux recovery requires a configured recording path".into());
        }
        let path = Path::new(configured_path);
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        self.request_discover_interrupted_remux_directory(directory)
    }

    fn request_discover_interrupted_remux_directory(
        &mut self,
        directory: PathBuf,
    ) -> Result<(), Box<dyn Error>> {
        if !self.remux_recovery_supported() {
            return Err("interrupted remux recovery is unavailable on this host".into());
        }
        if self.remux_recovery_running() {
            return Err("interrupted remux recovery is already in progress".into());
        }
        let (recording, streaming) = self.lifecycles();
        if !recording.is_stopped() || !streaming.is_stopped() {
            return Err("stop recording and streaming before recovering a recording".into());
        }
        self.remux_candidates = Some(
            self.worker
                .try_discover_interrupted_remux_candidates(directory)?,
        );
        Ok(())
    }

    /// Enqueues recovery for one candidate selected by the GUI.
    ///
    /// The worker owns the potentially long native demux/remux operation; the
    /// GUI receives a one-shot result during its normal bounded refresh.
    pub(crate) fn request_recover_interrupted_remux(
        &mut self,
        path: &str,
    ) -> Result<(), Box<dyn Error>> {
        if !self.remux_recovery_supported() {
            return Err("interrupted remux recovery is unavailable on this host".into());
        }
        if self.remux_recovery_running() {
            return Err("interrupted remux recovery is already in progress".into());
        }
        let (recording, streaming) = self.lifecycles();
        if !recording.is_stopped() || !streaming.is_stopped() {
            return Err("stop recording and streaming before recovering a recording".into());
        }
        validate_auto_remux_path(path)?;
        self.remux_recovery = Some(self.worker.try_recover_interrupted_remux_recording(path)?);
        Ok(())
    }

    /// Takes the completed candidate-discovery result without waiting for
    /// filesystem work.
    pub(crate) fn take_remux_candidate_result(&mut self) -> Option<Result<Vec<PathBuf>, String>> {
        let receiver = self.remux_candidates.as_ref()?;
        match receiver.try_recv() {
            Ok(result) => {
                self.remux_candidates = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.remux_candidates = None;
                Some(Err(
                    "candidate-discovery worker disconnected before reporting a result".to_owned(),
                ))
            }
        }
    }

    /// Takes the completed recovery result without waiting for native work.
    pub(crate) fn take_remux_recovery_result(&mut self) -> Option<Result<RemuxRecovery, String>> {
        let receiver = self.remux_recovery.as_ref()?;
        match receiver.try_recv() {
            Ok(result) => {
                self.remux_recovery = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.remux_recovery = None;
                Some(Err(
                    "recovery worker disconnected before reporting a result".to_owned(),
                ))
            }
        }
    }

    pub(crate) fn abort_recording(&mut self) {
        self.worker.abort_recording();
        self.recording_started_at = None;
    }

    /// Enqueues the bounded replay capture configuration without waiting for
    /// worker-side allocation.
    pub(crate) fn request_start_replay_buffer(&mut self) -> Result<(), Box<dyn Error>> {
        self.worker.try_start_replay_buffer(
            self.replay_buffer_capacity_bytes,
            self.replay_buffer_duration,
        )?;
        Ok(())
    }

    /// Applies the bounded replay settings for the next replay-buffer start.
    ///
    /// An active history is intentionally left untouched: changing its limits
    /// in place would require an unbounded or blocking copy on the UI path.
    pub(crate) fn configure_replay(&mut self, settings: &AppSettings) {
        let duration = settings.replay_buffer_duration_seconds.clamp(
            *REPLAY_BUFFER_DURATION_RANGE.start(),
            *REPLAY_BUFFER_DURATION_RANGE.end(),
        );
        let capacity = settings.replay_buffer_capacity_mib.clamp(
            *REPLAY_BUFFER_CAPACITY_MIB_RANGE.start(),
            *REPLAY_BUFFER_CAPACITY_MIB_RANGE.end(),
        );
        self.replay_buffer_duration = Duration::from_secs(u64::from(duration));
        self.replay_buffer_capacity_bytes = replay_capacity_bytes(capacity);
    }

    /// Applies the bounded split policy for the next recording.
    ///
    /// Reference recordings use the portable packet writer. Production
    /// containers use the native bounded split-muxer boundary when the host
    /// advertises it; the two paths share only this policy, not a container or
    /// a second source of recording state.
    pub(crate) fn configure_recording(&mut self, settings: &AppSettings) {
        let format = settings.effective_recording_format();
        self.segmented_recording_requested =
            settings.recording_split_enabled && format != RecordingFormat::ReferencePacket;
        self.auto_remux_requested = settings.recording_auto_remux
            && !settings.recording_split_enabled
            && format == RecordingFormat::Matroska;
        self.auto_remux_enabled = self.auto_remux_requested && self.capabilities.supports_remux();
        let enabled = settings.recording_split_enabled
            && segmented_recording_format_available(&self.capabilities, format);
        self.recording_profile = (format == RecordingFormat::FragmentedMp4 && !enabled)
            .then_some(OutputProfile::fragmented_mp4_h264_aac());
        self.segmented_recording_policy = enabled.then(|| {
            let duration = settings.recording_split_duration_minutes.clamp(
                *RECORDING_SPLIT_DURATION_MINUTES_RANGE.start(),
                *RECORDING_SPLIT_DURATION_MINUTES_RANGE.end(),
            );
            let size = settings.recording_split_size_mib.clamp(
                *RECORDING_SPLIT_SIZE_MIB_RANGE.start(),
                *RECORDING_SPLIT_SIZE_MIB_RANGE.end(),
            );
            let segments = settings.recording_split_max_segments.clamp(
                *RECORDING_SPLIT_SEGMENTS_RANGE.start(),
                *RECORDING_SPLIT_SEGMENTS_RANGE.end(),
            );
            SegmentedRecordingPolicy::new(
                replay_capacity_bytes(size),
                Duration::from_secs(u64::from(duration).saturating_mul(60)),
                usize::try_from(segments)
                    .unwrap_or(usize::try_from(RECORDING_SPLIT_SEGMENTS_DEFAULT).unwrap_or(1_024)),
            )
            .expect("GUI split bounds must produce a valid output policy")
        });
    }

    #[cfg(test)]
    pub(crate) const fn segmented_recording_policy(&self) -> Option<SegmentedRecordingPolicy> {
        self.segmented_recording_policy
    }

    #[cfg(test)]
    pub(crate) const fn recording_profile_kind(&self) -> Option<obs_rs_output::OutputProfileKind> {
        match self.recording_profile {
            Some(profile) => Some(profile.kind()),
            None => None,
        }
    }

    /// Returns the operator-facing replay configuration label.
    pub(crate) fn replay_configuration_label(&self) -> String {
        format!(
            "{} s / {} MiB",
            self.replay_buffer_duration.as_secs(),
            self.replay_buffer_capacity_bytes / (1024 * 1024)
        )
    }

    /// Enqueues replay teardown without waiting for the worker.
    pub(crate) fn request_stop_replay_buffer(&mut self) -> Result<(), Box<dyn Error>> {
        self.worker.try_stop_replay_buffer()?;
        Ok(())
    }

    /// Enqueues an atomic replay save into the recording directory and returns
    /// the concrete path for the operator-facing status message.
    pub(crate) fn request_save_replay_buffer(
        &mut self,
        recording_path: &str,
    ) -> Result<String, Box<dyn Error>> {
        let path = replay_save_path(recording_path);
        self.worker.try_save_replay_buffer(path.clone())?;
        Ok(path.to_string_lossy().into_owned())
    }

    /// Returns the two projections the Controls dock needs from one snapshot.
    pub(crate) fn replay_controls(&self) -> (bool, bool) {
        let snapshot = self.worker.snapshot();
        let buffering = snapshot.alive
            && matches!(
                snapshot.engine.replay_lifecycle,
                OutputLifecycle::Starting | OutputLifecycle::Running | OutputLifecycle::Stopping
            );
        let saving = snapshot.alive
            && matches!(snapshot.engine.replay_save_status, ReplaySaveStatus::Saving);
        (buffering, saving)
    }
}

fn is_production_recording_path(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("mkv")
            || extension.eq_ignore_ascii_case("mp4")
            || extension.eq_ignore_ascii_case("mov")
            || extension.eq_ignore_ascii_case("flv")
    })
}

fn segmented_recording_format_available(
    capabilities: &OutputCapabilitiesSnapshot,
    format: RecordingFormat,
) -> bool {
    if format == RecordingFormat::ReferencePacket {
        return true;
    }
    if !capabilities.supports_segmented_recording() {
        return false;
    }
    let profile = match format {
        RecordingFormat::Matroska => OutputProfileKind::MatroskaH264Aac,
        RecordingFormat::Mp4 | RecordingFormat::FragmentedMp4 => OutputProfileKind::Mp4H264Aac,
        RecordingFormat::Mov => OutputProfileKind::MovH264Aac,
        RecordingFormat::Flv => OutputProfileKind::FlvH264Aac,
        RecordingFormat::ReferencePacket => return true,
    };
    capabilities.recording_formats().contains(&profile)
}

fn validate_segmented_recording_path(path: &str) -> Result<(), Box<dyn Error>> {
    if Path::new(path).extension().is_some_and(|extension| {
        ["obsr", "mkv", "mp4", "mov", "flv"]
            .into_iter()
            .any(|expected| extension.eq_ignore_ascii_case(expected))
    }) {
        Ok(())
    } else {
        Err("split recording requires .obsr, .mkv, .mp4, .mov, or .flv".into())
    }
}

fn validate_auto_remux_path(path: &str) -> Result<(), Box<dyn Error>> {
    if Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
    {
        Ok(())
    } else {
        Err("automatic remux recording requires a final .mp4 path".into())
    }
}

fn replay_save_path(recording_path: &str) -> PathBuf {
    let recording_path = Path::new(recording_path);
    let directory = recording_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    directory.join(format!(
        "Replay-{}.obsr",
        recording_stamp(SystemTime::now())
    ))
}

pub(super) fn replay_capacity_bytes(capacity_mib: u32) -> usize {
    usize::try_from(capacity_mib)
        .unwrap_or(usize::MAX / (1024 * 1024))
        .saturating_mul(1024 * 1024)
}

pub(super) fn replay_save_label(status: &ReplaySaveStatus) -> String {
    match status {
        ReplaySaveStatus::Idle => "idle".to_owned(),
        ReplaySaveStatus::Saving => "saving".to_owned(),
        ReplaySaveStatus::Saved { bytes } => format!("saved {bytes} B"),
        ReplaySaveStatus::Failed { reason } => format!("failed: {reason}"),
    }
}

/// Builds the one-profile project an output-only engine session encodes with.
pub(super) fn output_only_project(
    format: obs_rs_media::VideoFormat,
) -> Result<obs_rs_project::Project, Box<dyn Error>> {
    let mut project = obs_rs_project::Project::new("OBS-RS output session")?;
    project.add_profile(obs_rs_project::Profile::new(
        "default",
        "Default output",
        format,
    )?)?;
    Ok(project)
}
