//! Controller for the standalone scene-item Transform window.

use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_media::FrameTransform;
use obs_rs_ui::DesktopState;
use slint::ComponentHandle;

use crate::{
    apply_source_transform_to, callbacks::canvas::canvas_item_for_target, scene_item_target,
    source_transform_document, I18n, MainWindow, Palette, PreviewSurface, SceneItemTarget,
    SourceTransformWindow,
};

/// Owns the scene-item transform dialog.
pub(crate) struct SourceTransformController {
    window: SourceTransformWindow,
    /// The scene item this dialog was opened for.
    ///
    /// A dialog is open for as long as the user wants it to be, and the studio
    /// window behind it stays clickable, so the item it edits is fixed when it
    /// opens rather than looked up again at OK.
    target: RefCell<Option<SceneItemTarget>>,
}

impl SourceTransformController {
    /// Repaints this window when the studio theme changes.
    pub(crate) fn set_tokens(&self, tokens: crate::ThemeTokens) {
        self.window.global::<Palette>().set_tokens(tokens);
    }

    #[cfg(test)]
    pub(crate) fn window(&self) -> &SourceTransformWindow {
        &self.window
    }
}

/// Creates and wires the standalone transform dialog.
pub(crate) fn install_source_transform_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) -> Result<Rc<SourceTransformController>, slint::PlatformError> {
    let controller = Rc::new(SourceTransformController {
        window: SourceTransformWindow::new()?,
        target: RefCell::new(None),
    });
    install_open(ui, state, &controller);
    install_actions(ui, state, surface, &controller);
    Ok(controller)
}

fn install_open(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &Rc<SourceTransformController>,
) {
    let weak = ui.as_weak();
    let window_state = Rc::clone(state);
    let window_controller = Rc::clone(controller);
    ui.on_open_source_transform_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let target = ui.get_selected_source().to_string();
        open_for_target(&ui, &window_state, &window_controller, &target);
    });

    let weak = ui.as_weak();
    let target_state = Rc::clone(state);
    let target_controller = Rc::clone(controller);
    ui.on_open_source_transform_for(move |target| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        open_for_target(&ui, &target_state, &target_controller, target.as_str());
    });
}

fn open_for_target(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &SourceTransformController,
    item: &str,
) {
    let Some(target) = scene_item_target(&state.borrow(), item) else {
        ui.set_status_message("Transform failed: the target is not a scene item".into());
        return;
    };
    let locale = state.borrow().locale();
    controller
        .window
        .global::<I18n>()
        .set_text(crate::i18n::catalog(locale));
    controller.set_tokens(ui.global::<Palette>().get_tokens());
    controller.target.replace(Some(target.clone()));
    populate_from_project(&controller.window, state, &target);
    match controller.window.show() {
        Ok(()) => controller.window.invoke_focus_keyboard_boundary(),
        Err(error) => ui.set_status_message(format!("Transform window: {error}").into()),
    }
}

fn install_actions(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    controller: &Rc<SourceTransformController>,
) {
    let weak = ui.as_weak();
    let accept_state = Rc::clone(state);
    let accept_surface = Rc::clone(surface);
    let accept_controller = Rc::clone(controller);
    controller.window.on_accept_transform(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(target) = accept_controller.target.borrow().clone() else {
            ui.set_status_message("Transform failed: no source is selected".into());
            return;
        };
        let transform = read_transform(&accept_controller.window);
        match transform {
            Ok(transform) => {
                apply_source_transform_to(
                    &ui,
                    &accept_state,
                    &accept_surface,
                    &target,
                    &source_transform_document(transform),
                );
                let _ = accept_controller.window.hide();
            }
            Err(error) => ui.set_status_message(format!("Transform failed: {error}").into()),
        }
    });

    let reset_controller = Rc::clone(controller);
    controller.window.on_reset_transform(move || {
        set_transform(&reset_controller.window, FrameTransform::IDENTITY);
    });

    let close_controller = Rc::clone(controller);
    controller.window.on_close_window(move || {
        let _ = close_controller.window.hide();
    });
}

fn populate_from_project(
    window: &SourceTransformWindow,
    state: &Rc<RefCell<DesktopState>>,
    target: &SceneItemTarget,
) {
    let state = state.borrow();
    let profile = state
        .project_session()
        .project()
        .profile(target.profile.as_str());
    let item = profile
        .and_then(|profile| canvas_item_for_target(profile, target.scene.as_str(), &target.item));
    let source_name = item.and_then(|item| {
        profile.and_then(|profile| {
            if item.is_scene_reference() {
                profile
                    .scene(item.source_id())
                    .map(|scene| scene.name().to_owned())
            } else {
                profile
                    .source(item.source_id())
                    .map(|source| source.name().to_owned())
            }
        })
    });
    window.set_source_name(source_name.unwrap_or_else(|| target.item.clone()).into());
    set_transform(
        window,
        item.map_or(
            FrameTransform::IDENTITY,
            obs_rs_project::SceneItemSpec::transform,
        ),
    );
}

fn set_transform(window: &SourceTransformWindow, transform: FrameTransform) {
    window.set_scale_x(i32::try_from(transform.scale_x_milli()).unwrap_or(i32::MAX));
    window.set_scale_y(i32::try_from(transform.scale_y_milli()).unwrap_or(i32::MAX));
    window.set_position_x(transform.translate_x());
    window.set_position_y(transform.translate_y());
    window.set_crop_left(i32::try_from(transform.crop_left()).unwrap_or(i32::MAX));
    window.set_crop_top(i32::try_from(transform.crop_top()).unwrap_or(i32::MAX));
    window.set_crop_right(i32::try_from(transform.crop_right()).unwrap_or(i32::MAX));
    window.set_crop_bottom(i32::try_from(transform.crop_bottom()).unwrap_or(i32::MAX));
    window.set_rotation_degrees(transform.rotation_degrees());
    window.set_item_opacity(i32::from(transform.opacity()));
    window.set_flip_horizontal(transform.flip_x());
    window.set_flip_vertical(transform.flip_y());
}

fn read_transform(window: &SourceTransformWindow) -> Result<FrameTransform, Box<dyn Error>> {
    let transform = FrameTransform::new(
        nonnegative(window.get_scale_x(), "scale-x")?,
        nonnegative(window.get_scale_y(), "scale-y")?,
        window.get_position_x(),
        window.get_position_y(),
        window.get_flip_horizontal(),
        window.get_flip_vertical(),
        u8::try_from(window.get_item_opacity())?,
    )?
    .with_rotation_degrees(window.get_rotation_degrees())?;
    transform
        .with_crop(
            nonnegative(window.get_crop_left(), "crop-left")?,
            nonnegative(window.get_crop_top(), "crop-top")?,
            nonnegative(window.get_crop_right(), "crop-right")?,
            nonnegative(window.get_crop_bottom(), "crop-bottom")?,
        )
        .map_err(Into::into)
}

fn nonnegative(value: i32, field: &str) -> Result<u32, Box<dyn Error>> {
    u32::try_from(value).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{field} is negative"),
        )
        .into()
    })
}

#[cfg(test)]
pub(crate) fn source_transform_window(
    controller: &Rc<SourceTransformController>,
) -> &SourceTransformWindow {
    controller.window()
}
