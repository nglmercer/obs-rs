use std::{
    fs,
    path::{Path, PathBuf},
};

use obs_rs_output::SegmentedRecordingPolicy;

use super::super::GStreamerError;

pub(super) fn native_error(error: impl std::fmt::Display) -> GStreamerError {
    GStreamerError::Native(error.to_string())
}

pub(super) fn recover_stale_recording_artifact(
    temp_path: Option<&Path>,
) -> Result<(), GStreamerError> {
    let Some(temp_path) = temp_path else {
        return Ok(());
    };
    remove_stale_recording_path(temp_path)
}

pub(super) fn recover_stale_segment_artifacts(
    base_path: &Path,
    policy: SegmentedRecordingPolicy,
) -> Result<(), GStreamerError> {
    for index in 1..=policy.max_segments() {
        let (_, temp_path) = segmented_recording_paths(base_path, index)?;
        remove_stale_recording_path(&temp_path)?;
    }
    Ok(())
}

pub(super) fn remove_stale_recording_path(path: &Path) -> Result<(), GStreamerError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(GStreamerError::Native(format!(
            "remove stale production recording artifact: {error}"
        ))),
    }
}

pub(super) fn segmented_recording_pattern(base_path: &Path) -> Result<PathBuf, GStreamerError> {
    let file_name = base_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| GStreamerError::InvalidEndpoint("recording path is not UTF-8".to_owned()))?;
    let stem = base_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            GStreamerError::InvalidEndpoint("recording path has no UTF-8 stem".to_owned())
        })?;
    let extension = base_path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            GStreamerError::InvalidEndpoint("recording path has no UTF-8 extension".to_owned())
        })?;
    if file_name.contains('%') {
        return Err(GStreamerError::InvalidEndpoint(
            "segmented recording path cannot contain '%'".to_owned(),
        ));
    }
    Ok(base_path.with_file_name(format!("{stem}-%05d.{extension}.part")))
}

pub(super) fn segmented_recording_paths(
    base_path: &Path,
    index: usize,
) -> Result<(PathBuf, PathBuf), GStreamerError> {
    let file_name = base_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| GStreamerError::InvalidEndpoint("recording path is not UTF-8".to_owned()))?;
    let stem = base_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            GStreamerError::InvalidEndpoint("recording path has no UTF-8 stem".to_owned())
        })?;
    let extension = base_path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            GStreamerError::InvalidEndpoint("recording path has no UTF-8 extension".to_owned())
        })?;
    if file_name.contains('%') || index == 0 || index > 99_999 {
        return Err(GStreamerError::InvalidEndpoint(
            "segmented recording path or index is invalid".to_owned(),
        ));
    }
    let stem = format!("{stem}-{index:05}.{extension}");
    let final_path = base_path.with_file_name(&stem);
    let temp_path = base_path.with_file_name(format!("{stem}.part"));
    Ok((final_path, temp_path))
}

pub(super) fn publish_segmented_recording(
    base_path: &Path,
    policy: SegmentedRecordingPolicy,
) -> Result<usize, GStreamerError> {
    let mut published = 0_usize;
    let mut total_bytes = 0_usize;
    for index in 1..=policy.max_segments() {
        let (final_path, temp_path) = segmented_recording_paths(base_path, index)?;
        match fs::metadata(&temp_path) {
            Ok(_) => {
                let bytes = publish_recording_artifact(&temp_path, &final_path)?;
                published = published.saturating_add(1);
                total_bytes = total_bytes.checked_add(bytes).ok_or_else(|| {
                    GStreamerError::Native(
                        "published production segment size exceeds platform limits".to_owned(),
                    )
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(GStreamerError::Native(format!(
                    "inspect production recording segment: {error}"
                )))
            }
        }
    }
    if published == 0 {
        return Err(GStreamerError::Native(
            "production segment muxer produced no recording artifacts".to_owned(),
        ));
    }
    Ok(total_bytes)
}

pub(super) fn publish_recording_artifact(
    temp: &Path,
    final_path: &Path,
) -> Result<usize, GStreamerError> {
    let bytes = fs::metadata(temp)
        .map_err(|error| GStreamerError::Native(format!("inspect production recording: {error}")))?
        .len();
    let bytes = usize::try_from(bytes).map_err(|_| {
        GStreamerError::Native("production recording size exceeds platform limits".to_owned())
    })?;
    if bytes == 0 {
        return Err(GStreamerError::Native(
            "refusing to publish an empty production recording".to_owned(),
        ));
    }
    fs::hard_link(temp, final_path).map_err(|error| {
        GStreamerError::Native(format!(
            "publish production recording without replacing an existing file: {error}"
        ))
    })?;
    fs::remove_file(temp).map_err(|error| {
        GStreamerError::Native(format!(
            "remove published production recording temporary path: {error}"
        ))
    })?;
    Ok(bytes)
}
