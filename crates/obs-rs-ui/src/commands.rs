use std::collections::BTreeMap;

use obs_rs_audio::{AudioFormat, AudioMixer};
use obs_rs_audio::{MAX_GAIN_MILLI, MAX_PAN_MILLI, MIN_PAN_MILLI};
use obs_rs_media::FrameTransition;

use super::{
    error::UiError,
    helpers::{first_scene_id, first_source_id, project_has_scene, project_has_source},
    state::DesktopState,
    types::{UiAction, UiCommand, UiNotice},
    MAX_UI_NOTICES,
};

impl DesktopState {
    pub(crate) fn ensure_scene(&self, id: &str) -> Result<(), UiError> {
        // One borrow of the project, one keyed profile lookup.
        let project = self.project.project();
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "profile",
                id: project.active_profile().to_string(),
            })?;
        if profile.scene(id).is_some() {
            Ok(())
        } else {
            Err(UiError::UnknownSelection {
                kind: "scene",
                id: id.to_owned(),
            })
        }
    }

    pub(crate) fn sync_selections_after_project_update(&mut self) {
        let (preview_valid, program_valid, selected_sources) = {
            let project = self.project.project();
            let preview_valid = self
                .preview_scene
                .as_ref()
                .is_some_and(|scene| project_has_scene(project, scene));
            let program_valid = self
                .program_scene
                .as_ref()
                .is_some_and(|scene| project_has_scene(project, scene));
            let selected_sources = self
                .preview_scene
                .as_ref()
                .map(|scene| {
                    self.selected_sources
                        .iter()
                        .filter(|source| project_has_source(project, scene, source))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (preview_valid, program_valid, selected_sources)
        };
        // `first_scene_id` is the same answer for preview and program, so it is
        // resolved at most once instead of per invalid selection.
        if !preview_valid || !program_valid {
            let fallback = first_scene_id(self.project.project());
            if !preview_valid {
                self.preview_scene.clone_from(&fallback);
            }
            if !program_valid {
                self.program_scene = fallback;
            }
        }
        self.selected_sources = selected_sources;
        if self.selected_sources.is_empty() {
            self.selected_sources = self
                .preview_scene
                .as_ref()
                .and_then(|scene| first_source_id(self.project.project(), scene))
                .into_iter()
                .collect();
        }
    }

    pub(crate) fn ensure_source(&self, id: &str) -> Result<(), UiError> {
        let preview_scene = self
            .preview_scene()
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "scene",
                id: "none".to_owned(),
            })?;
        let project = self.project.project();
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "profile",
                id: project.active_profile().to_string(),
            })?;
        let scene = profile
            .scene(preview_scene)
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "scene",
                id: preview_scene.to_owned(),
            })?;
        if scene.has_item(id) {
            Ok(())
        } else {
            Err(UiError::UnknownSelection {
                kind: "source",
                id: id.to_owned(),
            })
        }
    }

    pub(crate) fn dispatch_action(&mut self, action: UiAction) -> Result<(), UiError> {
        match action {
            UiAction::SwapPreviewProgram => {
                std::mem::swap(&mut self.preview_scene, &mut self.program_scene);
                Ok(())
            }
            UiAction::StartRecording => self.dispatch(UiCommand::StartRecording),
            UiAction::StopRecording => self.dispatch(UiCommand::StopRecording),
            UiAction::StartStreaming => self.dispatch(UiCommand::StartStreaming),
            UiAction::StopStreaming => self.dispatch(UiCommand::StopStreaming),
            UiAction::Undo => self.dispatch(UiCommand::Undo),
            UiAction::Redo => self.dispatch(UiCommand::Redo),
            UiAction::SaveProject
            | UiAction::FadeTransition
            | UiAction::SaveReplayBuffer
            | UiAction::StartReplayBuffer
            | UiAction::StopReplayBuffer => Err(UiError::FrontendActionRequired(action)),
        }
    }

    pub(crate) fn set_mixer_gain(&mut self, id: &str, gain_milli: u16) -> Result<(), UiError> {
        if gain_milli > MAX_GAIN_MILLI {
            return Err(UiError::InvalidMixerGain(gain_milli));
        }
        let source = *self
            .mixer_sources
            .get(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?;
        self.audio_mixer
            .set_gain(source, f32::from(gain_milli) / 1_000.0)?;
        let channel = self
            .mixer_channels
            .get_mut(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?;
        channel.gain_milli = gain_milli;
        Ok(())
    }

    pub(crate) fn set_mixer_pan(&mut self, id: &str, pan_milli: i32) -> Result<(), UiError> {
        if !(MIN_PAN_MILLI..=MAX_PAN_MILLI).contains(&pan_milli) {
            return Err(UiError::InvalidMixerPan(pan_milli));
        }
        let source = *self
            .mixer_sources
            .get(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?;
        self.audio_mixer.set_pan_milli(source, pan_milli)?;
        let channel = self
            .mixer_channels
            .get_mut(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?;
        channel.pan_milli = pan_milli;
        Ok(())
    }

    /// Rebuilds the mixer at a new format, carrying every channel's gain and
    /// mute across so changing the sample rate does not reset the desk.
    pub(crate) fn set_audio_format(
        &mut self,
        sample_rate: u32,
        channels: u16,
    ) -> Result<(), UiError> {
        let format = AudioFormat::new(sample_rate, channels).map_err(UiError::Audio)?;
        if format == self.audio_mixer.format() {
            return Ok(());
        }
        let mut mixer = AudioMixer::new(format);
        let mut sources = BTreeMap::new();
        for (id, channel) in &self.mixer_channels {
            let gain = f32::from(channel.gain_milli) / 1_000.0;
            let source = mixer.add_source(gain).map_err(UiError::Audio)?;
            mixer
                .set_muted(source, channel.muted)
                .map_err(UiError::Audio)?;
            mixer
                .set_pan_milli(source, channel.pan_milli)
                .map_err(UiError::Audio)?;
            sources.insert(id.clone(), source);
        }
        self.audio_mixer = mixer;
        self.mixer_sources = sources;
        for channel in self.mixer_channels.values_mut() {
            channel.peak_milli = 0;
            channel.peak_hold_milli = 0;
            channel.clipped = false;
        }
        Ok(())
    }

    pub(crate) fn set_recording(&mut self, active: bool) -> Result<&'static str, UiError> {
        if active && self.recording {
            return Err(UiError::RecordingAlreadyActive);
        }
        if !active && !self.recording {
            return Err(UiError::RecordingNotActive);
        }
        self.recording = active;
        Ok(if active {
            "recording started"
        } else {
            "recording stopped"
        })
    }

    pub(crate) fn set_streaming(&mut self, active: bool) -> Result<&'static str, UiError> {
        if active && self.streaming {
            return Err(UiError::StreamingAlreadyActive);
        }
        if !active && !self.streaming {
            return Err(UiError::StreamingNotActive);
        }
        self.streaming = active;
        Ok(if active {
            "streaming started"
        } else {
            "streaming stopped"
        })
    }

    pub(crate) fn set_transition(&mut self, transition: FrameTransition) -> Result<(), UiError> {
        match transition {
            FrameTransition::CrossFade { progress_milli } => {
                FrameTransition::cross_fade(progress_milli).map_err(UiError::Media)?;
            }
            FrameTransition::FadeToColor {
                progress_milli,
                color,
            } => {
                FrameTransition::fade_to_color(progress_milli, color).map_err(UiError::Media)?;
            }
            FrameTransition::Cut => {}
        }
        self.transition = transition;
        Ok(())
    }

    pub(crate) fn take_preview(
        &mut self,
        transition: FrameTransition,
    ) -> Result<&'static str, UiError> {
        let preview = self
            .preview_scene
            .clone()
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "preview scene",
                id: "none".to_owned(),
            })?;
        self.set_transition(transition)?;
        self.program_scene = Some(preview);
        Ok("preview sent to program")
    }

    pub(crate) fn toggle_mixer_mute(&mut self, id: &str) -> Result<&'static str, UiError> {
        let source = *self
            .mixer_sources
            .get(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?;
        let muted = !self
            .mixer_channels
            .get(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?
            .muted;
        self.audio_mixer.set_muted(source, muted)?;
        self.mixer_channels
            .get_mut(id)
            .ok_or_else(|| UiError::UnknownMixerChannel(id.to_owned()))?
            .muted = muted;
        Ok(if muted {
            "mixer channel muted"
        } else {
            "mixer channel unmuted"
        })
    }

    pub(crate) fn notice(&mut self, message: &str) -> Result<(), UiError> {
        let sequence = self.next_notice_sequence;
        self.next_notice_sequence = self
            .next_notice_sequence
            .checked_add(1)
            .ok_or(UiError::NoticeSequenceExhausted)?;
        if self.notices.len() == MAX_UI_NOTICES {
            let _ = self.notices.pop_front();
        }
        self.notices.push_back(UiNotice {
            sequence,
            message: message.to_owned(),
        });
        Ok(())
    }
}
