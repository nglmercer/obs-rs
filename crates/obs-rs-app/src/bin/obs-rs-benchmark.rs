//! Repeatable headless sustained-render fixture for OBS-RS.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = obs_rs_benchmark::run_legacy_benchmark().map_err(std::io::Error::other)?;
    if std::env::args()
        .skip(1)
        .any(|argument| argument == "--json")
    {
        println!("{}", obs_rs_benchmark::legacy_json(&output));
    } else {
        println!("{}", obs_rs_benchmark::legacy_text(&output));
    }
    Ok(())
}
