use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_project::ProjectCommand;
use obs_rs_ui::{DesktopState, UiCommand, MAX_CANVAS_SELECTIONS};
use slint::Weak;

use crate::{refresh_ui, MainWindow, PreviewSurface};

/// Removes the complete current canvas selection as one undoable project edit.
/// The project command owns target and lock validation so the GUI does not
/// maintain a second source-tree truth.
pub(crate) fn remove_selected_sources_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let (profile, scene, items) = {
            let state = state.borrow();
            let project = state.project_session().project();
            let scene = state
                .preview_scene()
                .ok_or_else(|| std::io::Error::other("no preview scene is selected"))?;
            let items = state
                .selected_sources()
                .take(MAX_CANVAS_SELECTIONS)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            (
                project.active_profile().to_string(),
                scene.to_owned(),
                items,
            )
        };
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::RemoveSceneItems {
                profile,
                scene,
                items,
            }))?;
        Ok(())
    })();

    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Remove sources failed: {error}").into()),
    }
}
