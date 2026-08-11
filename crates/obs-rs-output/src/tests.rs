use obs_rs_media::{FrameRate, VideoFormat};

pub(super) fn format() -> VideoFormat {
    VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("valid rate")).expect("valid format")
}

pub(super) fn unique_paths(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    let root = std::env::temp_dir();
    (
        root.join(format!("obs-rs-{label}-{token}.obsrraw")),
        root.join(format!("obs-rs-{label}-{token}.part")),
    )
}

mod codecs;
mod profile;
mod recording;
mod stream;
mod writers;
