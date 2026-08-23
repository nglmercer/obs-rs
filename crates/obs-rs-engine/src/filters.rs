use obs_rs_audio::AudioFilter;
use obs_rs_media::{
    ChromaKey, ColorCorrection, ColorKey, ColorMultiplyAdd, FrameFilter, FrameTransform, LumaKey,
    RenderDelay, MAX_RENDER_DELAY_MILLISECONDS, MAX_SCROLL_SPEED, MIN_RENDER_DELAY_MILLISECONDS,
    MIN_SCROLL_SPEED,
};
use obs_rs_project::{SourceFilterCategory, SourceFilterSpec};

use super::{FilterCompilation, FilterCompileFailure, FilterDiagnostic};

/// Compiles a persistent source filter into the built-in runtime operation.
///
/// Unknown kinds, audio/video filters, disabled instances, and malformed
/// settings remain valid project data but are omitted until a matching runtime
/// implementation is available. The project crate therefore stays independent
/// of this renderer-facing enum.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "the project-to-renderer boundary keeps every supported effect mapping explicit"
)]
fn compile_frame_filter(spec: &SourceFilterSpec) -> Option<FrameFilter> {
    if !spec.enabled() || spec.category() != SourceFilterCategory::Effect {
        return None;
    }
    match spec.kind().as_str() {
        "grayscale" => Some(FrameFilter::Grayscale),
        "brightness" => spec
            .settings()
            .get("milli")
            .and_then(|value| value.parse().ok())
            .map(|milli| FrameFilter::Brightness { milli }),
        "opacity" => spec
            .settings()
            .get("value")
            .and_then(|value| value.parse().ok())
            .map(FrameFilter::Opacity),
        "crop_pad" => {
            let read_edge = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<u32>().ok())
                    .filter(|value| *value <= FrameTransform::MAX_CROP)
            };
            Some(FrameFilter::CropPad {
                left: read_edge("left")?,
                top: read_edge("top")?,
                right: read_edge("right")?,
                bottom: read_edge("bottom")?,
            })
        }
        "color_correction" => {
            let read_value = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
            };
            Some(FrameFilter::ColorCorrection(ColorCorrection::new(
                read_value("gamma")?,
                read_value("contrast")?,
                read_value("brightness")?,
                read_value("saturation")?,
                read_value("hue_shift")?,
                read_value("opacity")?,
            )?))
        }
        "color_multiply_add" => {
            let read_channel = |key| spec.settings().get(key)?.parse::<u8>().ok();
            Some(FrameFilter::ColorMultiplyAdd(ColorMultiplyAdd::new(
                [
                    read_channel("multiply_red")?,
                    read_channel("multiply_green")?,
                    read_channel("multiply_blue")?,
                ],
                [
                    read_channel("add_red")?,
                    read_channel("add_green")?,
                    read_channel("add_blue")?,
                ],
            )))
        }
        "luma_key" => {
            let read_value = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
            };
            Some(FrameFilter::LumaKey(LumaKey::new(
                read_value("luma_max")?,
                read_value("luma_min")?,
                read_value("luma_max_smooth")?,
                read_value("luma_min_smooth")?,
            )?))
        }
        "color_key" => {
            let read_channel = |key| spec.settings().get(key)?.parse::<u8>().ok();
            let read_threshold = |key| spec.settings().get(key)?.parse::<i32>().ok();
            Some(FrameFilter::ColorKey(ColorKey::new(
                read_channel("key_red")?,
                read_channel("key_green")?,
                read_channel("key_blue")?,
                read_threshold("similarity")?,
                read_threshold("smoothness")?,
            )?))
        }
        "chroma_key" => {
            let read_channel = |key| spec.settings().get(key)?.parse::<u8>().ok();
            let read_threshold = |key| spec.settings().get(key)?.parse::<i32>().ok();
            Some(FrameFilter::ChromaKey(ChromaKey::new(
                read_channel("key_red")?,
                read_channel("key_green")?,
                read_channel("key_blue")?,
                read_threshold("similarity")?,
                read_threshold("smoothness")?,
                read_threshold("spill")?,
            )?))
        }
        "sharpen" => spec
            .settings()
            .get("sharpness")
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|value| *value <= 1_000)
            .map(|milli| FrameFilter::Sharpen { milli }),
        "scroll" => {
            let read_speed = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i16>().ok())
                    .filter(|value| (MIN_SCROLL_SPEED..=MAX_SCROLL_SPEED).contains(value))
            };
            let looped = match spec.settings().get("loop") {
                None | Some("true") => true,
                Some("false") => false,
                Some(_) => return None,
            };
            Some(FrameFilter::Scroll {
                speed_x: read_speed("speed_x")?,
                speed_y: read_speed("speed_y")?,
                looped,
            })
        }
        "render_delay" => spec
            .settings()
            .get("milliseconds")
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| {
                (MIN_RENDER_DELAY_MILLISECONDS..=MAX_RENDER_DELAY_MILLISECONDS).contains(value)
            })
            .map(|milliseconds| FrameFilter::RenderDelay(RenderDelay { milliseconds })),
        _ => None,
    }
}

/// Translates a persisted video effect and preserves the reason when it is
/// unavailable. The older [`compile_filter`] helper remains as a compact
/// compatibility view for callers that only need the applied operation.
#[must_use]
pub fn compile_filter_report(spec: &SourceFilterSpec) -> FilterCompilation<FrameFilter> {
    if !spec.enabled() {
        return FilterCompilation::Ignored;
    }
    if spec.category() != SourceFilterCategory::Effect {
        return FilterCompilation::Unavailable(FilterDiagnostic::new(
            spec,
            FilterCompileFailure::UnsupportedCategory,
        ));
    }
    match compile_frame_filter(spec) {
        Some(filter) => FilterCompilation::Applied(filter),
        None => FilterCompilation::Unavailable(FilterDiagnostic::new(
            spec,
            if frame_filter_kind_is_known(spec.kind().as_str()) {
                FilterCompileFailure::InvalidSettings
            } else {
                FilterCompileFailure::UnsupportedKind
            },
        )),
    }
}

/// Compiles a supported video effect, discarding an unavailable-filter reason.
#[must_use]
pub fn compile_filter(spec: &SourceFilterSpec) -> Option<FrameFilter> {
    match compile_filter_report(spec) {
        FilterCompilation::Applied(filter) => Some(filter),
        FilterCompilation::Ignored | FilterCompilation::Unavailable(_) => None,
    }
}

fn frame_filter_kind_is_known(kind: &str) -> bool {
    matches!(
        kind,
        "grayscale"
            | "brightness"
            | "opacity"
            | "crop_pad"
            | "color_correction"
            | "color_multiply_add"
            | "luma_key"
            | "color_key"
            | "chroma_key"
            | "sharpen"
            | "scroll"
            | "render_delay"
    )
}

/// Compiles a persistent audio/video filter into an ordered audio operation.
///
/// Audio filters are kept separate from [`compile_filter`] because they run on
/// captured audio blocks rather than rendered video frames. The project-facing
/// settings use fixed-point `db_milli` plus integer milliseconds, which avoids
/// locale-dependent decimal parsing on the real-time boundary.
#[must_use]
fn compile_audio_filter_operation(spec: &SourceFilterSpec) -> Option<AudioFilter> {
    if !spec.enabled() || spec.category() != SourceFilterCategory::AudioVideo {
        return None;
    }
    match spec.kind().as_str() {
        "gain" => spec
            .settings()
            .get("db_milli")
            .and_then(|value| value.parse::<i32>().ok())
            .and_then(|milli_db| AudioFilter::gain_db_milli(milli_db).ok()),
        "invert_polarity" => Some(AudioFilter::InvertPolarity),
        "limiter" => {
            let threshold = spec
                .settings()
                .get("threshold_db_milli")
                .and_then(|value| value.parse::<i32>().ok())?;
            let release_ms = spec
                .settings()
                .get("release_ms")
                .and_then(|value| value.parse::<u16>().ok())?;
            AudioFilter::limiter_db_milli(threshold, release_ms).ok()
        }
        "compressor" => {
            let read_signed = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
            };
            let read_unsigned = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<u16>().ok())
            };
            AudioFilter::compressor(
                read_unsigned("ratio_milli")?,
                read_signed("threshold_db_milli")?,
                read_unsigned("attack_ms")?,
                read_unsigned("release_ms")?,
                read_signed("output_gain_db_milli")?,
            )
            .ok()
        }
        "expander" => {
            let read_signed = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
            };
            let read_unsigned = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<u16>().ok())
            };
            AudioFilter::expander(
                read_unsigned("ratio_milli")?,
                read_signed("threshold_db_milli")?,
                read_unsigned("attack_ms")?,
                read_unsigned("release_ms")?,
                read_signed("output_gain_db_milli")?,
            )
            .ok()
        }
        "gate" | "noise_gate" => {
            let read_signed = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<i32>().ok())
            };
            let read_unsigned = |key| {
                spec.settings()
                    .get(key)
                    .and_then(|value| value.parse::<u16>().ok())
            };
            AudioFilter::noise_gate(
                read_signed("open_threshold_db_milli")?,
                read_signed("close_threshold_db_milli")?,
                read_unsigned("attack_ms")?,
                read_unsigned("hold_ms")?,
                read_unsigned("release_ms")?,
            )
            .ok()
        }
        _ => None,
    }
}

/// Translates a persisted audio filter and preserves the reason when it is
/// unavailable in the audio runtime.
#[must_use]
pub fn compile_audio_filter_report(spec: &SourceFilterSpec) -> FilterCompilation<AudioFilter> {
    if !spec.enabled() {
        return FilterCompilation::Ignored;
    }
    if spec.category() != SourceFilterCategory::AudioVideo {
        return FilterCompilation::Unavailable(FilterDiagnostic::new(
            spec,
            FilterCompileFailure::UnsupportedCategory,
        ));
    }
    match compile_audio_filter_operation(spec) {
        Some(filter) => FilterCompilation::Applied(filter),
        None => FilterCompilation::Unavailable(FilterDiagnostic::new(
            spec,
            if audio_filter_kind_is_known(spec.kind().as_str()) {
                FilterCompileFailure::InvalidSettings
            } else {
                FilterCompileFailure::UnsupportedKind
            },
        )),
    }
}

/// Compiles a supported audio filter, discarding an unavailable-filter reason.
#[must_use]
pub fn compile_audio_filter(spec: &SourceFilterSpec) -> Option<AudioFilter> {
    match compile_audio_filter_report(spec) {
        FilterCompilation::Applied(filter) => Some(filter),
        FilterCompilation::Ignored | FilterCompilation::Unavailable(_) => None,
    }
}

fn audio_filter_kind_is_known(kind: &str) -> bool {
    matches!(
        kind,
        "gain" | "invert_polarity" | "limiter" | "compressor" | "expander" | "gate" | "noise_gate"
    )
}
