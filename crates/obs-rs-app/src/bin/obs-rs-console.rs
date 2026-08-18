//! Accessible terminal frontend for the Rust-owned OBS-RS desktop state.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    error::Error,
    io::{self, BufRead, Write},
};

use obs_rs_config::Config;
use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_project::{Profile, Project, SceneItemSpec, SceneSpec, SourceSpec};
use obs_rs_ui::{parse_console_command, ConsoleCommand, DesktopState};

fn main() -> Result<(), Box<dyn Error>> {
    let mut state = DesktopState::new(initial_project()?);
    let stdin = io::stdin();
    let mut output = io::BufWriter::new(io::stdout());

    writeln!(output, "OBS-RS terminal frontend")?;
    writeln!(
        output,
        "Type `help` for commands; input is line-oriented and scriptable."
    )?;
    print_help(&mut output)?;
    print_snapshot(&mut output, &state)?;

    for line in stdin.lock().lines() {
        let line = line?;
        match parse_console_command(&line) {
            Ok(ConsoleCommand::Help) => print_help(&mut output)?,
            Ok(ConsoleCommand::Show) => print_snapshot(&mut output, &state)?,
            Ok(ConsoleCommand::Quit) => {
                writeln!(output, "bye")?;
                break;
            }
            Ok(ConsoleCommand::Apply(command)) => match state.dispatch(command) {
                Ok(()) => print_snapshot(&mut output, &state)?,
                Err(error) => writeln!(output, "error: {error}")?,
            },
            Err(error) => writeln!(output, "error: {error}")?,
        }
        output.flush()?;
    }

    Ok(())
}

fn print_help(output: &mut impl Write) -> io::Result<()> {
    writeln!(output, "Commands:")?;
    writeln!(output, "  show | snapshot")?;
    writeln!(output, "  preview <scene-id>")?;
    writeln!(output, "  program <scene-id>")?;
    writeln!(output, "  profile <profile-id>")?;
    writeln!(output, "  swap")?;
    writeln!(output, "  transition cut | transition fade <0..1000>")?;
    writeln!(output, "  take cut | take fade <0..1000>")?;
    writeln!(output, "  language en|es")?;
    writeln!(output, "  record start|stop")?;
    writeln!(output, "  stream start|stop")?;
    writeln!(output, "  help | quit")
}

fn print_snapshot(output: &mut impl Write, state: &DesktopState) -> io::Result<()> {
    writeln!(output, "{}", state.accessible_snapshot())
}

fn initial_project() -> Result<Project, Box<dyn Error>> {
    let format = VideoFormat::new(640, 360, FrameRate::new(30, 1)?)?;
    let mut project = Project::new("OBS-RS console")?;
    let mut profile = Profile::new("live", "Live profile", format)?;
    let (preview, preview_source) = scene("preview", "Preview", "background_preview", "#102030FF")?;
    let (program, program_source) = scene("program", "Program", "background_program", "#203040FF")?;
    profile.add_source(preview_source)?;
    profile.add_source(program_source)?;
    profile.add_scene(preview)?;
    profile.add_scene(program)?;
    project.add_profile(profile)?;
    Ok(project)
}

fn scene(
    id: &str,
    name: &str,
    source_id: &str,
    color: &str,
) -> Result<(SceneSpec, SourceSpec), Box<dyn Error>> {
    let mut settings = Config::new();
    settings.set("width", "640")?;
    settings.set("height", "360")?;
    settings.set("color", color)?;
    let mut scene = SceneSpec::new(id, name)?;
    scene.add_item(SceneItemSpec::for_source(source_id)?)?;
    let source = SourceSpec::new(source_id, "color_source", "Background", settings)?;
    Ok((scene, source))
}
