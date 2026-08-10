use std::path::PathBuf;

use obs_rs_capture::VideoCaptureDevice;
use obs_rs_config::Config;
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_plugin_api::{Source, SourceError, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;

use super::{frame_reader::ProcessFrameDevice, settings::parse_video_format};

pub(crate) struct ProcessSourceFactory {
    pub(crate) kind: Identifier,
    pub(crate) command: PathBuf,
    pub(crate) arguments: Vec<String>,
}

impl SourceFactory for ProcessSourceFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        let format = parse_video_format(settings)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        let device = ProcessFrameDevice::new(
            name,
            self.kind.clone(),
            self.command.clone(),
            self.arguments.clone(),
            settings.clone(),
        )
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        Ok(Box::new(ProcessSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            command: self.command.clone(),
            arguments: self.arguments.clone(),
            settings: settings.clone(),
            format,
            device,
        }))
    }
}

struct ProcessSource {
    kind: Identifier,
    name: String,
    command: PathBuf,
    arguments: Vec<String>,
    settings: Config,
    format: VideoFormat,
    device: ProcessFrameDevice,
}

impl Source for ProcessSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_video_format(settings)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        let mut replacement = ProcessFrameDevice::new(
            &self.name,
            self.kind.clone(),
            self.command.clone(),
            self.arguments.clone(),
            settings.clone(),
        )
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        if self.device.is_running() {
            replacement
                .start(format)
                .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        }
        self.device = replacement;
        self.settings = settings.clone();
        self.format = format;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        if !self.device.is_running() {
            self.device
                .start(self.format)
                .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        }
        self.device
            .next_frame(request.timestamp())
            .map_err(|error| SourceError::Unavailable(error.to_string()))
    }
}
