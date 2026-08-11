//! Linux `PipeWire` audio input through the reviewed `pw-cat` boundary.
//!
//! The core remains free of native FFI. This adapter discovers whether a
//! `PipeWire` graph is usable with `pw-dump`, then reads negotiated raw `f32`
//! samples from `pw-cat`. If either command is unavailable, the engine can
//! select its deterministic fallback provider.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use obs_rs_audio::{
    AudioBuffer, AudioDeviceError, AudioDeviceInfo, AudioDeviceKind, AudioFormat, AudioInput,
    AudioInputProvider, AudioInputState, AudioOutput, AudioOutputProvider, AudioOutputState,
};
use obs_rs_media::Timestamp;

/// Stable identifier for the `PipeWire` default input route.
pub const DEFAULT_INPUT_ID: &str = "pipewire-default";

/// Stable identifier for the `PipeWire` default output route.
pub const DEFAULT_OUTPUT_ID: &str = "pipewire-default-output";

/// Process-backed `PipeWire` provider.
#[derive(Clone, Debug)]
pub struct PipeWireAudioProvider {
    dump_command: PathBuf,
    cat_command: PathBuf,
}

impl PipeWireAudioProvider {
    /// Creates a provider using `pw-dump` and `pw-cat` from `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            dump_command: PathBuf::from("pw-dump"),
            cat_command: PathBuf::from("pw-cat"),
        }
    }

    /// Creates a provider with explicit command paths, useful for packaging and
    /// deterministic adapter tests.
    #[must_use]
    pub fn with_commands(
        dump_command: impl Into<PathBuf>,
        cat_command: impl Into<PathBuf>,
    ) -> Self {
        Self {
            dump_command: dump_command.into(),
            cat_command: cat_command.into(),
        }
    }

    /// Returns the configured `pw-dump` executable path.
    #[must_use]
    pub fn dump_command(&self) -> &Path {
        &self.dump_command
    }

    /// Returns the configured `pw-cat` executable path.
    #[must_use]
    pub fn cat_command(&self) -> &Path {
        &self.cat_command
    }

    /// Returns the `pw-cat` target of the first playback node in the graph.
    ///
    /// `pw-cat` accepts no "default sink" alias, so the alias this crate
    /// publishes has to be resolved to a concrete node before capture starts.
    fn default_sink_target(&self) -> Result<String, AudioDeviceError> {
        let output = Command::new(&self.dump_command).output().map_err(|error| {
            AudioDeviceError::Unavailable(format!(
                "PipeWire discovery command {} failed to start: {error}",
                self.dump_command.display()
            ))
        })?;
        let graph = String::from_utf8_lossy(&output.stdout);
        first_sink_target(&graph).ok_or_else(|| {
            AudioDeviceError::Unavailable("PipeWire has no playback node".to_owned())
        })
    }
}

impl Default for PipeWireAudioProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// The `pw-cat` file operand meaning "use standard input/output".
///
/// `pw-cat` always takes a file to read from or write to; `-` selects the
/// stdio stream instead of a path, which is what lets OBS-RS pipe raw samples
/// to and from the process rather than going through a temporary file. It is
/// positional and mandatory: dropping it makes `pw-cat` exit with a usage
/// error, and capture silently stops working.
const STDIO_OPERAND: &str = "-";

/// Direction one `pw-cat` invocation runs in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CatMode {
    /// Read samples from a node, writing them to stdout.
    Record,
    /// Write samples to a node, reading them from stdin.
    Playback,
}

impl CatMode {
    const fn flag(self) -> &'static str {
        match self {
            Self::Record => "--record",
            Self::Playback => "--playback",
        }
    }
}

/// Builds the complete `pw-cat` argument vector for one stream.
///
/// Kept separate from spawning so the command shape is covered by a test: the
/// arguments are load-bearing and a silent change to them degrades capture to
/// a runtime failure that no compile-time check would catch.
fn cat_arguments(mode: CatMode, format: AudioFormat, target: Option<&str>) -> Vec<String> {
    let mut arguments = vec![
        mode.flag().to_owned(),
        "--raw".to_owned(),
        "--format".to_owned(),
        "f32".to_owned(),
        "--rate".to_owned(),
        format.sample_rate().to_string(),
        "--channels".to_owned(),
        format.channels().to_string(),
    ];
    if let Some(target) = target {
        arguments.push("--target".to_owned());
        arguments.push(target.to_owned());
    }
    arguments.push(STDIO_OPERAND.to_owned());
    arguments
}

impl AudioInputProvider for PipeWireAudioProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        let output = Command::new(&self.dump_command).output().map_err(|error| {
            AudioDeviceError::Unavailable(format!(
                "PipeWire discovery command {} failed to start: {error}",
                self.dump_command.display()
            ))
        })?;
        if !output.status.success() {
            return Err(AudioDeviceError::Unavailable(format!(
                "PipeWire discovery exited with {}",
                output.status
            )));
        }
        let graph = String::from_utf8_lossy(&output.stdout);
        let devices = discover_devices(&graph);
        if devices.is_empty() {
            return Err(AudioDeviceError::Unavailable(
                "PipeWire has no discoverable audio node".to_owned(),
            ));
        }
        Ok(devices)
    }

    fn open_input(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
        // Recording *from* a playback node is how desktop audio is captured:
        // `pw-cat --record --target <sink>` reads that sink's monitor. The
        // default-output alias has no node number of its own, so it is resolved
        // against the graph here rather than being rejected as an input.
        let resolved;
        let target = if device_id == DEFAULT_INPUT_ID {
            None
        } else if device_id == DEFAULT_OUTPUT_ID {
            resolved = self.default_sink_target()?;
            Some(resolved.as_str())
        } else {
            Some(node_target(device_id).ok_or_else(|| {
                AudioDeviceError::Unavailable(format!("unknown PipeWire input {device_id}"))
            })?)
        };
        let mut command = Command::new(&self.cat_command);
        command
            .args(cat_arguments(CatMode::Record, format, target))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|error| {
            AudioDeviceError::Unavailable(format!(
                "PipeWire capture command {} failed to start: {error}",
                self.cat_command.display()
            ))
        })?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AudioDeviceError::Unavailable(
                "PipeWire capture has no stdout pipe".to_owned(),
            ));
        };
        Ok(Box::new(PipeWireInput {
            format,
            state: AudioInputState::Stopped,
            child,
            stdout,
            scratch: Vec::new(),
        }))
    }
}

impl AudioOutputProvider for PipeWireAudioProvider {
    fn discover_outputs(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        Ok(self
            .discover()?
            .into_iter()
            .filter(|device| device.kind() == AudioDeviceKind::Output)
            .collect())
    }

    fn open_output(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioOutput>, AudioDeviceError> {
        let target = if device_id == DEFAULT_OUTPUT_ID {
            None
        } else {
            Some(node_target(device_id).ok_or_else(|| {
                AudioDeviceError::Unavailable(format!("unknown PipeWire output {device_id}"))
            })?)
        };
        let mut command = Command::new(&self.cat_command);
        command
            .args(cat_arguments(CatMode::Playback, format, target))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|error| {
            AudioDeviceError::Unavailable(format!(
                "PipeWire playback command {} failed to start: {error}",
                self.cat_command.display()
            ))
        })?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AudioDeviceError::Unavailable(
                "PipeWire playback has no stdin pipe".to_owned(),
            ));
        };
        Ok(Box::new(PipeWireOutput {
            format,
            state: AudioOutputState::Stopped,
            child,
            stdin,
        }))
    }
}

struct PipeWireInput {
    format: AudioFormat,
    state: AudioInputState,
    child: Child,
    stdout: ChildStdout,
    /// Scratch buffer reused across reads.
    ///
    /// Blocks arrive around 200 times a second at a fixed size, so allocating
    /// and freeing the same buffer on every read was pure churn on the audio
    /// path. It is kept here and resized only when the block size changes.
    scratch: Vec<u8>,
}

struct PipeWireOutput {
    format: AudioFormat,
    state: AudioOutputState,
    child: Child,
    stdin: ChildStdin,
}

impl AudioInput for PipeWireInput {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn state(&self) -> AudioInputState {
        self.state
    }

    fn read_block(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<AudioBuffer, AudioDeviceError> {
        if frames == 0 {
            return Err(AudioDeviceError::InvalidDevice(
                "PipeWire audio blocks must contain at least one frame".to_owned(),
            ));
        }
        let sample_count = frames
            .checked_mul(usize::from(self.format.channels()))
            .ok_or_else(|| {
                AudioDeviceError::InvalidDevice("audio block is too large".to_owned())
            })?;
        let byte_count = sample_count
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or_else(|| {
                AudioDeviceError::InvalidDevice("audio block byte size overflowed".to_owned())
            })?;
        // `resize` keeps the existing allocation whenever the block size is
        // unchanged, which is every read after the first.
        self.scratch.resize(byte_count, 0);
        if let Err(error) = self.stdout.read_exact(&mut self.scratch) {
            self.state = AudioInputState::Failed;
            let error = if error.kind() == ErrorKind::UnexpectedEof {
                std::io::Error::new(ErrorKind::UnexpectedEof, "PipeWire ended the audio stream")
            } else {
                error
            };
            return Err(AudioDeviceError::Io(error));
        }
        let samples = self
            .scratch
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        self.state = AudioInputState::Running;
        Ok(AudioBuffer::new(self.format, timestamp, samples)?)
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.state = AudioInputState::Stopped;
    }
}

impl Drop for PipeWireInput {
    fn drop(&mut self) {
        self.stop();
    }
}

impl AudioOutput for PipeWireOutput {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn state(&self) -> AudioOutputState {
        self.state
    }

    fn write_block(&mut self, buffer: &AudioBuffer) -> Result<(), AudioDeviceError> {
        if buffer.format() != self.format {
            return Err(AudioDeviceError::Audio(
                obs_rs_audio::AudioError::FormatMismatch {
                    expected: self.format,
                    actual: buffer.format(),
                },
            ));
        }
        for sample in buffer.samples() {
            if let Err(error) = self.stdin.write_all(&sample.to_le_bytes()) {
                self.state = AudioOutputState::Failed;
                return Err(AudioDeviceError::Io(error));
            }
        }
        self.state = AudioOutputState::Running;
        Ok(())
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.state = AudioOutputState::Stopped;
    }
}

impl Drop for PipeWireOutput {
    fn drop(&mut self) {
        self.stop();
    }
}

fn node_target(device_id: &str) -> Option<&str> {
    let target = device_id.strip_prefix("pipewire-node-")?;
    (!target.is_empty() && target.bytes().all(|byte| byte.is_ascii_digit())).then_some(target)
}

fn first_sink_target(graph: &str) -> Option<String> {
    top_level_objects(graph)
        .into_iter()
        .filter_map(parse_node)
        .find(|(_, _, kind)| *kind == AudioDeviceKind::Output)
        .map(|(id, _, _)| id.to_string())
}

fn discover_devices(document: &str) -> Vec<AudioDeviceInfo> {
    let mut devices = top_level_objects(document)
        .into_iter()
        .filter_map(parse_node)
        .filter_map(|(id, name, kind)| {
            AudioDeviceInfo::new(format!("pipewire-node-{id}"), name, kind).ok()
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.id().cmp(right.id()));
    devices.dedup_by(|left, right| left.id() == right.id());
    let has_input = devices
        .iter()
        .any(|device| device.kind() == AudioDeviceKind::Input);
    let has_output = devices
        .iter()
        .any(|device| device.kind() == AudioDeviceKind::Output);
    if has_input {
        devices.insert(
            0,
            AudioDeviceInfo::new(
                DEFAULT_INPUT_ID,
                "PipeWire default input",
                AudioDeviceKind::Input,
            )
            .unwrap_or_else(|error| unreachable!("default PipeWire input is valid: {error}")),
        );
    }
    if has_output {
        let index = devices
            .iter()
            .position(|device| device.kind() == AudioDeviceKind::Output)
            .unwrap_or(devices.len());
        devices.insert(
            index,
            AudioDeviceInfo::new(
                DEFAULT_OUTPUT_ID,
                "PipeWire default output",
                AudioDeviceKind::Output,
            )
            .unwrap_or_else(|error| unreachable!("default PipeWire output is valid: {error}")),
        );
    }
    devices
}

fn parse_node(object: &str) -> Option<(u32, String, AudioDeviceKind)> {
    if json_string_field(object, "type")? != "PipeWire:Interface:Node" {
        return None;
    }
    let kind = match json_string_field(object, "media.class")?.as_str() {
        "Audio/Source" => AudioDeviceKind::Input,
        "Audio/Sink" => AudioDeviceKind::Output,
        _ => return None,
    };
    let id = json_u32_field(object, "id")?;
    let name = json_string_field(object, "node.description")
        .or_else(|| json_string_field(object, "node.nick"))
        .or_else(|| json_string_field(object, "node.name"))
        .unwrap_or_else(|| format!("PipeWire node {id}"));
    Some((id, name, kind))
}

fn top_level_objects(document: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let mut depth = 0_u32;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in document.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth = depth.saturating_add(1);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(start) = start.take() {
                        objects.push(&document[start..=index]);
                    }
                }
            }
            _ => {}
        }
    }
    objects
}

fn json_string_field(object: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let position = object.find(&needle)? + needle.len();
    let colon = object[position..].find(':')? + position + 1;
    decode_json_string(object[colon..].trim_start())
}

fn decode_json_string(value: &str) -> Option<String> {
    let mut chars = value.chars();
    if chars.next()? != '"' {
        return None;
    }
    let mut decoded = String::new();
    while let Some(character) = chars.next() {
        match character {
            '"' => return Some(decoded),
            '\\' => match chars.next()? {
                '"' => decoded.push('"'),
                '\\' => decoded.push('\\'),
                '/' => decoded.push('/'),
                'b' => decoded.push('\u{0008}'),
                'f' => decoded.push('\u{000C}'),
                'n' => decoded.push('\n'),
                'r' => decoded.push('\r'),
                't' => decoded.push('\t'),
                _ => return None,
            },
            character if character.is_control() => return None,
            character => decoded.push(character),
        }
    }
    None
}

fn json_u32_field(object: &str, key: &str) -> Option<u32> {
    let needle = format!("\"{key}\"");
    let position = object.find(&needle)? + needle.len();
    let colon = object[position..].find(':')? + position + 1;
    object[colon..]
        .trim_start()
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_uses_stable_default_id() {
        let provider = PipeWireAudioProvider::with_commands("pw-dump-test", "pw-cat-test");
        assert_eq!(provider.dump_command(), Path::new("pw-dump-test"));
        assert_eq!(provider.cat_command(), Path::new("pw-cat-test"));
    }

    #[test]
    fn unknown_device_is_rejected_before_spawning_process() {
        let provider = PipeWireAudioProvider::new();
        let format = AudioFormat::new(48_000, 2).expect("format");
        let Err(error) = provider.open_input("missing", format) else {
            panic!("unknown device should fail")
        };
        assert!(error.to_string().contains("unknown PipeWire input"));
    }

    #[test]
    fn the_default_output_alias_resolves_to_a_concrete_sink_node() {
        let document = r#"
        [
          {"id": 42, "type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Audio/Source", "node.name": "alsa_input.usb"
          }}},
          {"id": 7, "type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Audio/Sink", "node.name": "alsa_output.pci"
          }}}
        ]
        "#;
        assert_eq!(first_sink_target(document).as_deref(), Some("7"));
        assert_eq!(first_sink_target("[]"), None);
    }

    #[test]
    fn discovery_enumerates_stable_input_and_output_nodes() {
        let document = r#"
        [
          {"id": 42, "type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Audio/Source", "node.name": "alsa_input.usb", "node.description": "USB Mic"
          }}},
          {"id": 7, "type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Audio/Sink", "node.name": "alsa_output.pci", "node.description": "Speakers"
          }}},
          {"id": 42, "type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Audio/Source", "node.name": "duplicate"
          }}}
        ]
        "#;
        let devices = discover_devices(document);
        assert_eq!(devices.len(), 4);
        assert_eq!(devices[0].id(), DEFAULT_INPUT_ID);
        assert_eq!(devices[0].kind(), AudioDeviceKind::Input);
        let default_output = devices
            .iter()
            .find(|device| device.id() == DEFAULT_OUTPUT_ID)
            .expect("default output");
        assert_eq!(default_output.kind(), AudioDeviceKind::Output);
        assert!(devices
            .iter()
            .any(|device| { device.id() == "pipewire-node-42" && device.name() == "USB Mic" }));
        assert!(devices
            .iter()
            .any(|device| { device.id() == "pipewire-node-7" && device.name() == "Speakers" }));
    }

    #[test]
    fn a_node_that_left_the_graph_is_gone_from_the_next_snapshot() {
        let before = r#"
        [
          {"id": 42, "type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Audio/Source", "node.description": "USB Mic"
          }}},
          {"id": 43, "type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Audio/Source", "node.description": "Webcam Mic"
          }}}
        ]
        "#;
        // The USB microphone was unplugged between the two dumps.
        let after = r#"
        [
          {"id": 43, "type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Audio/Source", "node.description": "Webcam Mic"
          }}}
        ]
        "#;

        let before = discover_devices(before);
        let after = discover_devices(after);

        assert!(before
            .iter()
            .any(|device| device.id() == "pipewire-node-42"));
        assert!(
            !after.iter().any(|device| device.id() == "pipewire-node-42"),
            "a removed node must not linger in a later snapshot"
        );
        assert!(after.iter().any(|device| device.id() == "pipewire-node-43"));
    }

    #[test]
    fn a_graph_without_audio_nodes_offers_no_devices_at_all() {
        // Video nodes and non-node objects are present but none can capture.
        let document = r#"
        [
          {"id": 1, "type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Video/Source", "node.description": "Webcam"
          }}},
          {"id": 2, "type": "PipeWire:Interface:Client", "info": {"props": {}}}
        ]
        "#;

        let devices = discover_devices(document);

        assert!(
            devices.is_empty(),
            "no default route may be offered when nothing can be captured"
        );
    }

    #[test]
    fn an_unnamed_node_still_gets_a_stable_identifier_and_label() {
        let document = r#"
        [
          {"id": 9, "type": "PipeWire:Interface:Node", "info": {"props": {
            "media.class": "Audio/Source"
          }}}
        ]
        "#;

        let devices = discover_devices(document);

        let node = devices
            .iter()
            .find(|device| device.id() == "pipewire-node-9")
            .expect("an unnamed node is still selectable");
        assert_eq!(node.name(), "PipeWire node 9");
    }

    #[test]
    #[ignore = "requires a live PipeWire graph with an input device"]
    fn live_input_reads_negotiated_blocks() {
        let provider = PipeWireAudioProvider::new();
        let devices = provider.discover().expect("discover PipeWire devices");
        let input = devices
            .iter()
            .find(|device| device.kind() == AudioDeviceKind::Input)
            .expect("an input device is available");
        let format = AudioFormat::new(48_000, 2).expect("format");

        let mut capture = provider
            .open_input(input.id(), format)
            .expect("open the input");
        // Two blocks, so a partially buffered first read cannot pass.
        for index in 0..2 {
            let block = capture
                .read_block(Timestamp::ZERO, 480)
                .expect("read an audio block");
            assert_eq!(block.format(), format);
            assert_eq!(
                block.samples().len(),
                480 * usize::from(format.channels()),
                "block {index} was short"
            );
            assert!(
                block.samples().iter().all(|sample| sample.is_finite()),
                "block {index} contained non-finite samples"
            );
        }
        assert_eq!(capture.state(), AudioInputState::Running);
        capture.stop();
    }

    #[test]
    fn node_target_rejects_unstable_ids() {
        assert_eq!(node_target("pipewire-node-42"), Some("42"));
        assert_eq!(node_target("pipewire-node-name"), None);
        assert_eq!(node_target("pipewire-default"), None);
    }

    #[test]
    fn the_capture_command_keeps_its_load_bearing_argument_shape() {
        let format = AudioFormat::new(48_000, 2).expect("format");

        assert_eq!(
            cat_arguments(CatMode::Record, format, Some("51")),
            [
                "--record",
                "--raw",
                "--format",
                "f32",
                "--rate",
                "48000",
                "--channels",
                "2",
                "--target",
                "51",
                "-",
            ]
        );
    }

    #[test]
    fn the_playback_command_keeps_its_load_bearing_argument_shape() {
        let format = AudioFormat::new(44_100, 1).expect("format");

        assert_eq!(
            cat_arguments(CatMode::Playback, format, None),
            [
                "--playback",
                "--raw",
                "--format",
                "f32",
                "--rate",
                "44100",
                "--channels",
                "1",
                "-",
            ]
        );
    }

    #[test]
    fn the_stdio_operand_is_always_last() {
        // `pw-cat` takes the file operand positionally, so it has to follow the
        // options; a target appended after it would be read as the file name.
        let format = AudioFormat::new(48_000, 2).expect("format");

        for target in [None, Some("42")] {
            for mode in [CatMode::Record, CatMode::Playback] {
                let arguments = cat_arguments(mode, format, target);
                assert_eq!(
                    arguments.last().map(String::as_str),
                    Some(STDIO_OPERAND),
                    "{mode:?} with target {target:?}"
                );
            }
        }
    }
}
