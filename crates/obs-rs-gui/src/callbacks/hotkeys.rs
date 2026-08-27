//! Typed local shortcut dispatch at the Slint event boundary.
//!
//! Slint supplies the platform key event as a small canonical label. The
//! label is parsed by `obs-rs-ui` and resolved against the one runtime shortcut
//! table owned by [`DesktopState`]. Slint then executes the returned action so
//! output confirmation and project callbacks remain on their existing paths.

use std::{cell::RefCell, rc::Rc};

use obs_rs_ui::{DesktopState, Shortcut, UiAction};

use crate::MainWindow;

/// Action IDs are an intentionally tiny ABI between the typed Rust action and
/// the Slint function that preserves existing UI callback semantics.
const NO_ACTION: i32 = 0;
const SWAP_PREVIEW_PROGRAM: i32 = 1;
const START_RECORDING: i32 = 2;
const STOP_RECORDING: i32 = 3;
const START_STREAMING: i32 = 4;
const STOP_STREAMING: i32 = 5;
const UNDO: i32 = 6;
const REDO: i32 = 7;
const SAVE_PROJECT: i32 = 8;
const FADE_TRANSITION: i32 = 9;
const SAVE_REPLAY_BUFFER: i32 = 10;
const START_REPLAY_BUFFER: i32 = 11;
const STOP_REPLAY_BUFFER: i32 = 12;
const TOGGLE_MICROPHONE_MUTE: i32 = 13;
const TOGGLE_DESKTOP_MUTE: i32 = 14;
const CUT_TRANSITION: i32 = 15;
const PREVIOUS_PREVIEW_SCENE: i32 = 16;
const NEXT_PREVIEW_SCENE: i32 = 17;
const TOGGLE_STUDIO_MODE: i32 = 18;
const TOGGLE_SELECTED_SOURCE_VISIBILITY: i32 = 19;
const TOGGLE_SELECTED_SOURCE_LOCK: i32 = 20;
const TOGGLE_SELECTED_SOURCE_PROJECTOR: i32 = 21;
const TOGGLE_PREVIEW_SCENE_PROJECTOR: i32 = 22;
const PUSH_TO_TALK_MICROPHONE: i32 = 23;
const PUSH_TO_MUTE_MICROPHONE: i32 = 24;

pub(crate) fn install_shortcut_callbacks(ui: &MainWindow, state: &Rc<RefCell<DesktopState>>) {
    let shortcut_state = Rc::clone(state);
    ui.on_trigger_shortcut(move |label| {
        let Ok(Some(shortcut)) = Shortcut::parse(label.as_str()) else {
            return NO_ACTION;
        };
        shortcut_state
            .borrow()
            .shortcut_action(&shortcut)
            .map_or(NO_ACTION, action_code)
    });
}

fn action_code(action: UiAction) -> i32 {
    match action {
        UiAction::SwapPreviewProgram => SWAP_PREVIEW_PROGRAM,
        UiAction::PreviousPreviewScene => PREVIOUS_PREVIEW_SCENE,
        UiAction::NextPreviewScene => NEXT_PREVIEW_SCENE,
        UiAction::StartRecording => START_RECORDING,
        UiAction::StopRecording => STOP_RECORDING,
        UiAction::StartStreaming => START_STREAMING,
        UiAction::StopStreaming => STOP_STREAMING,
        UiAction::Undo => UNDO,
        UiAction::Redo => REDO,
        UiAction::SaveProject => SAVE_PROJECT,
        UiAction::CutTransition => CUT_TRANSITION,
        UiAction::FadeTransition => FADE_TRANSITION,
        UiAction::SaveReplayBuffer => SAVE_REPLAY_BUFFER,
        UiAction::StartReplayBuffer => START_REPLAY_BUFFER,
        UiAction::StopReplayBuffer => STOP_REPLAY_BUFFER,
        UiAction::ToggleMicrophoneMute => TOGGLE_MICROPHONE_MUTE,
        UiAction::ToggleDesktopMute => TOGGLE_DESKTOP_MUTE,
        UiAction::PushToTalkMicrophone => PUSH_TO_TALK_MICROPHONE,
        UiAction::PushToMuteMicrophone => PUSH_TO_MUTE_MICROPHONE,
        UiAction::ToggleStudioMode => TOGGLE_STUDIO_MODE,
        UiAction::ToggleSelectedSourceVisibility => TOGGLE_SELECTED_SOURCE_VISIBILITY,
        UiAction::ToggleSelectedSourceLock => TOGGLE_SELECTED_SOURCE_LOCK,
        UiAction::ToggleSelectedSourceProjector => TOGGLE_SELECTED_SOURCE_PROJECTOR,
        UiAction::TogglePreviewSceneProjector => TOGGLE_PREVIEW_SCENE_PROJECTOR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_codes_are_stable_and_nonzero() {
        let actions = [
            UiAction::SwapPreviewProgram,
            UiAction::PreviousPreviewScene,
            UiAction::NextPreviewScene,
            UiAction::StartRecording,
            UiAction::StopRecording,
            UiAction::StartStreaming,
            UiAction::StopStreaming,
            UiAction::Undo,
            UiAction::Redo,
            UiAction::SaveProject,
            UiAction::CutTransition,
            UiAction::FadeTransition,
            UiAction::SaveReplayBuffer,
            UiAction::StartReplayBuffer,
            UiAction::StopReplayBuffer,
            UiAction::ToggleMicrophoneMute,
            UiAction::ToggleDesktopMute,
            UiAction::PushToTalkMicrophone,
            UiAction::PushToMuteMicrophone,
            UiAction::ToggleStudioMode,
            UiAction::ToggleSelectedSourceVisibility,
            UiAction::ToggleSelectedSourceLock,
            UiAction::ToggleSelectedSourceProjector,
            UiAction::TogglePreviewSceneProjector,
        ];
        let mut codes = actions.into_iter().map(action_code).collect::<Vec<_>>();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(
            codes,
            (SWAP_PREVIEW_PROGRAM..=PUSH_TO_MUTE_MICROPHONE).collect::<Vec<_>>()
        );
        assert!(!codes.contains(&NO_ACTION));
    }
}
