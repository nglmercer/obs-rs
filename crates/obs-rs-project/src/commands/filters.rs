//! Compatibility conversion from legacy renderer filters to project filter instances.

use obs_rs_config::Config;
use obs_rs_media::FrameFilter;

use super::super::{
    error::ProjectError,
    model::{SourceFilterCategory, SourceFilterSpec},
};

#[allow(
    clippy::too_many_lines,
    reason = "the legacy conversion keeps every portable filter mapping in one exhaustive boundary"
)]
pub(super) fn legacy_filter_spec(
    index: usize,
    filter: FrameFilter,
) -> Result<SourceFilterSpec, ProjectError> {
    let (kind, name, settings) = match filter {
        FrameFilter::Grayscale => ("grayscale", "Grayscale", Config::new()),
        FrameFilter::Brightness { milli } => {
            let mut settings = Config::new();
            settings
                .set("milli", &milli.to_string())
                .map_err(ProjectError::Config)?;
            ("brightness", "Brightness", settings)
        }
        FrameFilter::Opacity(value) => {
            let mut settings = Config::new();
            settings
                .set("value", &value.to_string())
                .map_err(ProjectError::Config)?;
            ("opacity", "Opacity", settings)
        }
        FrameFilter::CropPad {
            left,
            top,
            right,
            bottom,
        } => {
            let mut settings = Config::new();
            for (key, value) in [
                ("left", left),
                ("top", top),
                ("right", right),
                ("bottom", bottom),
            ] {
                settings
                    .set(key, &value.to_string())
                    .map_err(ProjectError::Config)?;
            }
            ("crop_pad", "Crop/Pad", settings)
        }
        FrameFilter::ColorCorrection(correction) => {
            let mut settings = Config::new();
            for (key, value) in [
                ("gamma", correction.gamma_milli()),
                ("contrast", correction.contrast_milli()),
                ("brightness", correction.brightness_milli()),
                ("saturation", correction.saturation_milli()),
                ("hue_shift", correction.hue_shift_degrees()),
                ("opacity", correction.opacity_milli()),
            ] {
                settings
                    .set(key, &value.to_string())
                    .map_err(ProjectError::Config)?;
            }
            ("color_correction", "Color Correction", settings)
        }
        FrameFilter::ColorMultiplyAdd(color_wash) => {
            let mut settings = Config::new();
            let multiply = color_wash.multiply();
            let add = color_wash.add();
            for (key, value) in [
                ("multiply_red", multiply[0]),
                ("multiply_green", multiply[1]),
                ("multiply_blue", multiply[2]),
                ("add_red", add[0]),
                ("add_green", add[1]),
                ("add_blue", add[2]),
            ] {
                settings
                    .set(key, &value.to_string())
                    .map_err(ProjectError::Config)?;
            }
            ("color_multiply_add", "Color Multiply/Add", settings)
        }
        FrameFilter::LumaKey(luma_key) => {
            let mut settings = Config::new();
            for (key, value) in [
                ("luma_max", luma_key.luma_max_milli()),
                ("luma_min", luma_key.luma_min_milli()),
                ("luma_max_smooth", luma_key.luma_max_smooth_milli()),
                ("luma_min_smooth", luma_key.luma_min_smooth_milli()),
            ] {
                settings
                    .set(key, &value.to_string())
                    .map_err(ProjectError::Config)?;
            }
            ("luma_key", "Luma Key", settings)
        }
        FrameFilter::ColorKey(color_key) => {
            let mut settings = Config::new();
            for (key, value) in [
                ("key_red", i32::from(color_key.key_red())),
                ("key_green", i32::from(color_key.key_green())),
                ("key_blue", i32::from(color_key.key_blue())),
                ("similarity", color_key.similarity_milli()),
                ("smoothness", color_key.smoothness_milli()),
            ] {
                settings
                    .set(key, &value.to_string())
                    .map_err(ProjectError::Config)?;
            }
            ("color_key", "Color Key", settings)
        }
        FrameFilter::ChromaKey(chroma_key) => {
            let mut settings = Config::new();
            for (key, value) in [
                ("key_red", i32::from(chroma_key.key_red())),
                ("key_green", i32::from(chroma_key.key_green())),
                ("key_blue", i32::from(chroma_key.key_blue())),
                ("similarity", chroma_key.similarity_milli()),
                ("smoothness", chroma_key.smoothness_milli()),
                ("spill", chroma_key.spill_milli()),
            ] {
                settings
                    .set(key, &value.to_string())
                    .map_err(ProjectError::Config)?;
            }
            ("chroma_key", "Chroma Key", settings)
        }
        FrameFilter::Sharpen { milli } => {
            let mut settings = Config::new();
            settings
                .set("sharpness", &milli.to_string())
                .map_err(ProjectError::Config)?;
            ("sharpen", "Sharpen", settings)
        }
        FrameFilter::Scroll {
            speed_x,
            speed_y,
            looped,
        } => {
            let mut settings = Config::new();
            for (key, value) in [
                ("speed_x", speed_x.to_string()),
                ("speed_y", speed_y.to_string()),
                ("loop", looped.to_string()),
            ] {
                settings.set(key, &value).map_err(ProjectError::Config)?;
            }
            ("scroll", "Scroll", settings)
        }
        FrameFilter::RenderDelay(delay) => {
            let mut settings = Config::new();
            settings
                .set("milliseconds", &delay.milliseconds.to_string())
                .map_err(ProjectError::Config)?;
            ("render_delay", "Render Delay", settings)
        }
    };
    SourceFilterSpec::with_category(
        &format!("legacy_filter_{}", index.saturating_add(1)),
        name,
        kind,
        SourceFilterCategory::Effect,
        settings,
    )
}
