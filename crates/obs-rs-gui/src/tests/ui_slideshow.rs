use super::*;
use obs_rs_config::Config;

/// Verifies the slideshow properties path uses the directory-capable picker
/// boundary and still commits the selected directory through the local draft.
/// The native chooser is not opened in the deterministic GUI backend; its
/// command shape is covered by the picker unit tests.
pub(super) fn exercise_slideshow_directory_picker(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("obs-rs-gui-slideshow-picker-{token}"));
    let initial = root.join("initial");
    let selected = root.join("selected");
    std::fs::create_dir_all(&initial).expect("initial slideshow directory");
    std::fs::create_dir_all(&selected).expect("selected slideshow directory");
    let ppm = b"P6\n1 1\n255\n\xFF\x00\x00";
    std::fs::write(initial.join("first.ppm"), ppm).expect("initial slideshow image");
    std::fs::write(selected.join("first.ppm"), ppm).expect("selected slideshow image");

    let scene = state
        .borrow()
        .preview_scene()
        .expect("preview scene")
        .to_owned();
    let mut settings = source_settings("image_slideshow").expect("slideshow defaults");
    settings
        .set("paths", initial.to_str().expect("initial path is UTF-8"))
        .expect("slideshow path setting");
    let source = SourceSpec::new(
        "gui-slideshow",
        "image_slideshow",
        "GUI slideshow",
        settings,
    )
    .expect("slideshow source");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene,
            source,
        }))
        .expect("add slideshow source");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "gui-slideshow".to_owned(),
        })
        .expect("select slideshow source");
    refresh_ui(ui, state, surface);

    let controller = crate::install_source_properties_window(ui, state, surface)
        .expect("slideshow properties controller");
    ui.invoke_open_source_properties_window();
    let window =
        crate::callbacks::source_properties::SourcePropertiesController::window(&controller);
    assert_eq!(window.get_source_kind(), "image_slideshow");
    assert_eq!(
        window.get_source_file_picker_enabled(),
        crate::callbacks::detect_file_picker().is_some(),
        "Browse availability must reflect the detected desktop chooser"
    );
    let paths_row = window
        .get_property_rows()
        .row_data(0)
        .expect("slideshow properties expose a paths row");
    assert_eq!(paths_row.key, "paths");
    assert_eq!(paths_row.text, initial.to_string_lossy().as_ref());

    window.invoke_edit_property(
        "paths".into(),
        selected.to_string_lossy().into_owned().into(),
    );
    let edited_settings = Config::parse(&window.get_source_settings())
        .expect("edited slideshow settings should remain valid");
    assert_eq!(
        edited_settings.get("paths"),
        Some(selected.to_string_lossy().as_ref()),
        "the path comparison must use decoded config values on Windows"
    );
    window.invoke_accept_properties();

    let state_ref = state.borrow();
    let source = state_ref
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("gui-slideshow"))
        .expect("slideshow source persisted");
    assert_eq!(
        source.settings().get("paths"),
        selected.to_str(),
        "the selected slideshow directory must commit through source properties"
    );
    drop(state_ref);
    std::fs::remove_dir_all(root).expect("remove slideshow picker fixture");
}
