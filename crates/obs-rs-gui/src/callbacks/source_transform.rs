//! Controller for the standalone scene-item Transform window.

use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_media::FrameTransform;
use obs_rs_ui::DesktopState;
use slint::ComponentHandle;

use crate::{
    apply_source_transform_and_refresh, source_transform_document, I18n, MainWindow, Palette,
    PreviewRenderer, SourceTransformWindow,
};

/// Owns the scene-item transform dialog.
pub(crate) struct SourceTransformController {
    window: SourceTransformWindow,
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
    renderer: &Rc<RefCell<PreviewRenderer>>,
) -> Result<Rc<SourceTransformController>, slint::PlatformError> {
    let controller = Rc::new(SourceTransformController {
        window: SourceTransformWindow::new()?,
    });
    install_open(ui, state, &controller);
    install_actions(ui, state, renderer, &controller);
    Ok(controller)
}

fn install_open(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &Rc<SourceTransformController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let controller = Rc::clone(controller);
    ui.on_open_source_transform_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let locale = state.borrow().locale();
        controller
            .window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(locale));
        controller.set_tokens(ui.global::<Palette>().get_tokens());
        populate_from_project(&controller.window, &state);
        if let Err(error) = controller.window.show() {
            ui.set_status_message(format!("Transform window: {error}").into());
        }
    });
}

fn install_actions(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    controller: &Rc<SourceTransformController>,
) {
    let weak = ui.as_weak();
    let accept_state = Rc::clone(state);
    let accept_renderer = Rc::clone(renderer);
    let accept_controller = Rc::clone(controller);
    controller.window.on_accept_transform(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let transform = read_transform(&accept_controller.window);
        match transform {
            Ok(transform) => {
                apply_source_transform_and_refresh(
                    &ui,
                    &accept_state,
                    &accept_renderer,
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

fn populate_from_project(window: &SourceTransformWindow, state: &Rc<RefCell<DesktopState>>) {
    let state = state.borrow();
    let profile = state.project_session().project().active_profile_spec();
    let item = state
        .preview_scene()
        .and_then(|scene_id| profile.and_then(|profile| profile.scene(scene_id)))
        .and_then(|scene| state.selected_source().and_then(|id| scene.item(id)));
    let source = item.and_then(|item| profile.and_then(|profile| profile.source(item.source_id())));
    window.set_source_name(
        source
            .map_or_else(String::new, |source| source.name().to_owned())
            .into(),
    );
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
    window.set_item_opacity(i32::from(transform.opacity()));
    window.set_flip_horizontal(transform.flip_x());
    window.set_flip_vertical(transform.flip_y());
}

fn read_transform(window: &SourceTransformWindow) -> Result<FrameTransform, Box<dyn Error>> {
    FrameTransform::new(
        nonnegative(window.get_scale_x(), "scale-x")?,
        nonnegative(window.get_scale_y(), "scale-y")?,
        window.get_position_x(),
        window.get_position_y(),
        window.get_flip_horizontal(),
        window.get_flip_vertical(),
        u8::try_from(window.get_item_opacity())?,
    )?
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
