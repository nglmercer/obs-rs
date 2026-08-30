//! Native camera capture through Nokhwa.
//!
//! Nokhwa owns the platform-specific V4L2, Media Foundation, and `AVFoundation`
//! details. This module only translates its device/mode/frame values into the
//! capture crate's stable IDs, capability model, and normalized RGBA frames.

use nokhwa::{
    pixel_format::RgbFormat,
    query,
    utils::{
        frame_formats, ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat,
        RequestedFormatType,
    },
    Camera, NokhwaError,
};
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
#[cfg(target_os = "windows")]
use sha2::{Digest, Sha256};

use super::{
    device::{CaptureRequest, VideoCaptureDevice},
    error::CaptureError,
    types::{
        CameraDevice, CameraMode, CameraPixelFormat, CaptureBackendCapabilities, CaptureDeviceInfo,
        CaptureKind, CapturePermission,
    },
};

/// A native Nokhwa camera with a stable capture descriptor and a normalized
/// output lifecycle.
pub struct NokhwaCaptureDevice {
    info: CaptureDeviceInfo,
    camera: Camera,
    native_mode: Option<CameraMode>,
    output_format: Option<VideoFormat>,
    frame_index: u64,
    /// Reused decoder scratch. The final RGBA buffer belongs to the returned
    /// frame, but the intermediate RGB decode does not need a fresh allocation
    /// for every camera frame.
    rgb_scratch: Vec<u8>,
}

impl NokhwaCaptureDevice {
    /// Opens a camera from a stable ID returned by [`discover_nokhwa_cameras`].
    ///
    /// The stream is not started until [`VideoCaptureDevice::start`] or
    /// [`VideoCaptureDevice::start_capture`] is called. Opening the backend is
    /// still done here because Nokhwa needs the device object to query native
    /// modes before the source-properties UI can display them.
    ///
    /// # Errors
    ///
    /// Returns a typed capture error when the ID is invalid or Nokhwa cannot
    /// open the camera.
    pub fn from_device_id(id: &str, name: &str) -> Result<Self, CaptureError> {
        let index = camera_index_from_stable_id(id)?;
        Self::from_index(id, name, index)
    }

    /// Opens a camera from an explicit Nokhwa index and stable project ID.
    ///
    /// # Errors
    ///
    /// Returns a typed capture error when the descriptor is invalid or the
    /// native backend cannot open the camera.
    pub fn from_index(id: &str, name: &str, index: CameraIndex) -> Result<Self, CaptureError> {
        let mut camera = open_camera(index)?;
        let modes = camera_modes(&mut camera);
        let info = CaptureDeviceInfo::new(id, name, CaptureKind::Camera)?.with_capabilities(
            CaptureBackendCapabilities::default()
                .with_camera_modes(modes)
                .with_reconnectable(true)
                .with_hotplug(true),
        );
        Ok(Self {
            info,
            camera,
            native_mode: None,
            output_format: None,
            frame_index: 0,
            rgb_scratch: Vec::new(),
        })
    }

    /// Returns the selected native mode after a successful start.
    #[must_use]
    pub const fn native_mode(&self) -> Option<CameraMode> {
        self.native_mode
    }

    /// Returns the number of decoded frames.
    #[must_use]
    pub const fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Updates the permission gate used before stream opening.
    pub const fn set_permission(&mut self, permission: CapturePermission) {
        self.info.set_permission(permission);
    }
}

impl VideoCaptureDevice for NokhwaCaptureDevice {
    fn info(&self) -> &CaptureDeviceInfo {
        &self.info
    }

    fn start(&mut self, format: VideoFormat) -> Result<(), CaptureError> {
        self.start_capture(CaptureRequest::output(format))
    }

    fn start_capture(&mut self, request: CaptureRequest) -> Result<(), CaptureError> {
        if self.output_format.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        match self.info.permission() {
            CapturePermission::Granted => {}
            CapturePermission::PromptRequired => return Err(CaptureError::PermissionRequired),
            CapturePermission::Denied => return Err(CaptureError::PermissionDenied),
            CapturePermission::Unavailable => return Err(CaptureError::PermissionUnavailable),
        }

        let mode = request
            .native_mode()
            .or_else(|| {
                select_mode(
                    self.info.capabilities().camera_modes(),
                    request.output_format(),
                )
            })
            .or_else(|| camera_format_to_mode(self.camera.camera_format()))
            .ok_or(CaptureError::UnsupportedFormat(request.output_format()))?;
        if !self.info.capabilities().supports_camera_mode(mode) {
            return Err(CaptureError::UnsupportedFormat(request.output_format()));
        }

        let requested = RequestedFormat::with_formats(
            RequestedFormatType::Exact(to_nokhwa_format(mode)?),
            frame_formats(),
        );
        let actual = self
            .camera
            .set_camera_requset(requested)
            .map_err(|error| map_nokhwa_format_error(&error, request.output_format()))?;
        let actual_mode = camera_format_to_mode(actual)
            .ok_or(CaptureError::UnsupportedFormat(request.output_format()))?;
        self.camera
            .open_stream()
            .map_err(|error| map_nokhwa_error(&error))?;
        self.native_mode = Some(actual_mode);
        self.output_format = Some(request.output_format());
        self.frame_index = 0;
        Ok(())
    }

    fn stop(&mut self) {
        if self.output_format.is_some() {
            let _ = self.camera.stop_stream();
        }
        self.output_format = None;
        self.native_mode = None;
    }

    fn is_running(&self) -> bool {
        self.output_format.is_some()
    }

    fn next_frame(&mut self, timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let Some(output_format) = self.output_format else {
            return Err(CaptureError::NotRunning);
        };
        let buffer = self
            .camera
            .frame()
            .map_err(|error| map_nokhwa_error(&error))?;
        let resolution = buffer.resolution();
        let source_width = usize::try_from(resolution.width())
            .map_err(|_| CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        let source_height = usize::try_from(resolution.height())
            .map_err(|_| CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        let rgb_bytes = source_width
            .checked_mul(source_height)
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        if self.rgb_scratch.len() != rgb_bytes {
            self.rgb_scratch.resize(rgb_bytes, 0);
        }
        buffer
            .decode_image_to_buffer::<RgbFormat>(&mut self.rgb_scratch)
            .map_err(|error| map_nokhwa_error(&error))?;
        let destination_width = usize::try_from(output_format.width()).unwrap_or(0);
        let destination_height = usize::try_from(output_format.height()).unwrap_or(0);
        let pixels = if source_width == destination_width && source_height == destination_height {
            rgb_to_rgba(&self.rgb_scratch)
        } else {
            // Write the normalized RGBA image directly while scaling. This
            // avoids retaining a second full-size RGBA allocation for every
            // resized camera frame.
            resize_letterbox_rgb(
                &self.rgb_scratch,
                source_width,
                source_height,
                destination_width,
                destination_height,
            )?
        };
        let frame =
            VideoFrame::new(output_format, timestamp, pixels).map_err(CaptureError::Media)?;
        self.frame_index = self
            .frame_index
            .checked_add(1)
            .ok_or(CaptureError::FrameCounterExhausted)?;
        Ok(Some(frame))
    }
}

impl Drop for NokhwaCaptureDevice {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Discovers cameras through Nokhwa and preserves every device even when its
/// mode query is temporarily unavailable. A later open/retry can refresh the
/// mode list after a camera reconnects.
///
/// # Errors
///
/// Returns a provider error when the platform camera enumeration itself is
/// unavailable. Individual devices that cannot be opened are retained with an
/// empty mode list so one busy camera does not hide the others.
pub fn discover_nokhwa_cameras() -> Result<Vec<CameraDevice>, CaptureError> {
    discover_nokhwa_cameras_with_modes(true)
}

/// Discovers camera names and stable IDs without opening every camera to
/// inspect its complete native mode table.
///
/// Device pickers call this lightweight form first. Opening a descriptor just
/// to populate a list can be surprisingly slow on V4L2, and some drivers take
/// a long time to answer the format enumeration ioctl. The selected camera's
/// modes are queried separately by [`discover_nokhwa_camera_modes`].
///
/// # Errors
///
/// Returns a typed error when the platform camera enumeration is unavailable.
pub fn discover_nokhwa_camera_devices() -> Result<Vec<CameraDevice>, CaptureError> {
    discover_nokhwa_cameras_with_modes(false)
}

/// Queries native modes for one already-discovered camera.
///
/// This is deliberately a single-device operation so changing a camera
/// properties dropdown never reopens or probes unrelated cameras.
///
/// # Errors
///
/// Returns a typed error when the ID is invalid, the camera is disconnected,
/// or Nokhwa cannot query its native modes.
pub fn discover_nokhwa_camera_modes(id: &str) -> Result<Vec<CameraMode>, CaptureError> {
    initialize_nokhwa();
    let index = camera_index_from_stable_id(id)?;
    let mut camera = open_camera(index)?;
    Ok(camera_modes(&mut camera))
}

fn discover_nokhwa_cameras_with_modes(
    include_modes: bool,
) -> Result<Vec<CameraDevice>, CaptureError> {
    initialize_nokhwa();
    let devices = query(ApiBackend::Auto).map_err(|error| map_nokhwa_error(&error))?;
    let mut cameras = devices
        .into_iter()
        .filter_map(|device| {
            let metadata = device.misc();
            let id = stable_camera_id(device.index(), &metadata);
            let name = non_empty_name(device.human_name(), device.description());
            let modes = if include_modes {
                Camera::new(
                    device.index().clone(),
                    RequestedFormat::with_formats(
                        RequestedFormatType::AbsoluteHighestFrameRate,
                        frame_formats(),
                    ),
                )
                .ok()
                .map(|mut camera| camera_modes(&mut camera))
                .unwrap_or_default()
            } else {
                Vec::new()
            };
            CameraDevice::new(&id, &name, modes).ok()
        })
        .collect::<Vec<_>>();
    cameras.sort_by(|left, right| left.id().cmp(right.id()));
    cameras.dedup_by(|left, right| left.id() == right.id());
    Ok(cameras)
}

fn open_camera(index: CameraIndex) -> Result<Camera, CaptureError> {
    initialize_nokhwa();
    Camera::new(
        index,
        RequestedFormat::with_formats(
            RequestedFormatType::AbsoluteHighestFrameRate,
            frame_formats(),
        ),
    )
    .map_err(|error| map_nokhwa_error(&error))
}

fn camera_modes(camera: &mut Camera) -> Vec<CameraMode> {
    let mut modes = camera
        .compatible_camera_formats()
        .unwrap_or_default()
        .into_iter()
        .filter_map(camera_format_to_mode)
        .collect::<Vec<_>>();
    if modes.is_empty() {
        if let Some(mode) = camera_format_to_mode(camera.camera_format()) {
            modes.push(mode);
        }
    }
    modes.sort_unstable();
    modes.dedup();
    modes
}

fn camera_format_to_mode(format: CameraFormat) -> Option<CameraMode> {
    let pixel_format = from_nokhwa_format(format.format());
    let frame_rate = FrameRate::new(format.frame_rate(), 1).ok()?;
    CameraMode::new(pixel_format, format.width(), format.height(), frame_rate).ok()
}

fn to_nokhwa_format(mode: CameraMode) -> Result<CameraFormat, CaptureError> {
    if mode.frame_rate().denominator() != 1 {
        return Err(CaptureError::InvalidDevice {
            reason: format!(
                "Nokhwa cannot represent rational camera FPS {}/{}",
                mode.frame_rate().numerator(),
                mode.frame_rate().denominator()
            ),
        });
    }
    Ok(CameraFormat::new_from(
        mode.width(),
        mode.height(),
        to_nokhwa_pixel_format(mode.pixel_format()),
        mode.frame_rate().numerator() / mode.frame_rate().denominator(),
    ))
}

fn to_nokhwa_pixel_format(format: CameraPixelFormat) -> FrameFormat {
    match format {
        CameraPixelFormat::Mjpeg => FrameFormat::MJPEG,
        CameraPixelFormat::Yuyv => FrameFormat::YUYV,
        CameraPixelFormat::Nv12 => FrameFormat::NV12,
        CameraPixelFormat::Gray => FrameFormat::GRAY,
        CameraPixelFormat::Rgb => FrameFormat::RAWRGB,
        CameraPixelFormat::Bgr => FrameFormat::RAWBGR,
    }
}

fn from_nokhwa_format(format: FrameFormat) -> CameraPixelFormat {
    match format {
        FrameFormat::MJPEG => CameraPixelFormat::Mjpeg,
        FrameFormat::YUYV => CameraPixelFormat::Yuyv,
        FrameFormat::NV12 => CameraPixelFormat::Nv12,
        FrameFormat::GRAY => CameraPixelFormat::Gray,
        FrameFormat::RAWRGB => CameraPixelFormat::Rgb,
        FrameFormat::RAWBGR => CameraPixelFormat::Bgr,
    }
}

fn select_mode(modes: &[CameraMode], output: VideoFormat) -> Option<CameraMode> {
    modes.iter().copied().min_by_key(|mode| {
        let covers = mode.width() >= output.width() && mode.height() >= output.height();
        let width_delta = i64::from(mode.width()) - i64::from(output.width());
        let height_delta = i64::from(mode.height()) - i64::from(output.height());
        let fps_delta = i64::from(mode.frame_rate().numerator())
            * i64::from(output.frame_rate().denominator())
            - i64::from(output.frame_rate().numerator())
                * i64::from(mode.frame_rate().denominator());
        (
            !covers,
            u64::from(mode.width()) * u64::from(mode.height()),
            width_delta.unsigned_abs() + height_delta.unsigned_abs(),
            fps_delta.unsigned_abs(),
            *mode,
        )
    })
}

fn stable_camera_id(index: &CameraIndex, metadata: &str) -> String {
    #[cfg(target_os = "windows")]
    if !metadata.trim().is_empty() {
        // MSMF symbolic links are stable across device-order changes, but can
        // be much longer than the 64-byte project identifier limit. Keep the
        // persisted value bounded and resolve it back to the current catalog
        // by digest when a camera is opened.
        return format!(
            "nokhwa-camera-msmf-hash-{}",
            hex_encode(&camera_identity_digest(metadata))
        );
    }

    #[cfg(not(target_os = "windows"))]
    let _ = metadata;

    stable_camera_index_id(index)
}

fn stable_camera_index_id(index: &CameraIndex) -> String {
    match index {
        CameraIndex::Index(number) => format!("nokhwa-camera-{number}"),
        CameraIndex::String(value) => {
            let safe = value
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                        character
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            format!("nokhwa-camera-{}", safe.trim_matches('_'))
        }
    }
}

fn camera_index_from_stable_id(id: &str) -> Result<CameraIndex, CaptureError> {
    // Accept the old Linux spelling so existing projects migrate by opening
    // through Nokhwa, but never emit it for newly discovered cameras.
    if let Some(value) = id.strip_prefix("v4l2-video") {
        return value
            .parse::<u32>()
            .map(CameraIndex::Index)
            .map_err(|_| invalid_id(id));
    }
    if let Some(value) = id.strip_prefix("nokhwa-camera-") {
        #[cfg(target_os = "windows")]
        if let Some(digest) = value
            .strip_prefix("msmf-hash-")
            .and_then(|value| decode_hex_exact::<16>(value).ok())
        {
            return camera_index_from_msmf_digest(id, digest);
        }
        if let Some(encoded) = value.strip_prefix("msmf-") {
            let bytes = decode_hex(encoded).map_err(|()| invalid_id(id))?;
            let metadata = String::from_utf8(bytes).map_err(|_| invalid_id(id))?;
            if metadata.trim().is_empty() {
                return Err(invalid_id(id));
            }
            return Ok(CameraIndex::String(metadata));
        }
        if let Ok(index) = value.parse::<u32>() {
            return Ok(CameraIndex::Index(index));
        }
        if !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Ok(CameraIndex::String(value.to_owned()));
        }
    }
    Err(invalid_id(id))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ()> {
    if value.is_empty() || !value.len().is_multiple_of(2) {
        return Err(());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|_| ())?;
            u8::from_str_radix(text, 16).map_err(|_| ())
        })
        .collect()
}

fn decode_hex_exact<const N: usize>(value: &str) -> Result<[u8; N], ()> {
    decode_hex(value)?.try_into().map_err(|_| ())
}

#[cfg(target_os = "windows")]
fn camera_identity_digest(metadata: &str) -> [u8; 16] {
    let digest = Sha256::digest(metadata.as_bytes());
    let mut identity = [0_u8; 16];
    identity.copy_from_slice(&digest[..16]);
    identity
}

#[cfg(target_os = "windows")]
fn camera_index_from_msmf_digest(id: &str, digest: [u8; 16]) -> Result<CameraIndex, CaptureError> {
    initialize_nokhwa();
    let devices = query(ApiBackend::Auto).map_err(|error| map_nokhwa_error(&error))?;
    devices
        .into_iter()
        .find(|device| camera_identity_digest(&device.misc()) == digest)
        .map(|device| device.index().clone())
        .ok_or_else(|| CaptureError::PlatformUnavailable {
            message: format!("Nokhwa camera target is no longer available: {id}"),
        })
}

fn invalid_id(id: &str) -> CaptureError {
    CaptureError::InvalidDevice {
        reason: format!("Nokhwa camera stable ID is invalid: {id}"),
    }
}

fn non_empty_name(name: String, description: &str) -> String {
    if !name.trim().is_empty() {
        name
    } else if !description.trim().is_empty() {
        description.to_owned()
    } else {
        "Camera".to_owned()
    }
}

fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let pixel_count = rgb.len() / 3;
    let mut rgba = vec![u8::MAX; pixel_count * 4];
    for (source, destination) in rgb.chunks_exact(3).zip(rgba.chunks_exact_mut(4)) {
        destination[..3].copy_from_slice(source);
    }
    rgba
}

/// Resizes a decoded RGB camera image into the normalized RGBA output canvas
/// without stretching its aspect ratio.
fn resize_letterbox_rgb(
    source_rgb: &[u8],
    source_width: usize,
    source_height: usize,
    destination_width: usize,
    destination_height: usize,
) -> Result<Vec<u8>, CaptureError> {
    let source_bytes = source_width
        .checked_mul(source_height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    if source_rgb.len() != source_bytes
        || source_width == 0
        || source_height == 0
        || destination_width == 0
        || destination_height == 0
    {
        return Err(CaptureError::Protocol {
            message: "Nokhwa camera frame geometry is invalid".to_owned(),
        });
    }
    let destination_bytes = destination_width
        .checked_mul(destination_height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    let mut output = vec![0_u8; destination_bytes];
    for pixel in output.chunks_exact_mut(4) {
        pixel[3] = u8::MAX;
    }
    let width_limited = destination_width.saturating_mul(source_height)
        <= destination_height.saturating_mul(source_width);
    let (scaled_width, scaled_height) = if width_limited {
        (
            destination_width,
            destination_width
                .saturating_mul(source_height)
                .checked_div(source_width)
                .unwrap_or(0)
                .max(1),
        )
    } else {
        (
            destination_height
                .saturating_mul(source_width)
                .checked_div(source_height)
                .unwrap_or(0)
                .max(1),
            destination_height,
        )
    };
    let offset_x = (destination_width - scaled_width) / 2;
    let offset_y = (destination_height - scaled_height) / 2;
    for destination_y in 0..scaled_height {
        let source_y = destination_y
            .saturating_mul(source_height)
            .checked_div(scaled_height)
            .unwrap_or(0)
            .min(source_height - 1);
        for destination_x in 0..scaled_width {
            let source_x = destination_x
                .saturating_mul(source_width)
                .checked_div(scaled_width)
                .unwrap_or(0)
                .min(source_width - 1);
            let source_offset = (source_y * source_width + source_x) * 3;
            let output_offset =
                ((offset_y + destination_y) * destination_width + offset_x + destination_x) * 4;
            output[output_offset..output_offset + 3]
                .copy_from_slice(&source_rgb[source_offset..source_offset + 3]);
        }
    }
    Ok(output)
}

fn map_nokhwa_error(error: &NokhwaError) -> CaptureError {
    map_nokhwa_message(error.to_string())
}

fn map_nokhwa_format_error(error: &NokhwaError, format: VideoFormat) -> CaptureError {
    let message = error.to_string();
    if is_nokhwa_format_error(error, &message) {
        CaptureError::UnsupportedFormat(format)
    } else {
        map_nokhwa_message(message)
    }
}

fn map_nokhwa_message(message: String) -> CaptureError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("permission") || lower.contains("denied") {
        CaptureError::PermissionDenied
    } else {
        CaptureError::PlatformUnavailable { message }
    }
}

fn is_nokhwa_format_error(error: &NokhwaError, message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    matches!(
        error,
        NokhwaError::UnsupportedOperationError(_) | NokhwaError::NotImplementedError(_)
    ) || lower.contains("unsupported")
        || lower.contains("not supported")
        || lower.contains("no compatible")
        || (lower.contains("format")
            && (lower.contains("could not set")
                || lower.contains("failed to set")
                || lower.contains("cannot set")
                || lower.contains("negotiate")))
}

#[cfg(target_os = "macos")]
fn initialize_nokhwa() {
    nokhwa::nokhwa_initialize(|_| {});
}

#[cfg(not(target_os = "macos"))]
fn initialize_nokhwa() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(width: u32, height: u32) -> VideoFormat {
        VideoFormat::new(width, height, FrameRate::new(30, 1).expect("rate")).expect("format")
    }

    #[test]
    fn stable_linux_ids_round_trip_to_nokhwa_indices() {
        assert_eq!(
            stable_camera_id(&CameraIndex::Index(3), ""),
            "nokhwa-camera-3"
        );
        let id = "nokhwa-camera-3";
        assert_eq!(camera_index_from_stable_id(id), Ok(CameraIndex::Index(3)));
        assert_eq!(
            camera_index_from_stable_id("v4l2-video3"),
            Ok(CameraIndex::Index(3))
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn media_foundation_symbolic_links_use_a_bounded_stable_identity() {
        let symbolic_link = r"\\?\usb#vid_046d&pid_0825#camera#{7f1f}";
        let id = stable_camera_id(&CameraIndex::Index(0), symbolic_link);
        assert!(id.starts_with("nokhwa-camera-msmf-hash-"));
        assert!(id.len() <= obs_rs_util::MAX_IDENTIFIER_BYTES);
        let digest = id
            .strip_prefix("nokhwa-camera-msmf-hash-")
            .and_then(|value| decode_hex_exact::<16>(value).ok())
            .expect("bounded MSMF digest");
        assert_eq!(digest, camera_identity_digest(symbolic_link));
    }

    #[test]
    fn legacy_media_foundation_identity_still_decodes() {
        let symbolic_link = r"\\?\usb#vid_046d&pid_0825#camera#{7f1f}";
        let id = format!(
            "nokhwa-camera-msmf-{}",
            hex_encode(symbolic_link.as_bytes())
        );
        assert_eq!(
            camera_index_from_stable_id(&id),
            Ok(CameraIndex::String(symbolic_link.to_owned()))
        );
    }

    #[test]
    fn mode_selection_avoids_upscaling_when_a_larger_native_mode_exists() {
        let modes = [
            CameraMode::new(
                CameraPixelFormat::Mjpeg,
                320,
                240,
                FrameRate::new(30, 1).expect("rate"),
            )
            .expect("mode"),
            CameraMode::new(
                CameraPixelFormat::Mjpeg,
                1280,
                720,
                FrameRate::new(30, 1).expect("rate"),
            )
            .expect("mode"),
        ];
        assert_eq!(select_mode(&modes, output(640, 360)), Some(modes[1]));
    }

    #[test]
    fn rgb_frames_gain_opaque_alpha() {
        assert_eq!(
            rgb_to_rgba(&[1, 2, 3, 4, 5, 6]),
            vec![1, 2, 3, 255, 4, 5, 6, 255]
        );
    }

    #[test]
    fn resized_rgb_frames_are_letterboxed_with_opaque_alpha() {
        assert_eq!(
            resize_letterbox_rgb(&[1, 2, 3, 4, 5, 6], 2, 1, 4, 4).expect("resize"),
            vec![
                0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 1, 2, 3, 255, 1, 2, 3, 255,
                4, 5, 6, 255, 4, 5, 6, 255, 1, 2, 3, 255, 1, 2, 3, 255, 4, 5, 6, 255, 4, 5, 6, 255,
                0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255,
            ]
        );
    }

    #[test]
    fn unsupported_native_mode_errors_are_classified_for_fallback() {
        let format = output(1280, 720);
        assert!(is_nokhwa_format_error(
            &NokhwaError::GeneralError("camera format is not supported".to_owned()),
            "camera format is not supported"
        ));
        assert!(matches!(
            map_nokhwa_format_error(
                &NokhwaError::GeneralError("no compatible camera mode".to_owned()),
                format
            ),
            CaptureError::UnsupportedFormat(actual) if actual == format
        ));
    }

    #[test]
    fn unrelated_native_mode_errors_remain_platform_failures() {
        let format = output(1280, 720);
        assert!(matches!(
            map_nokhwa_format_error(
                &NokhwaError::GeneralError("camera disconnected".to_owned()),
                format
            ),
            CaptureError::PlatformUnavailable { message }
                if message == "Error: camera disconnected"
        ));
    }
}
