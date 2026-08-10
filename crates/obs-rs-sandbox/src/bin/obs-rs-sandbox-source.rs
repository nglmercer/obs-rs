//! Small reference subprocess extension used by the sandbox contract.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    error::Error,
    io::{self, BufWriter, Write},
};

use obs_rs_capture::encode_frame_packet;
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::PluginManifest;
use obs_rs_sandbox::SandboxedPluginManifest;
use obs_rs_util::Identifier;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|argument| argument == "--obs-rs-manifest")
    {
        let manifest = SandboxedPluginManifest::new(
            PluginManifest::new("obs_rs_sandbox_demo", "OBS-RS sandbox demo source", "0.1.0")?,
            [Identifier::new("sandbox_pattern")?],
        )?;
        print!("{}", manifest.serialize());
        return Ok(());
    }
    if !arguments.iter().any(|argument| argument == "--frames") {
        return Err("expected --obs-rs-manifest or --frames".into());
    }

    let width = environment_u32("OBS_RS_WIDTH", 640)?;
    let height = environment_u32("OBS_RS_HEIGHT", 360)?;
    let numerator = environment_u32("OBS_RS_FPS_NUMERATOR", 30)?;
    let denominator = environment_u32("OBS_RS_FPS_DENOMINATOR", 1)?;
    let rate = FrameRate::new(numerator, denominator)?;
    let format = VideoFormat::new(width, height, rate)?;
    let period = rate.period_nanos().ok_or("frame period overflowed")?;
    let stdout = io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    for index in 0_u64.. {
        let timestamp = Timestamp::from_nanos(
            period
                .checked_mul(index)
                .ok_or("sandbox frame timestamp overflowed")?,
        );
        let phase = u8::try_from(index % 240).unwrap_or_default();
        let frame = VideoFrame::solid(
            format,
            timestamp,
            [phase, 255_u8.saturating_sub(phase), 96, 255],
        );
        let packet = encode_frame_packet(&frame)?;
        if output.write_all(&packet).is_err() {
            break;
        }
        output.flush()?;
    }
    Ok(())
}

fn environment_u32(name: &str, default: u32) -> Result<u32, Box<dyn Error>> {
    let value = std::env::var(name).unwrap_or_else(|_| default.to_string());
    Ok(value.parse::<u32>()?)
}
