//! Local accessible browser frontend for the Rust-owned OBS-RS desktop state.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    error::Error,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    time::Duration,
};

use obs_rs_config::Config;
use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_project::{Profile, Project, SceneSpec, SourceSpec};
use obs_rs_ui::{
    parse_console_command, parse_web_request, ConsoleCommand, DesktopState, WebRoute,
    MAX_WEB_REQUEST_BYTES,
};

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8765";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

fn main() -> Result<(), Box<dyn Error>> {
    let (bind_address, once) = parse_arguments()?;
    let listener = TcpListener::bind(&bind_address)?;
    println!(
        "OBS-RS browser frontend listening on http://{bind_address}{}",
        if once { " (one request)" } else { "" }
    );
    let mut state = DesktopState::new(initial_project()?);

    for incoming in listener.incoming() {
        let mut stream = incoming?;
        if let Err(error) = serve_request(&mut stream, &mut state) {
            eprintln!("request error: {error}");
        }
        if once {
            break;
        }
    }
    Ok(())
}

fn parse_arguments() -> Result<(String, bool), Box<dyn Error>> {
    let mut bind_address = DEFAULT_BIND_ADDRESS.to_owned();
    let mut once = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--once" => once = true,
            "--bind" => {
                bind_address = arguments.next().ok_or("--bind requires an address")?;
            }
            "--help" | "-h" => {
                println!(
                    "Usage: obs-rs-web [--bind ADDRESS] [--once]\n\nDefault address: {DEFAULT_BIND_ADDRESS}"
                );
                std::process::exit(0);
            }
            value => return Err(format!("unknown argument {value}").into()),
        }
    }
    Ok((bind_address, once))
}

fn serve_request(stream: &mut TcpStream, state: &mut DesktopState) -> io::Result<()> {
    let request = read_request(stream)?;
    let response = match parse_web_request(&request) {
        Ok(WebRoute::Home) => response("200 OK", "text/html; charset=utf-8", state.web_page()),
        Ok(WebRoute::Snapshot) => response(
            "200 OK",
            "text/plain; charset=utf-8",
            state.accessible_snapshot(),
        ),
        Ok(WebRoute::Command(line)) => command_response(state, &line),
        Err(error) => response(
            "400 Bad Request",
            "text/plain; charset=utf-8",
            error.to_string(),
        ),
    };
    stream.write_all(response.as_bytes())
}

fn command_response(state: &mut DesktopState, line: &str) -> String {
    match parse_console_command(line) {
        Ok(ConsoleCommand::Apply(command)) => match state.dispatch(command) {
            Ok(()) => response(
                "200 OK",
                "text/plain; charset=utf-8",
                state.accessible_snapshot(),
            ),
            Err(error) => response(
                "409 Conflict",
                "text/plain; charset=utf-8",
                error.to_string(),
            ),
        },
        Ok(ConsoleCommand::Show) => response(
            "200 OK",
            "text/plain; charset=utf-8",
            state.accessible_snapshot(),
        ),
        Ok(ConsoleCommand::Help) => response("200 OK", "text/plain; charset=utf-8", help_text()),
        Ok(ConsoleCommand::Quit) => response(
            "400 Bad Request",
            "text/plain; charset=utf-8",
            "quit is only available to the terminal frontend".to_owned(),
        ),
        Err(error) => response(
            "400 Bad Request",
            "text/plain; charset=utf-8",
            error.to_string(),
        ),
    }
}

fn response(status: &str, content_type: &str, body: impl Into<String>) -> String {
    let body = body.into();
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn read_request(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.len() > MAX_WEB_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP request exceeds the configured limit",
            ));
        }
        let Some(header_end) = header_end(&request) else {
            continue;
        };
        let content_length = declared_content_length(&request[..header_end])?;
        if request.len() >= header_end.saturating_add(content_length) {
            break;
        }
    }
    Ok(request)
}

fn header_end(request: &[u8]) -> Option<usize> {
    request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn declared_content_length(header: &[u8]) -> io::Result<usize> {
    let header = std::str::from_utf8(header)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "HTTP header is not UTF-8"))?;
    for line in header.split("\r\n").skip(1) {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP header is malformed",
            ));
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "HTTP content length is invalid")
            });
        }
    }
    Ok(0)
}

fn help_text() -> String {
    "Commands:\n  show | snapshot\n  preview <scene-id>\n  program <scene-id>\n  profile <profile-id>\n  swap\n  transition cut | transition fade <0..1000>\n  record start|stop\n  stream start|stop\n  help".to_owned()
}

fn initial_project() -> Result<Project, Box<dyn Error>> {
    let format = VideoFormat::new(640, 360, FrameRate::new(30, 1)?)?;
    let mut project = Project::new("OBS-RS browser")?;
    let mut profile = Profile::new("live", "Live profile", format)?;
    profile.add_scene(scene("preview", "Preview", "#102030FF")?)?;
    profile.add_scene(scene("program", "Program", "#203040FF")?)?;
    project.add_profile(profile)?;
    Ok(project)
}

fn scene(id: &str, name: &str, color: &str) -> Result<SceneSpec, Box<dyn Error>> {
    let mut settings = Config::new();
    settings.set("width", "640")?;
    settings.set("height", "360")?;
    settings.set("color", color)?;
    let mut scene = SceneSpec::new(id, name)?;
    scene.add_source(SourceSpec::new(
        "background",
        "color_source",
        "Background",
        settings,
    )?)?;
    Ok(scene)
}
