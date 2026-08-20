//! Typed values behind the Appearance, Video, and Output settings pages.
//!
//! These live beside [`AppSettings`](crate::settings::AppSettings) rather than
//! inside it so the document type stays a list of fields: the invariants that
//! belong to a value — which font sizes are usable, which metrics a density
//! produces, what a quality preset means in bitrate — belong to the value.
//!
//! Every type here persists through a stable string identifier rather than a
//! combo-box index, so reordering a dropdown cannot silently rewrite a stored
//! document.

use obs_rs_media::{FrameRate, ScaleFilter, VideoFormat};

use crate::UiMetrics;

/// The visual style applied on top of the selected theme.
///
/// A theme picks the colours; a style decides how they are used. Each variant
/// transforms the preset's tokens, so a style is never a name the widgets
/// branch on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum UiStyle {
    /// The theme exactly as authored.
    #[default]
    Default,
    /// Panels merge into the window and borders recede.
    Flat,
    /// Text, borders, and the accent are pushed apart for legibility.
    Contrast,
}

impl UiStyle {
    /// Every style, in the order the Appearance page offers them.
    pub(crate) const ALL: [Self; 3] = [Self::Default, Self::Flat, Self::Contrast];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Flat => "flat",
            Self::Contrast => "contrast",
        }
    }

    pub(crate) fn from_id(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "flat" => Some(Self::Flat),
            "contrast" => Some(Self::Contrast),
            _ => None,
        }
    }
}

/// How much space the settings window gives each control.
///
/// Density changes one metric set that every page reads, which is what keeps
/// the pages free of per-density branches.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum UiDensity {
    /// The tightest rows, for small displays.
    Classic,
    /// Tighter than the default without crowding.
    Compact,
    /// The default OBS-like geometry.
    #[default]
    Normal,
    /// Extra breathing room, for touch and high-DPI displays.
    Comfortable,
}

impl UiDensity {
    /// Every density, in the order the Appearance page offers them.
    pub(crate) const ALL: [Self; 4] = [
        Self::Classic,
        Self::Compact,
        Self::Normal,
        Self::Comfortable,
    ];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Compact => "compact",
            Self::Normal => "normal",
            Self::Comfortable => "comfortable",
        }
    }

    pub(crate) fn from_id(value: &str) -> Option<Self> {
        match value {
            "classic" => Some(Self::Classic),
            "compact" => Some(Self::Compact),
            "normal" => Some(Self::Normal),
            "comfortable" => Some(Self::Comfortable),
            _ => None,
        }
    }

    /// Returns `(row, control, label column, page gap, row gap, nav row)`.
    const fn geometry(self) -> (f32, f32, f32, f32, f32, f32) {
        match self {
            Self::Classic => (26.0, 26.0, 190.0, 10.0, 4.0, 26.0),
            Self::Compact => (28.0, 28.0, 200.0, 12.0, 6.0, 28.0),
            Self::Normal => (32.0, 32.0, 210.0, 14.0, 8.0, 32.0),
            Self::Comfortable => (38.0, 36.0, 224.0, 18.0, 12.0, 36.0),
        }
    }
}

/// The smallest and largest font size the Appearance page accepts.
pub(crate) const FONT_SIZE_RANGE: std::ops::RangeInclusive<u8> = 8..=18;

/// The font size the window uses when nothing has been chosen.
pub(crate) const DEFAULT_FONT_SIZE: u8 = 12;

/// The fixed width of the settings window's category list.
pub(crate) const SIDEBAR_WIDTH: f32 = 180.0;

/// The margin between the page area and the window edge.
pub(crate) const CONTENT_MARGIN: f32 = 16.0;

/// Builds the metric set for a density and font size.
///
/// Controls grow with the font rather than staying at a fixed height, because
/// a larger font inside a 32-pixel combo box clips its own descenders.
pub(crate) fn metrics(density: UiDensity, font_size: u8) -> UiMetrics {
    let font = f32::from(font_size.clamp(*FONT_SIZE_RANGE.start(), *FONT_SIZE_RANGE.end()));
    let growth = (font - f32::from(DEFAULT_FONT_SIZE)).max(0.0) * 2.0;
    let (row, control, label, page_gap, row_gap, nav) = density.geometry();
    UiMetrics {
        row_height: row + growth,
        control_height: control + growth,
        label_width: label + growth,
        page_spacing: page_gap,
        group_spacing: row_gap,
        sidebar_row_height: nav + growth,
        sidebar_width: SIDEBAR_WIDTH,
        content_margin: CONTENT_MARGIN,
        font_size: font,
        small_font_size: (font - 1.0).max(8.0),
        title_font_size: font + 1.0,
    }
}

/// Which of the two output presentations the Output page shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum OutputMode {
    /// Bitrate, encoder, and quality presets, as OBS's Simple mode.
    #[default]
    Simple,
    /// The full encoder configuration the engine accepts.
    Advanced,
}

impl OutputMode {
    pub(crate) const ALL: [Self; 2] = [Self::Simple, Self::Advanced];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::Advanced => "advanced",
        }
    }

    pub(crate) fn from_id(value: &str) -> Option<Self> {
        match value {
            "simple" => Some(Self::Simple),
            "advanced" => Some(Self::Advanced),
            _ => None,
        }
    }
}

/// The recording quality presets offered in Simple output mode.
///
/// Each preset resolves to a real encoder configuration in
/// [`AppSettings::recording_video_encoder`], so choosing one changes what is
/// written rather than only what the page says.
///
/// [`AppSettings::recording_video_encoder`]: crate::settings::AppSettings::recording_video_encoder
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RecordingQuality {
    /// Reuse the streaming encoder settings exactly.
    SameAsStream,
    /// Roughly twice the streaming bitrate.
    #[default]
    HighQuality,
    /// Roughly four times the streaming bitrate.
    IndistinguishableQuality,
    /// The lossless reference codec, which forces the OBS-RS packet format.
    Lossless,
}

impl RecordingQuality {
    pub(crate) const ALL: [Self; 4] = [
        Self::SameAsStream,
        Self::HighQuality,
        Self::IndistinguishableQuality,
        Self::Lossless,
    ];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::SameAsStream => "stream",
            Self::HighQuality => "high",
            Self::IndistinguishableQuality => "indistinguishable",
            Self::Lossless => "lossless",
        }
    }

    pub(crate) fn from_id(value: &str) -> Option<Self> {
        match value {
            "stream" => Some(Self::SameAsStream),
            "high" => Some(Self::HighQuality),
            "indistinguishable" => Some(Self::IndistinguishableQuality),
            "lossless" => Some(Self::Lossless),
            _ => None,
        }
    }

    /// Returns the video bitrate this preset asks for at `format`.
    ///
    /// The reference bitrates are quoted at 1080p, so they scale with the
    /// encoded pixel count: a 720p recording at the 1080p bitrate would spend
    /// bandwidth it cannot use.
    ///
    /// `None` means the preset does not set a bitrate — the stream's own value
    /// is used for `SameAsStream`, and the lossless codec ignores bitrate.
    pub(crate) fn video_bitrate_kbps(self, format: VideoFormat) -> Option<u32> {
        let reference = match self {
            Self::SameAsStream | Self::Lossless => return None,
            Self::HighQuality => 12_000_u64,
            Self::IndistinguishableQuality => 24_000,
        };
        let pixels = u64::from(format.width()) * u64::from(format.height());
        let scaled = reference * pixels / (1_920 * 1_080);
        Some(u32::try_from(scaled.max(1_000)).unwrap_or(u32::MAX))
    }

    /// Returns whether the preset requires the lossless reference pipeline.
    pub(crate) const fn is_lossless(self) -> bool {
        matches!(self, Self::Lossless)
    }
}

/// How the frame rate is expressed on the Video page.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FpsMode {
    /// One of the rates OBS lists, including the NTSC fractions.
    #[default]
    Common,
    /// A whole number of frames per second.
    Integer,
    /// An explicit numerator and denominator.
    Fractional,
}

impl FpsMode {
    pub(crate) const ALL: [Self; 3] = [Self::Common, Self::Integer, Self::Fractional];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Integer => "integer",
            Self::Fractional => "fractional",
        }
    }

    pub(crate) fn from_id(value: &str) -> Option<Self> {
        match value {
            "common" => Some(Self::Common),
            "integer" => Some(Self::Integer),
            "fractional" => Some(Self::Fractional),
            _ => None,
        }
    }
}

/// The largest canvas or output edge the settings window accepts.
///
/// The renderer's own budget is a pixel count; this bound keeps a typed value
/// from overflowing the arithmetic that checks it.
pub(crate) const MAX_DIMENSION: u32 = 8_192;

/// The canvas, the encoded output, and the frame rate, as three separate
/// values.
///
/// The output resolution used to be an alias for the canvas. Keeping them
/// apart is what makes "render at 1080p, stream at 720p" expressible, and the
/// scaling filter is meaningful only because they can differ.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VideoSettings {
    pub(crate) base_width: u32,
    pub(crate) base_height: u32,
    pub(crate) output_width: u32,
    pub(crate) output_height: u32,
    pub(crate) scale_filter: ScaleFilter,
    pub(crate) fps_mode: FpsMode,
    pub(crate) fps_numerator: u32,
    pub(crate) fps_denominator: u32,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            base_width: 1_920,
            base_height: 1_080,
            output_width: 1_280,
            output_height: 720,
            scale_filter: ScaleFilter::Bicubic,
            fps_mode: FpsMode::Common,
            fps_numerator: 60,
            fps_denominator: 1,
        }
    }
}

impl VideoSettings {
    /// Returns the frame rate, falling back to 60 fps for an unusable pair.
    pub(crate) fn frame_rate(self) -> FrameRate {
        FrameRate::new(self.fps_numerator, self.fps_denominator).unwrap_or_else(|_| {
            FrameRate::new(60, 1)
                .unwrap_or_else(|error| unreachable!("60 fps is a valid frame rate: {error}"))
        })
    }

    /// Returns the canvas format the renderer draws at.
    ///
    /// # Errors
    ///
    /// Returns the media error when the stored canvas cannot be a format.
    pub(crate) fn base_format(self) -> Result<VideoFormat, obs_rs_media::MediaError> {
        VideoFormat::new(self.base_width, self.base_height, self.frame_rate())
    }

    /// Returns the format the encoders receive after scaling.
    ///
    /// # Errors
    ///
    /// Returns the media error when the stored output size cannot be a format.
    pub(crate) fn output_format(self) -> Result<VideoFormat, obs_rs_media::MediaError> {
        VideoFormat::new(self.output_width, self.output_height, self.frame_rate())
    }

    /// Returns whether the encoders receive the canvas unscaled.
    pub(crate) const fn is_unscaled(self) -> bool {
        self.base_width == self.output_width && self.base_height == self.output_height
    }
}

/// Parses `1920x1080` into a validated pair of dimensions.
///
/// `x` and `×` are both accepted because the second is what a locale-aware
/// paste produces, and rejecting it would look like the field is broken.
pub(crate) fn parse_resolution(value: &str) -> Option<(u32, u32)> {
    let trimmed = value.trim();
    let (width, height) = trimmed
        .split_once(['x', 'X', '×'])
        .map(|(width, height)| (width.trim(), height.trim()))?;
    let width = width.parse::<u32>().ok()?;
    let height = height.parse::<u32>().ok()?;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return None;
    }
    // The renderer's pixel budget is the same one `VideoFormat` enforces, so a
    // resolution that would be rejected downstream is rejected in the field.
    let pixels = usize::try_from(width).ok()? * usize::try_from(height).ok()?;
    (pixels <= VideoFormat::MAX_PIXELS).then_some((width, height))
}

/// Formats a resolution the way the editable combo boxes show it.
pub(crate) fn resolution_text(width: u32, height: u32) -> String {
    format!("{width}x{height}")
}

/// Returns `16:9` for a resolution, reduced by the greatest common divisor.
///
/// A resolution with no tidy ratio keeps its decimal form rather than showing
/// something like `853:480`, which reads as noise.
pub(crate) fn aspect_ratio_text(width: u32, height: u32) -> String {
    if width == 0 || height == 0 {
        return String::new();
    }
    let divisor = greatest_common_divisor(width, height);
    let (ratio_width, ratio_height) = (width / divisor, height / divisor);
    if ratio_width <= 64 && ratio_height <= 64 {
        return format!("{ratio_width}:{ratio_height}");
    }
    let value = f64::from(width) / f64::from(height);
    format!("{value:.2}:1")
}

const fn greatest_common_divisor(left: u32, right: u32) -> u32 {
    let (mut left, mut right) = (left, right);
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 {
        1
    } else {
        left
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Metrics are lengths, so they are compared with a tolerance far below
    /// one device pixel rather than for bit equality.
    fn is_close(left: f32, right: f32) -> bool {
        (left - right).abs() < 0.001
    }

    #[test]
    fn resolutions_parse_only_when_both_edges_are_usable() {
        assert_eq!(parse_resolution(" 1920x1080 "), Some((1_920, 1_080)));
        assert_eq!(parse_resolution("1280X720"), Some((1_280, 720)));
        assert_eq!(parse_resolution("1280×720"), Some((1_280, 720)));
        assert_eq!(parse_resolution("0x720"), None);
        assert_eq!(parse_resolution("1280x0"), None);
        assert_eq!(parse_resolution("1280"), None);
        assert_eq!(parse_resolution("-4x8"), None);
        assert_eq!(parse_resolution("99999x99999"), None);
        assert_eq!(parse_resolution("4294967296x8"), None);
    }

    #[test]
    fn aspect_ratios_reduce_to_the_familiar_form() {
        assert_eq!(aspect_ratio_text(1_920, 1_080), "16:9");
        assert_eq!(aspect_ratio_text(1_280, 720), "16:9");
        assert_eq!(aspect_ratio_text(1_024, 768), "4:3");
        assert_eq!(aspect_ratio_text(854, 480), "1.78:1");
        assert_eq!(aspect_ratio_text(0, 1_080), "");
    }

    #[test]
    fn every_value_type_round_trips_through_its_identifier() {
        for style in UiStyle::ALL {
            assert_eq!(UiStyle::from_id(style.id()), Some(style));
        }
        for density in UiDensity::ALL {
            assert_eq!(UiDensity::from_id(density.id()), Some(density));
        }
        for mode in OutputMode::ALL {
            assert_eq!(OutputMode::from_id(mode.id()), Some(mode));
        }
        for quality in RecordingQuality::ALL {
            assert_eq!(RecordingQuality::from_id(quality.id()), Some(quality));
        }
        for mode in FpsMode::ALL {
            assert_eq!(FpsMode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(UiStyle::from_id("neon"), None);
        assert_eq!(UiDensity::from_id("roomy"), None);
    }

    #[test]
    fn density_and_font_size_drive_one_metric_set() {
        let compact = metrics(UiDensity::Compact, DEFAULT_FONT_SIZE);
        let comfortable = metrics(UiDensity::Comfortable, DEFAULT_FONT_SIZE);
        assert!(comfortable.row_height > compact.row_height);
        assert!(comfortable.page_spacing > compact.page_spacing);
        assert!(is_close(compact.sidebar_width, SIDEBAR_WIDTH));

        // A larger font must lift the control heights with it, or the text
        // clips inside controls sized for the default.
        let large = metrics(UiDensity::Normal, 18);
        let normal = metrics(UiDensity::Normal, DEFAULT_FONT_SIZE);
        assert!(large.control_height > normal.control_height);
        assert!(is_close(large.font_size, 18.0));

        // Out-of-range values are clamped rather than producing a broken
        // layout, because the document is not the only writer of this field.
        assert!(is_close(metrics(UiDensity::Normal, 2).font_size, 8.0));
        assert!(is_close(metrics(UiDensity::Normal, 240).font_size, 18.0));
    }

    #[test]
    fn recording_quality_scales_its_bitrate_with_the_encoded_pixels() {
        let rate = FrameRate::new(60, 1).expect("frame rate");
        let full = VideoFormat::new(1_920, 1_080, rate).expect("1080p");
        let half = VideoFormat::new(1_280, 720, rate).expect("720p");

        assert_eq!(
            RecordingQuality::HighQuality.video_bitrate_kbps(full),
            Some(12_000)
        );
        let scaled = RecordingQuality::HighQuality
            .video_bitrate_kbps(half)
            .expect("a bitrate for 720p");
        assert!(scaled < 12_000 && scaled > 1_000, "720p bitrate: {scaled}");
        assert_eq!(
            RecordingQuality::SameAsStream.video_bitrate_kbps(full),
            None
        );
        assert!(RecordingQuality::Lossless.is_lossless());
    }

    #[test]
    fn video_settings_keep_the_canvas_and_the_output_apart() {
        let video = VideoSettings::default();

        assert!(!video.is_unscaled());
        assert_eq!(video.base_format().expect("canvas").width(), 1_920);
        assert_eq!(video.output_format().expect("output").width(), 1_280);
        assert_eq!(video.frame_rate().numerator(), 60);

        let unscaled = VideoSettings {
            output_width: 1_920,
            output_height: 1_080,
            ..video
        };
        assert!(unscaled.is_unscaled());
    }

    #[test]
    fn an_unusable_frame_rate_falls_back_rather_than_panicking() {
        let video = VideoSettings {
            fps_numerator: 0,
            fps_denominator: 0,
            ..VideoSettings::default()
        };

        assert_eq!(video.frame_rate().numerator(), 60);
        assert_eq!(video.frame_rate().denominator(), 1);
    }
}
