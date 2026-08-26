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
    CollectionExport,
    CollectionImport,
}

impl PickerPurpose {
    fn path_limit(self) -> usize {
        match self {
            Self::StingerResource => MAX_STINGER_RESOURCE_PATH_BYTES,
            Self::ProjectSaveAs
            | Self::ProjectOpen
            | Self::CollectionExport
            | Self::CollectionImport => MAX_PROJECT_PATH_BYTES,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::StingerResource => "Stinger",
            Self::ProjectSaveAs | Self::ProjectOpen => "Project",
            Self::CollectionExport | Self::CollectionImport => "Collection",
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
            Self::CollectionExport => {
                "Collection file picker is unavailable; type the export path manually"
            }
            Self::CollectionImport => {
                "Collection file picker is unavailable; type the import path manually"
            }
        }
    }

    fn already_open_message(self) -> &'static str {
        match self {
            Self::StingerResource => "Stinger file picker is already open",
            Self::ProjectSaveAs | Self::ProjectOpen => "Project file picker is already open",
            Self::CollectionExport | Self::CollectionImport => {
                "Collection file picker is already open"
            }
        }
    }

    fn thread_name(self) -> &'static str {
        match self {
            Self::StingerResource => "obs-rs-stinger-file-picker",
            Self::ProjectSaveAs => "obs-rs-project-file-picker",
            Self::ProjectOpen => "obs-rs-project-open-file-picker",
            Self::CollectionExport => "obs-rs-collection-export-file-picker",
            Self::CollectionImport => "obs-rs-collection-import-file-picker",
        }
    }

    fn opening_message(self) -> &'static str {
        match self {
            Self::StingerResource => "Opening Stinger file picker…",
            Self::ProjectSaveAs | Self::ProjectOpen => "Opening project file picker…",
            Self::CollectionExport | Self::CollectionImport => "Opening collection file picker…",
        }
    }

    fn cancelled_message(self) -> &'static str {
        match self {
            Self::StingerResource => "Stinger file selection cancelled",
            Self::ProjectSaveAs | Self::ProjectOpen => "Project file selection cancelled",
            Self::CollectionExport | Self::CollectionImport => {
                "Collection file selection cancelled"
            }
        }
    }

    fn selected_message(self) -> &'static str {
        match self {
            Self::StingerResource => "Stinger resource selected",
            Self::ProjectSaveAs => "Project Save As path selected",
            Self::ProjectOpen => "Project path selected",
            Self::CollectionExport => "Collection export path selected",
            Self::CollectionImport => "Collection import path selected",
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
            project_picker_purpose(ui.get_project_dialog_mode())
        });
        begin_picker(&weak, &active_for_project, tool, purpose);
    });
}

fn project_picker_purpose(mode: i32) -> PickerPurpose {
    match mode {
        1 => PickerPurpose::CollectionExport,
        2 => PickerPurpose::CollectionImport,
        4 => PickerPurpose::ProjectOpen,
        _ => PickerPurpose::ProjectSaveAs,
    }
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
        PickerPurpose::CollectionExport | PickerPurpose::CollectionImport => {
            ui.get_collection_transfer_path().to_string()
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
                            PickerPurpose::CollectionExport | PickerPurpose::CollectionImport => {
                                ui.set_collection_transfer_path(path.into());
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
    match tool {
        "zenity" => {
            configure_zenity(command, start, purpose);
            Ok(())
        }
        "kdialog" => {
            configure_kdialog(command, start, purpose);
            Ok(())
        }
        "osascript" => {
            configure_osascript(command, purpose);
            Ok(())
        }
        "powershell" | "pwsh" => {
            configure_powershell(command, purpose);
            Ok(())
        }
        _ => Err(format!("unsupported file picker: {tool}")),
    }
}

fn configure_zenity(command: &mut Command, start: &str, purpose: PickerPurpose) {
    let (title, save) = match purpose {
        PickerPurpose::StingerResource => ("Select Stinger resource", false),
        PickerPurpose::ProjectSaveAs => ("Save OBS-RS project", true),
        PickerPurpose::ProjectOpen => ("Open OBS-RS project", false),
        PickerPurpose::CollectionExport => ("Export OBS-RS collection", true),
        PickerPurpose::CollectionImport => ("Import OBS-RS collection", false),
    };
    command.arg("--file-selection");
    if save {
        command.args(["--save", "--confirm-overwrite"]);
    }
    command.arg(format!("--title={title}"));
    if !start.is_empty() {
        command.arg(format!("--filename={start}"));
    }
}

fn configure_kdialog(command: &mut Command, start: &str, purpose: PickerPurpose) {
    let (save, default_name, filter) = match purpose {
        PickerPurpose::StingerResource => {
            (false, ".", "Video files (*.webm *.mp4 *.mkv *.mov *.avi)")
        }
        PickerPurpose::ProjectSaveAs => (
            true,
            "obs-rs-project.obsrproj",
            "OBS-RS projects (*.obsrproj)",
        ),
        PickerPurpose::ProjectOpen => (false, ".", "OBS-RS projects (*.obsrproj)"),
        PickerPurpose::CollectionExport => (
            true,
            "obs-rs-collection-export.obsrproj",
            "OBS-RS collections (*.obsrproj)",
        ),
        PickerPurpose::CollectionImport => (false, ".", "OBS-RS collections (*.obsrproj)"),
    };
    command.arg(if save {
        "--getsavefilename"
    } else {
        "--getopenfilename"
    });
    command.args([
        if start.is_empty() {
            default_name
        } else {
            start
        },
        filter,
    ]);
}

fn configure_osascript(command: &mut Command, purpose: PickerPurpose) {
    let script = match purpose {
        PickerPurpose::StingerResource => {
            "set selectedFile to choose file with prompt \"Select Stinger resource\"\nPOSIX path of selectedFile"
        }
        PickerPurpose::ProjectSaveAs => {
            "set selectedFile to choose file name with prompt \"Save OBS-RS project as\"\ndefault name \"obs-rs-project.obsrproj\"\nPOSIX path of selectedFile"
        }
        PickerPurpose::ProjectOpen => {
            "set selectedFile to choose file with prompt \"Open OBS-RS project\"\nPOSIX path of selectedFile"
        }
        PickerPurpose::CollectionExport => {
            "set selectedFile to choose file name with prompt \"Export OBS-RS collection as\"\ndefault name \"obs-rs-collection-export.obsrproj\"\nPOSIX path of selectedFile"
        }
        PickerPurpose::CollectionImport => {
            "set selectedFile to choose file with prompt \"Import OBS-RS collection\"\nPOSIX path of selectedFile"
        }
    };
    command.args(["-e", script]);
}

fn configure_powershell(command: &mut Command, purpose: PickerPurpose) {
    let (dialog, filter, save) = match purpose {
        PickerPurpose::StingerResource => (
            "OpenFileDialog",
            "Video files|*.webm;*.mp4;*.mkv;*.mov;*.avi|All files|*.*",
            false,
        ),
        PickerPurpose::ProjectSaveAs => (
            "SaveFileDialog",
            "OBS-RS projects|*.obsrproj|All files|*.*",
            true,
        ),
        PickerPurpose::ProjectOpen => (
            "OpenFileDialog",
            "OBS-RS projects|*.obsrproj|All files|*.*",
            false,
        ),
        PickerPurpose::CollectionExport => (
            "SaveFileDialog",
            "OBS-RS collections|*.obsrproj|All files|*.*",
            true,
        ),
        PickerPurpose::CollectionImport => (
            "OpenFileDialog",
            "OBS-RS collections|*.obsrproj|All files|*.*",
            false,
        ),
    };
    let extension = if save {
        "; $dialog.DefaultExt = 'obsrproj'; $dialog.AddExtension = $true"
    } else {
        ""
    };
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; $dialog = New-Object System.Windows.Forms.{dialog}; $dialog.Filter = '{filter}'{extension}; if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ [Console]::Write($dialog.FileName) }}"
    );
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
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
    fn collection_picker_uses_save_and_open_dialogs() {
        let mut zenity = Command::new("zenity");
        configure_command(
            &mut zenity,
            "zenity",
            "obs-rs-collection.obsrproj",
            PickerPurpose::CollectionExport,
        )
        .expect("zenity collection export dialog");
        let zenity_args = zenity
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(zenity_args.iter().any(|arg| arg == "--save"));
        assert!(zenity_args
            .iter()
            .any(|arg| arg == "--title=Export OBS-RS collection"));

        let mut kdialog = Command::new("kdialog");
        configure_command(
            &mut kdialog,
            "kdialog",
            "obs-rs-collection.obsrproj",
            PickerPurpose::CollectionImport,
        )
        .expect("kdialog collection import dialog");
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

    #[test]
    fn project_dialog_modes_select_the_matching_picker_purpose() {
        assert_eq!(project_picker_purpose(0), PickerPurpose::ProjectSaveAs);
        assert_eq!(project_picker_purpose(1), PickerPurpose::CollectionExport);
        assert_eq!(project_picker_purpose(2), PickerPurpose::CollectionImport);
        assert_eq!(project_picker_purpose(3), PickerPurpose::ProjectSaveAs);
        assert_eq!(project_picker_purpose(4), PickerPurpose::ProjectOpen);
    }
}
