//! Asynchronous, bounded file selection for scene-owned Stinger resources.

use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

use obs_rs_media::MAX_STINGER_RESOURCE_PATH_BYTES;
use slint::ComponentHandle;

use crate::MainWindow;

const MAX_PICKER_OUTPUT_BYTES: usize = MAX_STINGER_RESOURCE_PATH_BYTES + 1;

/// Connects the scene-properties Browse button to a desktop file chooser.
///
/// The external dialog is launched on a dedicated thread. The callback only
/// checks capability, captures the current path, and returns to the event loop;
/// no process or file operation runs on the UI thread.
pub(crate) fn install_stinger_file_picker(ui: &MainWindow) {
    let tool = detect_file_picker();
    ui.set_scene_stinger_picker_enabled(tool.is_some());
    let active = Arc::new(AtomicBool::new(false));
    let weak = ui.as_weak();
    ui.on_browse_scene_stinger(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(tool) = tool else {
            ui.set_status_message(
                "Stinger file picker is unavailable; type the resource path manually".into(),
            );
            return;
        };
        if active.swap(true, Ordering::AcqRel) {
            ui.set_status_message("Stinger file picker is already open".into());
            return;
        }
        let start = ui.get_scene_stinger_path().to_string();
        let active_for_worker = Arc::clone(&active);
        let callback_ui = weak.clone();
        let worker = thread::Builder::new()
            .name("obs-rs-stinger-file-picker".to_owned())
            .spawn(move || {
                let result = choose_file(tool, &start);
                active_for_worker.store(false, Ordering::Release);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = callback_ui.upgrade() else {
                        return;
                    };
                    match result {
                        Ok(Some(path)) => {
                            ui.set_scene_stinger_path(path.into());
                            ui.set_status_message("Stinger resource selected".into());
                        }
                        Ok(None) => {
                            ui.set_status_message("Stinger file selection cancelled".into());
                        }
                        Err(error) => ui.set_status_message(
                            format!("Stinger file picker failed: {error}").into(),
                        ),
                    }
                });
            });
        if let Err(error) = worker {
            active.store(false, Ordering::Release);
            ui.set_status_message(format!("Stinger file picker failed: {error}").into());
        } else {
            ui.set_status_message("Opening Stinger file picker…".into());
        }
    });
}

/// Returns the first supported chooser available on the current desktop.
pub(crate) fn detect_file_picker() -> Option<&'static str> {
    file_picker_tools()
        .iter()
        .copied()
        .find(|tool| command_exists(tool))
}

fn file_picker_tools() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    {
        &["zenity", "kdialog"]
    }
    #[cfg(target_os = "macos")]
    {
        &["osascript"]
    }
    #[cfg(target_os = "windows")]
    {
        &["powershell", "pwsh"]
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        &[]
    }
}

fn command_exists(tool: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| {
        directory.join(tool).is_file()
            || cfg!(target_os = "windows") && directory.join(format!("{tool}.exe")).is_file()
    })
}

fn choose_file(tool: &str, start: &str) -> Result<Option<String>, String> {
    let mut command = Command::new(tool);
    configure_command(&mut command, tool, start)?;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let mut output = Vec::with_capacity(MAX_PICKER_OUTPUT_BYTES);
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("file picker did not provide stdout".to_owned());
    };
    if let Err(error) = stdout
        .take(u64::try_from(MAX_PICKER_OUTPUT_BYTES).unwrap_or(u64::MAX))
        .read_to_end(&mut output)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.to_string());
    }
    if output.len() > MAX_STINGER_RESOURCE_PATH_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "selected path exceeds {MAX_STINGER_RESOURCE_PATH_BYTES} bytes"
        ));
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    if !status.success() {
        return Ok(None);
    }
    let path = String::from_utf8(output)
        .map_err(|_| "file picker returned a non-UTF-8 path".to_owned())?;
    validate_picker_path(path.trim()).map(Some)
}

fn configure_command(command: &mut Command, tool: &str, start: &str) -> Result<(), String> {
    match tool {
        "zenity" => {
            command.args(["--file-selection", "--title=Select Stinger resource"]);
            if !start.is_empty() {
                command.arg(format!("--filename={start}"));
            }
        }
        "kdialog" => {
            command.args([
                "--getopenfilename",
                if start.is_empty() { "." } else { start },
                "Video files (*.webm *.mp4 *.mkv *.mov *.avi)",
            ]);
        }
        "osascript" => {
            command.args([
                "-e",
                "set selectedFile to choose file with prompt \"Select Stinger resource\"\nPOSIX path of selectedFile",
            ]);
        }
        "powershell" | "pwsh" => {
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; $dialog = New-Object System.Windows.Forms.OpenFileDialog; $dialog.Filter = 'Video files|*.webm;*.mp4;*.mkv;*.mov;*.avi|All files|*.*'; if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Write($dialog.FileName) }",
            ]);
        }
        _ => return Err(format!("unsupported file picker: {tool}")),
    }
    Ok(())
}

fn validate_picker_path(path: &str) -> Result<String, String> {
    let bytes = path.len();
    if !(1..=MAX_STINGER_RESOURCE_PATH_BYTES).contains(&bytes)
        || path.chars().any(char::is_control)
        || Path::new(path).components().next().is_none()
    {
        return Err("selected path is empty, unsafe, or too long".to_owned());
    }
    Ok(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_path_validation_keeps_bounded_utf8_paths() {
        assert_eq!(
            validate_picker_path("assets/intro.webm").unwrap(),
            "assets/intro.webm"
        );
        assert!(validate_picker_path("").is_err());
        assert!(validate_picker_path("assets/\nintro.webm").is_err());
        assert!(validate_picker_path(&"x".repeat(MAX_STINGER_RESOURCE_PATH_BYTES + 1)).is_err());
    }

    #[test]
    fn unsupported_picker_tools_fail_before_spawning_a_process() {
        let mut command = Command::new("unused-picker");
        assert!(configure_command(&mut command, "unused-picker", "").is_err());
    }

    #[test]
    fn picker_activity_flag_is_shareable_between_ui_and_worker() {
        let active = Arc::new(AtomicBool::new(false));
        assert!(!active.swap(true, Ordering::AcqRel));
        assert!(active.swap(true, Ordering::AcqRel));
        active.store(false, Ordering::Release);
        assert!(!active.load(Ordering::Acquire));
    }
}
