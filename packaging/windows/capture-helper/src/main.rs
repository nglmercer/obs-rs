//! Standalone Windows Graphics Capture/Media Foundation process.
//!
//! This manifest deliberately owns its own Cargo workspace. The main OBS-RS
//! workspace remains free of Windows-native source and receives only bounded
//! OBSRWIN1 discovery text and OBSFRM01 RGBA packets over stdout.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    collections::HashSet,
    error::Error,
    io::{self, BufWriter, Write},
    process, thread,
    time::{Duration, Instant},
};

use obs_rs_capture::{
    discover_nokhwa_camera_devices, write_frame_packet, NokhwaCaptureDevice, VideoCaptureDevice,
};
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

const PROTOCOL: &str = "OBSRWIN1";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_DISCOVERY_RECORDS: usize = 512;

type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Debug, Default)]
struct Arguments {
    protocol: Option<String>,
    discover: bool,
    version: bool,
    device: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    fps_numerator: Option<u32>,
    fps_denominator: Option<u32>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("obs-rs-capture-windows-helper: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), BoxError> {
    let arguments = parse_arguments(std::env::args().skip(1))?;
    if arguments
        .protocol
        .as_deref()
        .is_some_and(|protocol| protocol != PROTOCOL)
    {
        return Err(invalid("unsupported helper protocol"));
    }
    if arguments.version {
        println!("{PROTOCOL}\tVERSION\t{VERSION}");
        return Ok(());
    }
    if arguments.discover {
        return discover();
    }
    let device = arguments
        .device
        .as_deref()
        .ok_or_else(|| invalid("--device is required for frame capture"))?;
    let format = requested_format(&arguments)?;
    capture_device(device, format)
}

fn parse_arguments<I>(arguments: I) -> Result<Arguments, BoxError>
where
    I: IntoIterator<Item = String>,
{
    let mut parsed = Arguments::default();
    let mut iterator = arguments.into_iter();
    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "--protocol" => parsed.protocol = Some(next_value(&mut iterator, "--protocol")?),
            "--discover" => parsed.discover = true,
            "--version" => parsed.version = true,
            "--device" => parsed.device = Some(next_value(&mut iterator, "--device")?),
            "--width" => parsed.width = Some(parse_value(&mut iterator, "--width")?),
            "--height" => parsed.height = Some(parse_value(&mut iterator, "--height")?),
            "--fps-numerator" => {
                parsed.fps_numerator = Some(parse_value(&mut iterator, "--fps-numerator")?);
            }
            "--fps-denominator" => {
                parsed.fps_denominator = Some(parse_value(&mut iterator, "--fps-denominator")?);
            }
            other => return Err(invalid(format!("unknown argument {other}"))),
        }
    }
    Ok(parsed)
}

fn next_value<I>(iterator: &mut I, flag: &str) -> Result<String, BoxError>
where
    I: Iterator<Item = String>,
{
    iterator
        .next()
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| invalid(format!("{flag} requires a value")))
}

fn parse_value<I, T>(iterator: &mut I, flag: &str) -> Result<T, BoxError>
where
    I: Iterator<Item = String>,
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    next_value(iterator, flag)?
        .parse::<T>()
        .map_err(|error| invalid(format!("invalid {flag} value: {error}")))
}

fn requested_format(arguments: &Arguments) -> Result<VideoFormat, BoxError> {
    let width = arguments
        .width
        .ok_or_else(|| invalid("--width is required for frame capture"))?;
    let height = arguments
        .height
        .ok_or_else(|| invalid("--height is required for frame capture"))?;
    let numerator = arguments
        .fps_numerator
        .ok_or_else(|| invalid("--fps-numerator is required for frame capture"))?;
    let denominator = arguments
        .fps_denominator
        .ok_or_else(|| invalid("--fps-denominator is required for frame capture"))?;
    let frame_rate = FrameRate::new(numerator, denominator)?;
    Ok(VideoFormat::new(width, height, frame_rate)?)
}

fn discover() -> Result<(), BoxError> {
    let mut stdout = BufWriter::new(io::stdout().lock());
    writeln!(stdout, "{PROTOCOL}\tDISCOVERY\t1")?;
    writeln!(stdout, "{PROTOCOL}\tVERSION\t{VERSION}")?;

    let monitors = Monitor::enumerate()?;
    let primary_name = Monitor::primary()
        .ok()
        .and_then(|monitor| monitor.device_name().ok());
    let mut remaining_records = MAX_DISCOVERY_RECORDS;
    let mut x = 0_i32;
    for monitor in monitors {
        if remaining_records == 0 {
            break;
        }
        let index = monitor.index()?;
        let device_name = monitor.device_name()?;
        let name = monitor
            .name()
            .ok()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| device_name.clone());
        let width = monitor.width()?;
        let height = monitor.height()?;
        let primary = primary_name.as_deref() == Some(device_name.as_str());
        writeln!(
            stdout,
            "screen\twgc-screen-{index}\t{}\t{x}\t0\t{width}\t{height}\t{}",
            clean_field(&name),
            i32::from(primary)
        )?;
        remaining_records -= 1;
        x = x.saturating_add(i32::try_from(width).unwrap_or(i32::MAX));
    }

    let mut window_ids = HashSet::new();
    for window in Window::enumerate()? {
        if remaining_records == 0 {
            break;
        }
        let title = window.title().unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        let process_name = window.process_name().unwrap_or_default();
        let id = window_id(window, &title, &process_name);
        if !window_ids.insert(id.clone()) {
            continue;
        }
        let name = if process_name.trim().is_empty() {
            title
        } else {
            format!("{title} ({process_name})")
        };
        writeln!(stdout, "window\t{id}\t{}", clean_field(&name))?;
        remaining_records -= 1;
    }

    if let Ok(cameras) = discover_nokhwa_camera_devices() {
        for camera in cameras {
            if remaining_records == 0 {
                break;
            }
            writeln!(
                stdout,
                "camera\t{}\t{}",
                camera.id(),
                clean_field(camera.name())
            )?;
            remaining_records -= 1;
        }
    }
    stdout.flush()?;
    Ok(())
}

fn capture_device(device: &str, format: VideoFormat) -> Result<(), BoxError> {
    if device == "media-foundation-camera-default" || device.starts_with("nokhwa-camera-") {
        return capture_camera(device, format);
    }
    if device == "wgc-window-picker" || device.starts_with("wgc-window-") {
        let window = resolve_window(device)?;
        return run_graphics_capture(window, format);
    }
    if device == "wgc-screen-picker" || device.starts_with("wgc-screen-") {
        let monitor = resolve_monitor(device)?;
        return run_graphics_capture(monitor, format);
    }
    Err(invalid(format!("unknown capture device {device}")))
}

fn resolve_monitor(device: &str) -> Result<Monitor, BoxError> {
    if device == "wgc-screen-picker" {
        return Ok(Monitor::primary()?);
    }
    let index = device
        .strip_prefix("wgc-screen-")
        .ok_or_else(|| invalid("invalid display capture ID"))?
        .parse::<usize>()?;
    Ok(Monitor::from_index(index)?)
}

fn resolve_window(device: &str) -> Result<Window, BoxError> {
    if device == "wgc-window-picker" {
        return Ok(Window::foreground()?);
    }
    let windows = Window::enumerate()?;
    windows
        .into_iter()
        .find(|window| {
            let title = window.title().unwrap_or_default();
            let process_name = window.process_name().unwrap_or_default();
            window_id(*window, &title, &process_name) == device
        })
        .ok_or_else(|| invalid("the selected window is no longer available"))
}

fn capture_camera(device: &str, format: VideoFormat) -> Result<(), BoxError> {
    let cameras = discover_nokhwa_camera_devices()?;
    let camera = if device == "media-foundation-camera-default" {
        cameras.first()
    } else {
        cameras.iter().find(|camera| camera.id().as_str() == device)
    }
    .ok_or_else(|| invalid("the selected camera is no longer available"))?;
    let mut capture = NokhwaCaptureDevice::from_device_id(camera.id().as_str(), camera.name())?;
    capture.start(format)?;
    let start = Instant::now();
    let mut stdout = BufWriter::new(io::stdout().lock());
    loop {
        if let Some(frame) = capture.next_frame(elapsed_timestamp(start))? {
            write_frame_packet(&frame, &mut stdout)?;
            stdout.flush()?;
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn run_graphics_capture<T>(item: T, format: VideoFormat) -> Result<(), BoxError>
where
    T: TryInto<windows_capture::settings::GraphicsCaptureItemType> + Send + 'static,
    T::Error: Error + Send + Sync + 'static,
{
    let settings = Settings::new(
        item,
        // Keep optional Graphics Capture toggles at their OS defaults. Older
        // Windows builds expose the capture API but reject attempts to toggle
        // cursor, border, or secondary-window behavior during session setup.
        CursorCaptureSettings::Default,
        DrawBorderSettings::Default,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        format,
    );
    FrameWriter::start(settings)
        .map_err(|error| invalid(format!("start graphics capture: {error}")))
}

struct FrameWriter {
    format: VideoFormat,
    writer: BufWriter<io::Stdout>,
    scratch: Vec<u8>,
    started: Instant,
}

impl GraphicsCaptureApiHandler for FrameWriter {
    type Flags = VideoFormat;
    type Error = BoxError;

    fn new(context: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            format: context.flags,
            writer: BufWriter::new(io::stdout()),
            scratch: Vec::new(),
            started: Instant::now(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let (source_width, source_height, source_pixels) = {
            let buffer = frame.buffer()?;
            let width = buffer.width();
            let height = buffer.height();
            let pixels = buffer.as_nopadding_buffer(&mut self.scratch).to_vec();
            (width, height, pixels)
        };
        let pixels = resize_rgba(
            &source_pixels,
            source_width,
            source_height,
            self.format.width(),
            self.format.height(),
        )?;
        let output = VideoFrame::new(self.format, elapsed_timestamp(self.started), pixels)?;
        write_frame_packet(&output, &mut self.writer)?;
        self.writer.flush()?;
        Ok(())
    }
}

fn resize_rgba(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>, BoxError> {
    let source_size = usize::try_from(source_width)
        .ok()
        .and_then(|width| {
            usize::try_from(source_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| invalid("captured frame dimensions overflow"))?;
    if source.len() < source_size {
        return Err(invalid(
            "captured frame payload is shorter than its dimensions",
        ));
    }
    let target_size = usize::try_from(target_width)
        .ok()
        .and_then(|width| {
            usize::try_from(target_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| invalid("output frame dimensions overflow"))?;
    if source_width == target_width && source_height == target_height {
        return Ok(source[..source_size].to_vec());
    }
    let mut target = vec![0_u8; target_size];
    for y in 0..target_height {
        let source_y = y.saturating_mul(source_height) / target_height;
        for x in 0..target_width {
            let source_x = x.saturating_mul(source_width) / target_width;
            let source_index = (usize::try_from(source_y).unwrap_or(0)
                * usize::try_from(source_width).unwrap_or(0)
                + usize::try_from(source_x).unwrap_or(0))
                * 4;
            let target_index = (usize::try_from(y).unwrap_or(0)
                * usize::try_from(target_width).unwrap_or(0)
                + usize::try_from(x).unwrap_or(0))
                * 4;
            target[target_index..target_index + 4]
                .copy_from_slice(&source[source_index..source_index + 4]);
        }
    }
    Ok(target)
}

fn window_id(window: Window, title: &str, process_name: &str) -> String {
    let process_id = window.process_id().unwrap_or_default();
    let key = format!("{process_id}:{process_name}:{title}");
    format!("wgc-window-{:016x}", stable_hash(&key))
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1_099_511_628_211_u64);
    }
    hash
}

fn elapsed_timestamp(start: Instant) -> Timestamp {
    Timestamp::from_nanos(u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX))
}

fn clean_field(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\t' | '\r' | '\n' => ' ',
            other => other,
        })
        .collect()
}

fn invalid(message: impl Into<String>) -> BoxError {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
