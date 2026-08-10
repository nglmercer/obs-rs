use std::error::Error;

use obs_rs_core::CompositorMetrics;
use obs_rs_diagnostics::{AtomicDiagnosticFileWriter, DiagnosticBundle};
use obs_rs_media::VideoFormat;
use obs_rs_video::VideoMetrics;

use crate::fixtures::project_fixture;

pub(crate) fn project_diagnostics_fixture(
    format: VideoFormat,
    compositor_metrics: CompositorMetrics,
    video_metrics: VideoMetrics,
    checksum: u64,
) -> Result<(usize, usize, usize, usize), Box<dyn Error>> {
    let (project_bytes, project_profiles, ui_snapshot_bytes, bundle) =
        project_fixture(format, compositor_metrics, video_metrics, checksum)?;
    let diagnostic_bytes = diagnostic_file_fixture(&bundle)?;
    Ok((
        project_bytes,
        project_profiles,
        ui_snapshot_bytes,
        diagnostic_bytes,
    ))
}

pub(crate) fn diagnostic_file_fixture(bundle: &DiagnosticBundle) -> Result<usize, Box<dyn Error>> {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let root = std::env::temp_dir();
    let final_path = root.join(format!("obs-rs-demo-{}-{token}.diag", std::process::id()));
    let temp_path = root.join(format!(
        "obs-rs-demo-{}-{token}.diag.part",
        std::process::id()
    ));
    let mut writer = AtomicDiagnosticFileWriter::new(&final_path, &temp_path)?;
    let committed = writer.finalize(bundle)?;
    let persisted = std::fs::read(writer.final_path())?;
    let restored = DiagnosticBundle::decode(&persisted)?;
    if &restored != bundle || persisted.len() != committed {
        return Err("diagnostic bundle changed after atomic commit".into());
    }
    std::fs::remove_file(writer.final_path())?;
    Ok(committed)
}
