use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use gstreamer as gst;
use gstreamer::prelude::*;
use obs_rs_util::Json;

use super::super::GStreamerError;
use super::{
    native_error, publish_recording_artifact, recover_stale_recording_artifact,
    remove_stale_recording_path, RemuxRecovery, MAX_REMUX_MANIFEST_BYTES,
    MAX_REMUX_RECOVERY_CANDIDATES, MAX_REMUX_RECOVERY_DIRECTORY_ENTRIES, REMUX_MANIFEST_FORMAT,
};

/// Finds recoverable automatic-remux destinations in a bounded directory scan.
///
/// A native remux keeps its source at <final>.mkv.part and writes a matching
/// bounded sidecar until the MP4 is published. This function returns the
/// corresponding final .mp4 paths for non-empty, marked sources whose
/// destination does not already exist. It performs no media work and is
/// intended for the engine control plane, never a UI or real-time callback.
///
/// # Errors
///
/// Returns a typed filesystem error when the directory cannot be inspected or
/// the bounded scan would be exceeded.
pub fn discover_interrupted_remux_candidates(
    directory: impl AsRef<Path>,
) -> Result<Vec<PathBuf>, GStreamerError> {
    let directory = directory.as_ref();
    if !directory.exists() {
        return Ok(Vec::new());
    }
    if !directory.is_dir() {
        return Err(GStreamerError::InvalidEndpoint(
            "remux candidate path must be a directory".to_owned(),
        ));
    }
    let entries = fs::read_dir(directory).map_err(|error| {
        GStreamerError::Native(format!("inspect remux candidate directory: {error}"))
    })?;
    let mut candidates = Vec::new();
    for (index, entry) in entries
        .take(MAX_REMUX_RECOVERY_DIRECTORY_ENTRIES + 1)
        .enumerate()
    {
        if index == MAX_REMUX_RECOVERY_DIRECTORY_ENTRIES {
            return Err(GStreamerError::Native(format!(
                "remux candidate directory exceeds {MAX_REMUX_RECOVERY_DIRECTORY_ENTRIES} entries"
            )));
        }
        let entry = entry.map_err(|error| {
            GStreamerError::Native(format!("read remux candidate entry: {error}"))
        })?;
        if !entry
            .file_type()
            .map_err(|error| {
                GStreamerError::Native(format!("inspect remux candidate entry: {error}"))
            })?
            .is_file()
        {
            continue;
        }
        let source_path = entry.path();
        let Some(final_path) = remux_final_path_from_source(&source_path) else {
            continue;
        };
        let source_bytes = entry.metadata().map_err(|error| {
            GStreamerError::Native(format!("inspect remux candidate metadata: {error}"))
        })?;
        if source_bytes.len() == 0 || final_path.exists() {
            continue;
        }
        if !remux_manifest_matches(&source_path, &final_path)? {
            continue;
        }
        candidates.push(final_path);
        if candidates.len() > MAX_REMUX_RECOVERY_CANDIDATES {
            return Err(GStreamerError::Native(format!(
                "remux candidate directory contains more than {MAX_REMUX_RECOVERY_CANDIDATES} recoverable files"
            )));
        }
    }
    candidates.sort_unstable();
    Ok(candidates)
}

/// Writes the durable marker for an automatic remux recording.
///
/// The manifest is a bounded, atomically published JSON sidecar. Its
/// file-name-only payload lets recovery distinguish an automatic-remux
/// Matroska source from an ordinary `.mkv.part` recording without exposing
/// native paths or trusting an arbitrary manifest to choose a destination.
///
/// # Errors
///
/// Returns a typed filesystem or endpoint error when the final path is not an
/// MP4 path or the sidecar cannot be published.
pub fn write_interrupted_remux_manifest(
    final_path: impl AsRef<Path>,
) -> Result<(), GStreamerError> {
    let final_path = final_path.as_ref();
    if !final_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
    {
        return Err(GStreamerError::InvalidEndpoint(
            "remux manifest destination must use the .mp4 extension".to_owned(),
        ));
    }
    let source_path = final_path.with_extension("mkv.part");
    let source_name = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| GStreamerError::InvalidEndpoint("remux source is not UTF-8".to_owned()))?;
    let destination_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            GStreamerError::InvalidEndpoint("remux destination is not UTF-8".to_owned())
        })?;
    let document = Json::object([
        ("destination", Json::string(destination_name)),
        ("format", Json::string(REMUX_MANIFEST_FORMAT)),
        ("source", Json::string(source_name)),
    ])
    .to_pretty_string();
    if document.len() > MAX_REMUX_MANIFEST_BYTES {
        return Err(GStreamerError::InvalidEndpoint(
            "remux manifest exceeds its byte limit".to_owned(),
        ));
    }
    recover_stale_remux_manifest(final_path)?;
    let manifest_path = remux_manifest_path(final_path);
    let temporary_path = manifest_path.with_extension("json.part");
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .map_err(|error| {
                GStreamerError::Native(format!("create remux manifest temporary path: {error}"))
            })?;
        file.write_all(document.as_bytes())
            .map_err(|error| GStreamerError::Native(format!("write remux manifest: {error}")))?;
        file.sync_all()
            .map_err(|error| GStreamerError::Native(format!("sync remux manifest: {error}")))?;
        fs::hard_link(&temporary_path, &manifest_path).map_err(|error| {
            GStreamerError::Native(format!(
                "publish remux manifest without replacing an existing file: {error}"
            ))
        })?;
        fs::remove_file(&temporary_path).map_err(|error| {
            GStreamerError::Native(format!("remove remux manifest temporary path: {error}"))
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

pub(super) fn remux_manifest_path(final_path: &Path) -> PathBuf {
    final_path.with_extension("remux.json")
}

pub(super) fn recover_stale_remux_manifest(final_path: &Path) -> Result<(), GStreamerError> {
    let manifest_path = remux_manifest_path(final_path);
    remove_stale_recording_path(&manifest_path)?;
    remove_stale_recording_path(&manifest_path.with_extension("json.part"))
}

pub(super) fn remux_manifest_matches(
    source_path: &Path,
    final_path: &Path,
) -> Result<bool, GStreamerError> {
    let manifest_path = remux_manifest_path(final_path);
    let file = match fs::File::open(&manifest_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(GStreamerError::Native(format!(
                "inspect remux manifest: {error}"
            )))
        }
    };
    let mut document = Vec::with_capacity(MAX_REMUX_MANIFEST_BYTES);
    file.take((MAX_REMUX_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut document)
        .map_err(|error| GStreamerError::Native(format!("read remux manifest: {error}")))?;
    if document.len() > MAX_REMUX_MANIFEST_BYTES {
        return Ok(false);
    }
    let Ok(document) = String::from_utf8(document) else {
        return Ok(false);
    };
    let Ok(value) = Json::parse(&document) else {
        return Ok(false);
    };
    let Some(format) = value.get("format").and_then(Json::as_str) else {
        return Ok(false);
    };
    let Some(source) = value.get("source").and_then(Json::as_str) else {
        return Ok(false);
    };
    let Some(destination) = value.get("destination").and_then(Json::as_str) else {
        return Ok(false);
    };
    Ok(format == REMUX_MANIFEST_FORMAT
        && source_path.file_name().and_then(|name| name.to_str()) == Some(source)
        && final_path.file_name().and_then(|name| name.to_str()) == Some(destination))
}

pub(super) fn remux_final_path_from_source(source_path: &Path) -> Option<PathBuf> {
    let file_name = source_path.file_name()?.to_str()?;
    let suffix = ".mkv.part";
    if file_name.len() <= suffix.len()
        || !file_name[file_name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    {
        return None;
    }
    let stem = &file_name[..file_name.len() - suffix.len()];
    (!stem.is_empty()).then(|| {
        source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("{stem}.mp4"))
    })
}

/// Remuxes the native H.264/AAC Matroska recording into MP4 without decoding
/// or re-encoding either stream.
///
/// The operation is intentionally a control-plane function. It streams through
/// bounded `GStreamer` elements, writes a hidden destination, waits for EOS, and
/// publishes the destination only after a non-empty file exists. The caller
/// must keep it off the UI, capture, audio, and render threads.
///
/// # Errors
///
/// Returns a typed endpoint, `GStreamer`, or filesystem error when the source
/// is invalid, the pipeline fails, or the destination cannot be published.
#[allow(
    clippy::too_many_lines,
    reason = "native remux setup and teardown must keep the bounded pipeline lifecycle together"
)]
pub fn remux_matroska_to_mp4(
    source_path: impl AsRef<Path>,
    final_path: impl Into<PathBuf>,
) -> Result<usize, GStreamerError> {
    gst::init().map_err(native_error)?;
    let source_path = source_path.as_ref();
    let final_path = final_path.into();
    if source_path == final_path {
        return Err(GStreamerError::InvalidEndpoint(
            "remux source and destination must differ".to_owned(),
        ));
    }
    let source_name = source_path.file_name().and_then(|name| name.to_str());
    let is_hidden_mkv_source =
        source_name.is_some_and(|name| name.to_ascii_lowercase().ends_with(".mkv.part"));
    if !source_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mkv"))
        && !is_hidden_mkv_source
    {
        return Err(GStreamerError::InvalidEndpoint(
            "remux source must use the .mkv or .mkv.part extension".to_owned(),
        ));
    }
    if !final_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
    {
        return Err(GStreamerError::InvalidEndpoint(
            "remux destination must use the .mp4 extension".to_owned(),
        ));
    }
    let source = source_path
        .to_str()
        .ok_or_else(|| GStreamerError::InvalidEndpoint("remux source is not UTF-8".to_owned()))?;
    let source_bytes = fs::metadata(source_path)
        .map_err(|error| GStreamerError::Native(format!("inspect remux source: {error}")))?
        .len();
    if source_bytes == 0 {
        return Err(GStreamerError::Native(
            "refusing to remux an empty Matroska source".to_owned(),
        ));
    }
    let temporary_path = final_path.with_extension("mp4.part");
    recover_stale_recording_artifact(Some(&temporary_path))?;
    let temporary = temporary_path.to_str().ok_or_else(|| {
        GStreamerError::InvalidEndpoint("remux destination is not UTF-8".to_owned())
    })?;

    let source_element = gst::ElementFactory::make("filesrc")
        .property("location", source)
        .build()
        .map_err(native_error)?;
    let demuxer = gst::ElementFactory::make("matroskademux")
        .build()
        .map_err(native_error)?;
    let video_queue = gst::ElementFactory::make("queue")
        .property("max-size-buffers", 16_u32)
        .property("max-size-bytes", 4_194_304_u32)
        .build()
        .map_err(native_error)?;
    let audio_queue = gst::ElementFactory::make("queue")
        .property("max-size-buffers", 16_u32)
        .property("max-size-bytes", 4_194_304_u32)
        .build()
        .map_err(native_error)?;
    let video_parser = gst::ElementFactory::make("h264parse")
        .build()
        .map_err(native_error)?;
    let audio_parser = gst::ElementFactory::make("aacparse")
        .build()
        .map_err(native_error)?;
    let muxer = gst::ElementFactory::make("mp4mux")
        .property("faststart", true)
        .build()
        .map_err(native_error)?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", temporary)
        .build()
        .map_err(native_error)?;
    let pipeline = gst::Pipeline::new();
    pipeline
        .add_many([
            &source_element,
            &demuxer,
            &video_queue,
            &audio_queue,
            &video_parser,
            &audio_parser,
            &muxer,
            &sink,
        ])
        .map_err(native_error)?;
    gst::Element::link_many([&source_element, &demuxer]).map_err(native_error)?;
    gst::Element::link_many([&video_queue, &video_parser]).map_err(native_error)?;
    gst::Element::link_many([&audio_queue, &audio_parser]).map_err(native_error)?;

    let video_mux_pad = muxer
        .request_pad_simple("video_%u")
        .ok_or_else(|| GStreamerError::Native("MP4 video pad is unavailable".to_owned()))?;
    let audio_mux_pad = muxer
        .request_pad_simple("audio_%u")
        .ok_or_else(|| GStreamerError::Native("MP4 audio pad is unavailable".to_owned()))?;
    video_parser
        .static_pad("src")
        .ok_or_else(|| GStreamerError::Native("H.264 parser source pad is unavailable".to_owned()))?
        .link(&video_mux_pad)
        .map_err(native_error)?;
    audio_parser
        .static_pad("src")
        .ok_or_else(|| GStreamerError::Native("AAC parser source pad is unavailable".to_owned()))?
        .link(&audio_mux_pad)
        .map_err(native_error)?;
    gst::Element::link_many([&muxer, &sink]).map_err(native_error)?;

    let link_error = Arc::new(Mutex::new(None::<String>));
    let link_error_for_callback = Arc::clone(&link_error);
    let video_sink = video_queue.static_pad("sink").ok_or_else(|| {
        GStreamerError::Native("remux video queue sink pad is unavailable".to_owned())
    })?;
    let audio_sink = audio_queue.static_pad("sink").ok_or_else(|| {
        GStreamerError::Native("remux audio queue sink pad is unavailable".to_owned())
    })?;
    demuxer.connect_pad_added(move |_demuxer, source_pad| {
        let caps = source_pad
            .current_caps()
            .unwrap_or_else(|| source_pad.query_caps(None));
        let Some(structure) = caps.structure(0) else {
            return;
        };
        let sink_pad = match structure.name().as_str() {
            "video/x-h264" if !video_sink.is_linked() => &video_sink,
            "audio/mpeg" if !audio_sink.is_linked() => &audio_sink,
            _ => return,
        };
        if let Err(error) = source_pad.link(sink_pad) {
            if let Ok(mut link_error) = link_error_for_callback.lock() {
                *link_error = Some(error.to_string());
            }
        }
    });

    pipeline
        .set_state(gst::State::Playing)
        .map_err(native_error)?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| GStreamerError::Native("remux pipeline has no bus".to_owned()))?;
    let message = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(60 * 60),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    let pipeline_result = match message.as_ref().map(|message| message.view()) {
        Some(gst::MessageView::Eos(_)) => Ok(()),
        Some(gst::MessageView::Error(error)) => Err(GStreamerError::Native(format!(
            "remux pipeline reported an error: {}",
            error.error()
        ))),
        _ => Err(GStreamerError::Native(
            "remux pipeline timed out".to_owned(),
        )),
    };
    let state_result = pipeline.set_state(gst::State::Null).map_err(native_error);
    if let Err(error) = pipeline_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    state_result?;
    if let Ok(link_error) = link_error.lock() {
        if let Some(error) = link_error.as_ref() {
            let _ = fs::remove_file(&temporary_path);
            return Err(GStreamerError::Native(format!(
                "remux stream link failed: {error}"
            )));
        }
    }
    match publish_recording_artifact(&temporary_path, &final_path) {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            let _ = fs::remove_file(&temporary_path);
            Err(error)
        }
    }
}

/// Recovers an interrupted automatic remux without replacing an existing MP4.
///
/// Automatic remux recordings keep their Matroska source at the exact hidden
/// path `<final>.mkv.part` with a matching durable manifest until publication
/// succeeds. Startup cleanup removes those paths only when a new recording
/// session claims the destination; this explicit operation lets a control-plane
/// caller recover it first.
///
/// The operation remains bounded by the native remux timeout and must stay off
/// UI, capture, audio, and render threads.
///
/// # Errors
///
/// Returns a typed endpoint, filesystem, or native pipeline error. A missing
/// hidden source is an ordinary [`RemuxRecovery::NoCandidate`] result.
pub fn recover_interrupted_remux_recording(
    final_path: impl AsRef<Path>,
) -> Result<RemuxRecovery, GStreamerError> {
    gst::init().map_err(native_error)?;
    let final_path = final_path.as_ref();
    if !final_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
    {
        return Err(GStreamerError::InvalidEndpoint(
            "interrupted remux recovery requires a final .mp4 path".to_owned(),
        ));
    }
    if final_path.exists() {
        return Err(GStreamerError::Native(
            "refusing to recover over an existing MP4 destination".to_owned(),
        ));
    }
    let source_path = final_path.with_extension("mkv.part");
    if !remux_manifest_matches(&source_path, final_path)? {
        return Ok(RemuxRecovery::NoCandidate);
    }
    let source_bytes = match fs::metadata(&source_path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RemuxRecovery::NoCandidate);
        }
        Err(error) => {
            return Err(GStreamerError::Native(format!(
                "inspect interrupted remux source: {error}"
            )));
        }
    };
    if source_bytes == 0 {
        return Err(GStreamerError::Native(
            "refusing to recover an empty Matroska source".to_owned(),
        ));
    }
    let bytes = remux_matroska_to_mp4(&source_path, final_path)?;
    fs::remove_file(&source_path).map_err(|error| {
        GStreamerError::Native(format!("remove recovered Matroska source: {error}"))
    })?;
    recover_stale_remux_manifest(final_path)?;
    Ok(RemuxRecovery::Recovered { bytes })
}
