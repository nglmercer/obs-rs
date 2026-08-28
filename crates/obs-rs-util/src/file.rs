//! Small filesystem operations whose behavior differs across host platforms.

#[cfg(any(target_os = "windows", test))]
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, io, path::Path};

/// Publishes a fully written temporary file at `final_path`.
///
/// Unix `rename` replaces an existing file atomically. Windows does not permit
/// that replacement through `std::fs::rename`, so the Windows path moves the
/// old file aside, publishes the new one, and restores the old file if the
/// second move fails. The backup is removed after a successful publish.
///
/// # Errors
///
/// Returns the filesystem error from the publish or restoration operation.
pub fn replace_file(temporary: &Path, final_path: &Path) -> io::Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        fs::rename(temporary, final_path)
    }

    #[cfg(target_os = "windows")]
    {
        match fs::rename(temporary, final_path) {
            Ok(()) => Ok(()),
            Err(first_error)
                if final_path.exists()
                    && matches!(
                        first_error.kind(),
                        io::ErrorKind::AlreadyExists | io::ErrorKind::PermissionDenied
                    ) =>
            {
                let backup = backup_path(final_path)?;
                fs::rename(final_path, &backup)?;
                match fs::rename(temporary, final_path) {
                    Ok(()) => {
                        let _ = fs::remove_file(backup);
                        Ok(())
                    }
                    Err(publish_error) => {
                        let restore_error = fs::rename(&backup, final_path).err();
                        Err(restore_error.unwrap_or(publish_error))
                    }
                }
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_os = "windows")]
fn backup_path(final_path: &Path) -> io::Result<std::path::PathBuf> {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("file replacement target must name a file"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    Ok(final_path.with_file_name(format!(
        ".{file_name}.obs-rs-old-{}-{nonce}",
        std::process::id()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_publishes_new_contents() {
        let root = std::env::temp_dir().join(format!(
            "obs-rs-util-replace-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("temporary directory");
        let final_path = root.join("settings.toml");
        let temporary = root.join("settings.toml.tmp");
        fs::write(&final_path, "old").expect("old file");
        fs::write(&temporary, "new").expect("temporary file");

        replace_file(&temporary, &final_path).expect("publish replacement");

        assert_eq!(
            fs::read_to_string(&final_path).expect("published file"),
            "new"
        );
        assert!(!temporary.exists());
        let _ = fs::remove_dir_all(root);
    }
}
