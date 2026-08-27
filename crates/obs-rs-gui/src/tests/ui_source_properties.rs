use super::*;

pub(super) fn render_source_properties_window() {
    let window = SourcePropertiesWindow::new().expect("properties window should instantiate");
    window.set_source_name("Background".into());
    window.set_source_kind("color_source".into());
    window.set_source_settings("color = \"#405070FF\"\nheight = 360\nwidth = 640\n".into());
    window.set_property_rows(ModelRc::new(VecModel::from(crate::properties::rows(
        "color_source",
        "color = \"#405070FF\"\nheight = 360\nwidth = 640\n",
        UiLocale::English,
    ))));
    window.show().expect("properties window should show");
    for locale in UiLocale::supported() {
        window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(*locale));
        let snapshot = window
            .window()
            .take_snapshot()
            .expect("properties window should render");
        assert!(snapshot.width() > 0 && snapshot.height() > 0);
    }
    window.hide().expect("properties window should hide");
}

/// Verifies that an image source gets the native Browse capability while its
/// selected path still travels through the properties draft and the existing
/// project commit path.
pub(super) fn exercise_image_source_file_picker(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let scene = state
        .borrow()
        .preview_scene()
        .expect("preview scene")
        .to_owned();
    let mut settings = source_settings("image_source").expect("image source defaults");
    settings
        .set("path", "/tmp/example.png")
        .expect("image path setting");
    let source =
        SourceSpec::new("gui-image", "image_source", "GUI image", settings).expect("image source");
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::AddSource {
            profile: "live".to_owned(),
            scene,
            source,
        }))
        .expect("add image source");
    state
        .borrow_mut()
        .dispatch(UiCommand::SelectSource {
            id: "gui-image".to_owned(),
        })
        .expect("select image source");
    refresh_ui(ui, state, surface);

    let controller = crate::install_source_properties_window(ui, state, surface)
        .expect("image properties controller");
    ui.invoke_open_source_properties_window();
    let window =
        crate::callbacks::source_properties::SourcePropertiesController::window(&controller);
    assert_eq!(window.get_source_kind(), "image_source");
    assert_eq!(
        window.get_source_file_picker_enabled(),
        crate::callbacks::detect_file_picker().is_some(),
        "Browse availability must reflect the detected desktop chooser"
    );
    assert_image_path(
        window,
        "/tmp/example.png",
        "image properties expose the source path",
    );
    discard_with_escape(window);

    ui.invoke_open_source_properties_window();
    assert_image_path(
        window,
        "/tmp/example.png",
        "Escape discards the local source-properties draft",
    );
    discard_with_native_close(window);

    ui.invoke_open_source_properties_window();
    assert_image_path(
        window,
        "/tmp/example.png",
        "native window close discards the local source-properties draft",
    );
    commit_with_ctrl_enter(window);

    let state = state.borrow();
    let source = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.source("gui-image"))
        .expect("image source persisted");
    assert_eq!(
        source.settings().get("path"),
        Some("/tmp/selected.png"),
        "the selected image path must commit through source properties"
    );
}

fn assert_image_path(window: &SourcePropertiesWindow, expected: &str, message: &str) {
    let path_row = window
        .get_property_rows()
        .row_data(0)
        .expect("image properties expose a path row");
    assert_eq!(path_row.key, "path");
    assert_eq!(path_row.text, expected, "{message}");
}

fn edit_image_path(window: &SourcePropertiesWindow, path: &str) {
    window.invoke_edit_property("path".into(), path.into());
    assert!(window.get_source_settings().contains(path));
}

fn render_before_key(window: &SourcePropertiesWindow, message: &str) {
    window.window().take_snapshot().expect(message);
}

fn discard_with_escape(window: &SourcePropertiesWindow) {
    edit_image_path(window, "/tmp/discarded.png");
    render_before_key(
        window,
        "properties window should render before Escape dispatch",
    );
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
}

fn discard_with_native_close(window: &SourcePropertiesWindow) {
    edit_image_path(window, "/tmp/native-close-discarded.png");
    render_before_key(
        window,
        "properties window should render before native close dispatch",
    );
    window.window().dispatch_event(WindowEvent::CloseRequested);
}

fn commit_with_ctrl_enter(window: &SourcePropertiesWindow) {
    edit_image_path(window, "/tmp/selected.png");
    render_before_key(
        window,
        "properties window should render before Ctrl+Enter dispatch",
    );
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
}
