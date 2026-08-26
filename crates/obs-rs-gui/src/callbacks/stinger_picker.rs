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
use slint::{ComponentHandle, Weak};

use crate::MainWindow;

const MAX_PROJECT_PATH_BYTES: usize = 4_096;
const MAX_PICKER_OUTPUT_BYTES: usize = MAX_PROJECT_PATH_BYTES + 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerPurpose {
    StingerResource,
    ProjectSaveAs,
    ProjectOpen,
}

impl PickerPurpose {
    fn path_limit(self) -> usize {
        match self {
            Self::StingerResource => MAX_STINGER_RESOURCE_PATH_BYTES,
            Self::ProjectSaveAs | Self::ProjectOpen => MAX_PROJECT_PATH_BYTES,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::StingerResource => "Stinger",
            Self::ProjectSaveAs | Self::ProjectOpen => "Project",
        }
    }

    fn unavailable_message(self) -> &'static str {
        match self {
            Self::StingerResource => {
                "Stinger file picker is unavailable; type the resource path manually"
            }
            Self::ProjectSaveAs => {
                "Project file picker is unavailable; type the Save As path manually"
            }
            Self::ProjectOpen => {
                "Project file picker is unavailable; type the project path manually"
            }
        }
    }

    fn already_open_message(self) -> &'static str {
        match self {
            Self::StingerResource => "Stinger file picker is already open",
            Self::ProjectSaveAs | Self::ProjectOpen => "Project file picker is already open",
        }
    }

    fn thread_name(self) -> &'static str {
        match self {
            Self::StingerResource => "obs-rs-stinger-file-picker",
            Self::ProjectSaveAs => "obs-rs-project-file-picker",
            Self::ProjectOpen => "obs-rs-project-open-file-picker",
        }
    }

    fn opening_message(self) -> &'static str {
        match self {
            Self::StingerResource => "Opening Stinger file picker…",
            Self::ProjectSaveAs | Self::ProjectOpen => "Opening project file picker…",
        }
    }

    fn cancelled_message(self) -> &'static str {
        match self {
            Self::StingerResource => "Stinger file selection cancelled",
            Self::ProjectSaveAs | Self::ProjectOpen => "Project file selection cancelled",
        }
    }

    fn selected_message(self) -> &'static str {
        match self {
            Self::StingerResource => "Stinger resource selected",
            Self::ProjectSaveAs => "Project Save As path selected",
            Self::ProjectOpen => "Project path selected",
        }
    }
}

/// Connects the scene-properties Browse button to a desktop file chooser.
///
/// The external dialog is launched on a dedicated thread. The callback only
/// checks capability, captures the current path, and returns to the event loop;
/// no process or file operation runs on the UI thread.
pub(crate) fn install_file_pickers(ui: &MainWindow) {
    let tool = detect_file_picker();
    ui.set_file_picker_enabled(tool.is_some());
    let active = Arc::new(AtomicBool::new(false));
    let weak = ui.as_weak();
    let active_for_stinger = Arc::clone(&active);
    ui.on_browse_scene_stinger(move || {
        begin_picker(
            &weak,
            &active_for_stinger,
            tool,
            PickerPurpose::StingerResource,
        );
    });

    let weak = ui.as_weak();
    let active_for_project = Arc::clone(&active);
    ui.on_browse_project_save_as(move || {
        let purpose = weak.upgrade().map_or(PickerPurpose::ProjectSaveAs, |ui| {
            if ui.get_project_dialog_mode() == 4 {
                PickerPurpose::ProjectOpen
            } else {
                PickerPurpose::ProjectSaveAs
            }
        });
        begin_picker(&weak, &active_for_project, tool, purpose);
    });
}

fn begin_picker(
    weak: &Weak<MainWindow>,
    active: &Arc<AtomicBool>,
    tool: Option<&'static str>,
    purpose: PickerPurpose,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let Some(tool) = tool else {
        ui.set_status_message(purpose.unavailable_message().into());
        return;
    };
    if active.swap(true, Ordering::AcqRel) {
        ui.set_status_message(purpose.already_open_message().into());
        return;
    }
    let start = match purpose {
        PickerPurpose::StingerResource => ui.get_scene_stinger_path().to_string(),
        PickerPurpose::ProjectSaveAs | PickerPurpose::ProjectOpen => {
            ui.get_project_path().to_string()
        }
    };
    let active_for_worker = Arc::clone(active);
    let callback_ui = weak.clone();
    let worker = thread::Builder::new()
        .name(purpose.thread_name().to_owned())
        .spawn(move || {
            let result = choose_path(tool, &start, purpose);
            active_for_worker.store(false, Ordering::Release);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = callback_ui.upgrade() else {
                    return;
                };
                match result {
                    Ok(Some(path)) => {
                        match purpose {
                            PickerPurpose::StingerResource => {
                                ui.set_scene_stinger_path(path.into());
                            }
                            PickerPurpose::ProjectSaveAs | PickerPurpose::ProjectOpen => {
                                ui.set_project_path(path.into());
                            }
                        }
                        ui.set_status_message(purpose.selected_message().into());
                    }
                    Ok(None) => ui.set_status_message(purpose.cancelled_message().into()),
                    Err(error) => ui.set_status_message(
                        format!("{} file picker failed: {error}", purpose.label()).into(),
                    ),
                }
            });
        });
    if let Err(error) = worker {
        active.store(false, Ordering::Release);
        ui.set_status_message(format!("{} file picker failed: {error}", purpose.label()).into());
    } else {
        ui.set_status_message(purpose.opening_message().into());
    }
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

fn choose_path(tool: &str, start: &str, purpose: PickerPurpose) -> Result<Option<String>, String> {
    let mut command = Command::new(tool);
    configure_command(&mut command, tool, start, purpose)?;
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
    if output.len() > purpose.path_limit() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "selected path exceeds {} bytes",
            purpose.path_limit()
        ));
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    if !status.success() {
        return Ok(None);
    }
    let path = String::from_utf8(output)
        .map_err(|_| "file picker returned a non-UTF-8 path".to_owned())?;
    validate_picker_path(path.trim(), purpose.path_limit()).map(Some)
}

fn configure_command(
    command: &mut Command,
    tool: &str,
    start: &str,
    purpose: PickerPurpose,
) -> Result<(), String> {
    match (tool, purpose) {
        ("zenity", PickerPurpose::StingerResource) => {
            command.args(["--file-selection", "--title=Select Stinger resource"]);
            if !start.is_empty() {
                command.arg(format!("--filename={start}"));
            }
        }
        ("zenity", PickerPurpose::ProjectSaveAs) => {
            command.args([
                "--file-selection",
                "--save",
                "--confirm-overwrite",
                "--title=Save OBS-RS project",
            ]);
            if !start.is_empty() {
                command.arg(format!("--filename={start}"));
            }
        }
        ("zenity", PickerPurpose::ProjectOpen) => {
            command.args(["--file-selection", "--title=Open OBS-RS project"]);
            if !start.is_empty() {
                command.arg(format!("--filename={start}"));
            }
        }
        ("kdialog", PickerPurpose::StingerResource) => {
            command.args([
                "--getopenfilename",
                if start.is_empty() { "." } else { start },
                "Video files (*.webm *.mp4 *.mkv *.mov *.avi)",
            ]);
        }
        ("kdialog", PickerPurpose::ProjectSaveAs) => {
            command.args([
                "--getsavefilename",
                if start.is_empty() {
                    "obs-rs-project.obsrproj"
                } else {
                    start
                },
                "OBS-RS projects (*.obsrproj)",
            ]);
        }
        ("kdialog", PickerPurpose::ProjectOpen) => {
            command.args([
                "--getopenfilename",
                if start.is_empty() { "." } else { start },
                "OBS-RS projects (*.obsrproj)",
            ]);
        }
        ("osascript", PickerPurpose::StingerResource) => {
            command.args([
                "-e",
                "set selectedFile to choose file with prompt \"Select Stinger resource\"\nPOSIX path of selectedFile",
            ]);
        }
        ("osascript", PickerPurpose::ProjectSaveAs) => {
            command.args([
                "-e",
                "set selectedFile to choose file name with prompt \"Save OBS-RS project as\"\ndefault name \"obs-rs-project.obsrproj\"\nPOSIX path of selectedFile",
            ]);
        }
        ("osascript", PickerPurpose::ProjectOpen) => {
            command.args([
                "-e",
                "set selectedFile to choose file with prompt \"Open OBS-RS project\"\nPOSIX path of selectedFile",
            ]);
        }
        ("powershell" | "pwsh", PickerPurpose::StingerResource) => {
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; $dialog = New-Object System.Windows.Forms.OpenFileDialog; $dialog.Filter = 'Video files|*.webm;*.mp4;*.mkv;*.mov;*.avi|All files|*.*'; if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Write($dialog.FileName) }",
            ]);
        }
        ("powershell" | "pwsh", PickerPurpose::ProjectSaveAs) => {
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; $dialog = New-Object System.Windows.Forms.SaveFileDialog; $dialog.Filter = 'OBS-RS projects|*.obsrproj|All files|*.*'; $dialog.DefaultExt = 'obsrproj'; $dialog.AddExtension = $true; if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Write($dialog.FileName) }",
            ]);
        }
        ("powershell" | "pwsh", PickerPurpose::ProjectOpen) => {
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; $dialog = New-Object System.Windows.Forms.OpenFileDialog; $dialog.Filter = 'OBS-RS projects|*.obsrproj|All files|*.*'; if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Write($dialog.FileName) }",
            ]);
        }
        _ => return Err(format!("unsupported file picker: {tool}")),
    }
    Ok(())
}

fn validate_picker_path(path: &str, max_bytes: usize) -> Result<String, String> {
    let bytes = path.len();
    if !(1..=max_bytes).contains(&bytes)
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
            validate_picker_path("assets/intro.webm", MAX_STINGER_RESOURCE_PATH_BYTES).unwrap(),
            "assets/intro.webm"
        );
        assert!(validate_picker_path("", MAX_STINGER_RESOURCE_PATH_BYTES).is_err());
        assert!(
            validate_picker_path("assets/\nintro.webm", MAX_STINGER_RESOURCE_PATH_BYTES).is_err()
        );
        assert!(validate_picker_path(
            &"x".repeat(MAX_STINGER_RESOURCE_PATH_BYTES + 1),
            MAX_STINGER_RESOURCE_PATH_BYTES,
        )
        .is_err());
        assert!(validate_picker_path(
            &"x".repeat(MAX_STINGER_RESOURCE_PATH_BYTES + 1),
            MAX_PROJECT_PATH_BYTES,
        )
        .is_ok());
    }

    #[test]
    fn unsupported_picker_tools_fail_before_spawning_a_process() {
        let mut command = Command::new("unused-picker");
        assert!(configure_command(
            &mut command,
            "unused-picker",
            "",
            PickerPurpose::StingerResource,
        )
        .is_err());
    }

    #[test]
    fn project_picker_uses_save_dialogs_on_supported_desktops() {
        let mut zenity = Command::new("zenity");
        configure_command(
            &mut zenity,
            "zenity",
            "obs-rs-project.obsrproj",
            PickerPurpose::ProjectSaveAs,
        )
        .expect("zenity save dialog");
        let zenity_args = zenity
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(zenity_args.iter().any(|arg| arg == "--save"));
        assert!(zenity_args.iter().any(|arg| arg == "--confirm-overwrite"));

        let mut kdialog = Command::new("kdialog");
        configure_command(
            &mut kdialog,
            "kdialog",
            "obs-rs-project.obsrproj",
            PickerPurpose::ProjectSaveAs,
        )
        .expect("kdialog save dialog");
        let kdialog_args = kdialog
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(kdialog_args.iter().any(|arg| arg == "--getsavefilename"));
    }

    #[test]
    fn project_picker_uses_open_dialogs_for_project_selection() {
        let mut zenity = Command::new("zenity");
        configure_command(
            &mut zenity,
            "zenity",
            "obs-rs-project.obsrproj",
            PickerPurpose::ProjectOpen,
        )
        .expect("zenity open dialog");
        let zenity_args = zenity
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(zenity_args.iter().any(|arg| arg == "--file-selection"));
        assert!(zenity_args
            .iter()
            .any(|arg| arg == "--title=Open OBS-RS project"));
        assert!(!zenity_args.iter().any(|arg| arg == "--save"));

        let mut kdialog = Command::new("kdialog");
        configure_command(
            &mut kdialog,
            "kdialog",
            "obs-rs-project.obsrproj",
            PickerPurpose::ProjectOpen,
        )
        .expect("kdialog open dialog");
        let kdialog_args = kdialog
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(kdialog_args.iter().any(|arg| arg == "--getopenfilename"));
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
