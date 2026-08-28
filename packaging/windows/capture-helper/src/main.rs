//! Standalone Windows Graphics Capture/Media Foundation process.
//!
//! This manifest deliberately owns its own Cargo workspace. The main OBS-RS
//! workspace remains free of Windows-native source and receives only bounded
//! OBSRWIN1 discovery text and OBSFRM01 RGBA packets over stdout.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    collections::HashMap,
    error::Error,
    io::{self, BufWriter, Read, Write},
    process,
    time::{Duration, Instant},
};

use obs_rs_capture::write_frame_packet;
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
#[cfg(target_os = "windows")]
use winsafe::{self as w, prelude::*};

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
    capture_cursor: Option<bool>,
    capture_border: Option<bool>,
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
    capture_device(
        device,
        format,
        arguments.capture_cursor,
        arguments.capture_border,
    )
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
            "--capture-cursor" => {
                parsed.capture_cursor = Some(parse_value(&mut iterator, "--capture-cursor")?);
            }
            "--capture-border" => {
                parsed.capture_border = Some(parse_value(&mut iterator, "--capture-border")?);
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
    let monitor_geometry = monitor_geometry()?;
    let mut remaining_records = MAX_DISCOVERY_RECORDS;
    let mut x = 0_i32;
    for monitor in monitors {
        if remaining_records == 0 {
            break;
        }
        let device_name = monitor.device_name()?;
        let name = monitor
            .name()
            .ok()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| device_name.clone());
        let width = monitor.width()?;
        let height = monitor.height()?;
        let (monitor_x, monitor_y, primary) = monitor_geometry
            .get(&device_name)
            .copied()
            .unwrap_or_else(|| (x, 0, primary_name.as_deref() == Some(device_name.as_str())));
        writeln!(
            stdout,
            "screen\t{}\t{}\t{monitor_x}\t{monitor_y}\t{width}\t{height}\t{}",
            monitor_id(&device_name),
            clean_field(&name),
            i32::from(primary)
        )?;
        remaining_records -= 1;
        x = x.saturating_add(i32::try_from(width).unwrap_or(i32::MAX));
    }

    let mut window_occurrences = HashMap::new();
    for window in capturable_windows()? {
        if remaining_records == 0 {
            break;
        }
        let base_id = window_id(&window.title, &window.process_name);
        let occurrence = window_occurrences.entry(base_id.clone()).or_insert(0);
        let id = window_id_with_occurrence(&base_id, *occurrence);
        *occurrence = occurrence.saturating_add(1);
        let name = if window.process_name.trim().is_empty() {
            window.title
        } else {
            format!("{} ({})", window.title, window.process_name)
        };
        writeln!(stdout, "window\t{id}\t{}", clean_field(&name))?;
        remaining_records -= 1;
    }

    stdout.flush()?;
    Ok(())
}

/// A discovery record sorted independently of the window manager's current
/// z-order. Windows with the same title and executable therefore keep their
/// occurrence suffix when another window is focused or raised.
struct CapturableWindow {
    window: Window,
    title: String,
    process_name: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn capturable_windows() -> Result<Vec<CapturableWindow>, BoxError> {
    let mut windows = Vec::new();
    for window in Window::enumerate()? {
        if !window.is_valid() {
            continue;
        }
        let title = window.title().unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        let process_name = window.process_name().unwrap_or_default();
        let Ok(width) = window.width() else {
            continue;
        };
        let Ok(height) = window.height() else {
            continue;
        };
        if width <= 0 || height <= 0 {
            continue;
        }
        let (x, y) = window.rect().map_or((0, 0), |rect| (rect.left, rect.top));
        windows.push(CapturableWindow {
            window,
            title,
            process_name,
            x,
            y,
            width: u32::try_from(width).unwrap_or(0),
            height: u32::try_from(height).unwrap_or(0),
        });
    }
    windows.sort_by(|left, right| {
        (
            window_id(&left.title, &left.process_name),
            left.y,
            left.x,
            left.width,
            left.height,
            left.window.as_raw_hwnd() as usize,
        )
            .cmp(&(
                window_id(&right.title, &right.process_name),
                right.y,
                right.x,
                right.width,
                right.height,
                right.window.as_raw_hwnd() as usize,
            ))
    });
    Ok(windows)
}

/// Returns desktop-space monitor geometry from the safe Windows API wrapper.
///
/// `windows-capture` intentionally exposes the capture handle but not the
/// monitor rectangle. Keeping this small query in the helper preserves
/// negative coordinates for left/top monitors without exposing an HMONITOR to
/// the portable workspace.
#[cfg(target_os = "windows")]
fn monitor_geometry() -> Result<HashMap<String, (i32, i32, bool)>, BoxError> {
    let mut geometry = HashMap::new();
    w::HDC::NULL.EnumDisplayMonitors(None, |monitor, _dc, rectangle| {
        let mut info = w::MONITORINFOEX::default();
        if monitor.GetMonitorInfo(&mut info).is_err() {
            return false;
        }
        geometry.insert(
            info.szDevice().clone(),
            (
                rectangle.left,
                rectangle.top,
                info.dwFlags == w::co::MONITORINFOF::PRIMARY,
            ),
        );
        true
    })?;
    Ok(geometry)
}

#[cfg(not(target_os = "windows"))]
fn monitor_geometry() -> Result<HashMap<String, (i32, i32, bool)>, BoxError> {
    Ok(HashMap::new())
}

fn capture_device(
    device: &str,
    format: VideoFormat,
    capture_cursor: Option<bool>,
    capture_border: Option<bool>,
) -> Result<(), BoxError> {
    if device == "wgc-window-picker" || device.starts_with("wgc-window-") {
        let window = resolve_window(device)?;
        return run_graphics_capture(&window, format, capture_cursor, capture_border);
    }
    if device == "wgc-screen-picker" || device.starts_with("wgc-screen-") {
        let monitor = resolve_monitor(device)?;
        return run_graphics_capture(&monitor, format, capture_cursor, capture_border);
    }
    Err(invalid(format!("unknown capture device {device}")))
}

fn resolve_monitor(device: &str) -> Result<Monitor, BoxError> {
    if device == "wgc-screen-picker" {
        return Ok(Monitor::primary()?);
    }
    let _ = device
        .strip_prefix("wgc-screen-")
        .ok_or_else(|| invalid("invalid display capture ID"))?;
    Monitor::enumerate()?
        .into_iter()
        .find(|monitor| {
            monitor
                .device_name()
                .is_ok_and(|name| monitor_id(&name) == device)
        })
        .ok_or_else(|| invalid("the selected display is no longer available"))
}

fn resolve_window(device: &str) -> Result<Window, BoxError> {
    if device == "wgc-window-picker" {
        let window = Window::foreground()?;
        if !window.is_valid()
            || !window.width().is_ok_and(|width| width > 0)
            || !window.height().is_ok_and(|height| height > 0)
        {
            return Err(invalid("the foreground window is not a capturable target"));
        }
        return Ok(window);
    }
    let windows = capturable_windows()?;
    let mut window_occurrences = HashMap::new();
    windows
        .into_iter()
        .find(|window| {
            let base_id = window_id(&window.title, &window.process_name);
            let occurrence = window_occurrences.entry(base_id.clone()).or_insert(0);
            let candidate = window_id_with_occurrence(&base_id, *occurrence);
            *occurrence = occurrence.saturating_add(1);
            candidate == device
        })
        .map(|window| window.window)
        .ok_or_else(|| invalid("the selected window is no longer available"))
}

fn run_graphics_capture<T>(
    item: &T,
    format: VideoFormat,
    capture_cursor: Option<bool>,
    capture_border: Option<bool>,
) -> Result<(), BoxError>
where
    T: TryInto<windows_capture::settings::GraphicsCaptureItemType> + Clone + Send + 'static,
    T::Error: Error + Send + Sync + 'static,
{
    let cursor_settings = match capture_cursor {
        Some(true) => CursorCaptureSettings::WithCursor,
        Some(false) => CursorCaptureSettings::WithoutCursor,
        None => CursorCaptureSettings::Default,
    };
    let border_settings = match capture_border {
        Some(true) => DrawBorderSettings::WithBorder,
        Some(false) => DrawBorderSettings::WithoutBorder,
        None => DrawBorderSettings::Default,
    };
    let start = |border_settings| {
        let settings = Settings::new(
            item.clone(),
            cursor_settings,
            border_settings,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Rgba8,
            format,
        );
        FrameWriter::start_free_threaded(settings)
    };
    let control = match start(border_settings) {
        Ok(control) => control,
        Err(error)
            if border_settings == DrawBorderSettings::WithoutBorder
                && border_toggle_is_unsupported(&error.to_string()) =>
        {
            eprintln!(
                "capture border policy is unavailable; retrying Windows Graphics Capture with the OS default"
            );
            start(DrawBorderSettings::Default)
                .map_err(|error| invalid(format!("start graphics capture: {error}")))?
        }
        Err(error) => return Err(invalid(format!("start graphics capture: {error}"))),
    };
    wait_for_parent_shutdown()?;
    control
        .stop()
        .map_err(|error| invalid(format!("stop graphics capture: {error}")))
}

fn border_toggle_is_unsupported(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("capture border") && message.contains("not supported")
}

/// The parent closes stdin when the source is stopped. Reading it on the
/// helper's main thread leaves the Graphics Capture callback free to publish
/// frames while still giving the host a graceful stop path.
fn wait_for_parent_shutdown() -> Result<(), io::Error> {
    let mut stdin = io::stdin().lock();
    let mut buffer = [0_u8; 1024];
    while stdin.read(&mut buffer)? != 0 {}
    Ok(())
}

struct FrameWriter {
    format: VideoFormat,
    writer: BufWriter<io::Stdout>,
    scratch: Vec<u8>,
    /// Reused destination storage for the normalized frame. `VideoFrame`
    /// gives it back after the packet is written, avoiding a 4K allocation on
    /// every callback while preserving the helper's safe process boundary.
    output: Vec<u8>,
    pacer: FramePacer,
    started: Instant,
}

struct FramePacer {
    interval: Duration,
    next_deadline: Instant,
}

impl FramePacer {
    fn new(format: &VideoFormat, started: Instant) -> Self {
        let interval_nanos = format.frame_rate().period_nanos().unwrap_or(1).max(1);
        Self {
            interval: Duration::from_nanos(interval_nanos),
            next_deadline: started,
        }
    }

    fn should_emit(&mut self, now: Instant) -> bool {
        if now < self.next_deadline {
            return false;
        }
        self.next_deadline = now.checked_add(self.interval).unwrap_or(now);
        true
    }
}

impl GraphicsCaptureApiHandler for FrameWriter {
    type Flags = VideoFormat;
    type Error = BoxError;

    fn new(context: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let format = context.flags;
        let started = Instant::now();
        let pacer = FramePacer::new(&format, started);
        Ok(Self {
            format,
            writer: BufWriter::new(io::stdout()),
            scratch: Vec::new(),
            output: Vec::new(),
            pacer,
            started,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if !self.pacer.should_emit(Instant::now()) {
            return Ok(());
        }
        let target_width = self.format.width();
        let target_height = self.format.height();
        let pixels = {
            let buffer = frame.buffer()?;
            let width = buffer.width();
            let height = buffer.height();
            resize_rgba_into(
                buffer.as_nopadding_buffer(&mut self.scratch),
                width,
                height,
                target_width,
                target_height,
                &mut self.output,
            )?;
            std::mem::take(&mut self.output)
        };
        let output = VideoFrame::new(self.format, elapsed_timestamp(self.started), pixels)?;
        write_frame_packet(&output, &mut self.writer)?;
        self.writer.flush()?;
        self.output = output.into_pixels();
        Ok(())
    }
}

#[cfg(test)]
fn resize_rgba(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>, BoxError> {
    let mut target = Vec::new();
    resize_rgba_into(
        source,
        source_width,
        source_height,
        target_width,
        target_height,
        &mut target,
    )?;
    Ok(target)
}

fn resize_rgba_into(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    target: &mut Vec<u8>,
) -> Result<(), BoxError> {
    if source_width == 0 || source_height == 0 {
        return Err(invalid("captured frame dimensions must be non-zero"));
    }
    if target_width == 0 || target_height == 0 {
        return Err(invalid("output frame dimensions must be non-zero"));
    }
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
        target.clear();
        target.extend_from_slice(&source[..source_size]);
        return Ok(());
    }
    let source_width = usize::try_from(source_width).unwrap_or(usize::MAX);
    let source_height = usize::try_from(source_height).unwrap_or(usize::MAX);
    let target_width = usize::try_from(target_width).unwrap_or(usize::MAX);
    let target_height = usize::try_from(target_height).unwrap_or(usize::MAX);
    target.clear();
    target.resize(target_size, 0);
    for y in 0..target_height {
        let source_y = y.saturating_mul(source_height) / target_height;
        let source_row = source_y.saturating_mul(source_width).saturating_mul(4);
        let target_row = y.saturating_mul(target_width).saturating_mul(4);
        for x in 0..target_width {
            let source_x = x.saturating_mul(source_width) / target_width;
            let source_index = source_row.saturating_add(source_x.saturating_mul(4));
            let target_index = target_row.saturating_add(x.saturating_mul(4));
            target[target_index..target_index + 4]
                .copy_from_slice(&source[source_index..source_index + 4]);
        }
    }
    Ok(())
}

fn window_id(title: &str, process_name: &str) -> String {
    // A process/title identity survives an OBS-RS restart, unlike an HWND. It
    // also deliberately stops resolving when the title changes, so a closed
    // target cannot accidentally turn into a different window that reused the
    // old native handle.
    let key = format!("{}\0{}", process_name.trim(), title.trim());
    format!("wgc-window-{:016x}", stable_hash(&key))
}

fn window_id_with_occurrence(base_id: &str, occurrence: usize) -> String {
    if occurrence == 0 {
        base_id.to_owned()
    } else {
        format!("{base_id}-{occurrence}")
    }
}

fn monitor_id(device_name: &str) -> String {
    format!("wgc-screen-{:016x}", stable_hash(device_name))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_keeps_rgba_channel_order_and_handles_padding() {
        let source = [
            10, 20, 30, 40, 50, 60, 70, 80, // row 0
            90, 100, 110, 120, 130, 140, 150, 160, // row 1
            0, 0, 0, 0, // ignored row padding
        ];
        let resized = resize_rgba(&source, 2, 2, 1, 1).expect("resize");
        assert_eq!(resized, vec![10, 20, 30, 40]);
    }

    #[test]
    fn resize_uses_nearest_source_pixels() {
        let source = [
            1, 2, 3, 4, 5, 6, 7, 8, // row 0
            9, 10, 11, 12, 13, 14, 15, 16, // row 1
        ];
        let resized = resize_rgba(&source, 2, 2, 4, 4).expect("resize");
        assert_eq!(&resized[0..4], &[1, 2, 3, 4]);
        assert_eq!(&resized[12..16], &[5, 6, 7, 8]);
        assert_eq!(&resized[48..52], &[9, 10, 11, 12]);
        assert_eq!(&resized[60..64], &[13, 14, 15, 16]);
    }

    #[test]
    fn resize_rejects_truncated_or_zero_dimension_frames() {
        assert!(resize_rgba(&[0; 3], 1, 1, 1, 1).is_err());
        assert!(resize_rgba(&[0; 4], 0, 1, 1, 1).is_err());
        assert!(resize_rgba(&[0; 4], 1, 1, 0, 1).is_err());
    }

    #[test]
    fn resize_reuses_the_caller_owned_output_buffer() {
        let source = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut output = Vec::with_capacity(32);
        let initial_capacity = output.capacity();
        resize_rgba_into(&source, 2, 1, 2, 1, &mut output).expect("first resize");
        assert_eq!(output, source);
        assert_eq!(output.capacity(), initial_capacity);

        resize_rgba_into(&source, 2, 1, 1, 1, &mut output).expect("second resize");
        assert_eq!(output, vec![1, 2, 3, 4]);
        assert_eq!(output.capacity(), initial_capacity);
    }

    #[test]
    fn frame_pacer_emits_immediately_then_waits_for_the_requested_period() {
        let format =
            VideoFormat::new(320, 180, FrameRate::new(30, 1).expect("rate")).expect("format");
        let started = Instant::now();
        let mut pacer = FramePacer::new(&format, started);
        assert!(pacer.should_emit(started));
        assert!(!pacer.should_emit(started + Duration::from_millis(10)));
        assert!(pacer.should_emit(started + Duration::from_millis(34)));
    }

    #[test]
    fn cursor_argument_is_optional_but_validated_when_present() {
        let default = parse_arguments(Vec::<String>::new()).expect("default arguments");
        assert_eq!(default.capture_cursor, None);
        assert_eq!(default.capture_border, None);
        let disabled = parse_arguments(["--capture-cursor".to_owned(), "false".to_owned()])
            .expect("cursor argument");
        assert_eq!(disabled.capture_cursor, Some(false));
        let border = parse_arguments(["--capture-border".to_owned(), "true".to_owned()])
            .expect("border argument");
        assert_eq!(border.capture_border, Some(true));
        assert!(parse_arguments(["--capture-cursor".to_owned(), "maybe".to_owned(),]).is_err());
        assert!(parse_arguments(["--capture-border".to_owned(), "maybe".to_owned(),]).is_err());
    }

    #[test]
    fn border_fallback_only_matches_an_unsupported_border_error() {
        assert!(border_toggle_is_unsupported(
            "Graphics capture error: Toggling the capture border is not supported by the Graphics Capture API on this platform."
        ));
        assert!(!border_toggle_is_unsupported(
            "the selected display is no longer available"
        ));
    }

    #[test]
    fn stable_window_ids_are_deterministic() {
        let first = window_id("OBS-RS", "obs-rs.exe");
        let second = window_id("OBS-RS", "obs-rs.exe");
        assert_eq!(first, second);
        assert!(first.starts_with("wgc-window-"));
    }

    #[test]
    fn window_identity_changes_when_the_title_or_process_changes() {
        assert_ne!(
            window_id("OBS-RS", "obs-rs.exe"),
            window_id("Other project", "obs-rs.exe")
        );
        assert_ne!(
            window_id("OBS-RS", "obs-rs.exe"),
            window_id("OBS-RS", "other.exe")
        );
    }

    #[test]
    fn duplicate_window_identities_are_disambiguated_without_changing_the_first_id() {
        let base = window_id("OBS-RS", "obs-rs.exe");
        assert_eq!(window_id_with_occurrence(&base, 0), base);
        assert_ne!(
            window_id_with_occurrence(&base, 0),
            window_id_with_occurrence(&base, 1)
        );
    }
}
