use super::*;

/// Drives the projector keyboard boundary through the window opened by the
/// real menu/controller path.
pub(super) fn exercise_program_projector_keyboard(
    ui: &crate::MainWindow,
    projectors: &crate::ProjectorController,
) {
    let program_window = projectors
        .projector_window(true)
        .and_then(|window| window.upgrade())
        .expect("the open program projector is reachable");
    program_window
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::F11.into(),
        });
    assert!(
        !projectors.is_fullscreen(true),
        "F11 leaves the program projector fullscreen"
    );
    program_window
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::F11.into(),
        });
    assert!(
        projectors.is_fullscreen(true),
        "F11 restores the program projector fullscreen"
    );
    program_window
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::Escape.into(),
        });
    assert!(
        !projectors.is_open(true),
        "Escape closes the program projector"
    );

    ui.invoke_open_projector(true);
    let native_window = projectors
        .projector_window(true)
        .and_then(|window| window.upgrade())
        .expect("the program projector reopens for native close");
    native_window
        .window()
        .take_snapshot()
        .expect("projector should render before native close dispatch");
    native_window
        .window()
        .dispatch_event(WindowEvent::CloseRequested);
    assert!(
        !projectors.is_open(true),
        "native close closes the program projector"
    );
}
