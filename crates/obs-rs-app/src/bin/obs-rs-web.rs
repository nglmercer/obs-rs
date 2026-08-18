//! Local accessible browser frontend for the Rust-owned OBS-RS desktop state.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    error::Error,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

use obs_rs_config::Config;
use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_project::{Profile, Project, SceneItemSpec, SceneSpec, SourceSpec};
use obs_rs_ui::{
    parse_console_command, parse_web_request, ConsoleCommand, DesktopState, WebRoute,
    MAX_WEB_REQUEST_BYTES,
};

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8765";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Threads that serve requests concurrently.
///
/// Requests are short and mostly serialize on the state lock, so a small fixed
/// pool is enough. What it buys is isolation: one client that opens a socket and
/// stalls occupies a single worker for the read timeout instead of blocking
/// every other request, which is what a single-threaded accept loop did.
const WORKER_THREADS: usize = 4;

/// Accepted connections allowed to wait for a worker.
///
/// Bounded so a flood of connections cannot grow the backlog without limit;
/// beyond this the listener simply stops accepting, and the OS queue applies
/// backpressure.
const CONNECTION_BACKLOG: usize = 64;

/// Bytes of entropy in the session token, hex-encoded to twice this length.
const SESSION_TOKEN_BYTES: usize = 16;

fn main() -> Result<(), Box<dyn Error>> {
    let (bind_address, once, authenticate) = parse_arguments()?;
    let listener = TcpListener::bind(&bind_address)?;

    // Binding to loopback keeps other hosts out, but every local process — every
    // browser tab, every script — can still reach this port, and `/command`
    // drives the engine. A per-session token is what separates the operator who
    // launched this process from everything else running as the same user.
    let token = if authenticate {
        Some(session_token()?)
    } else {
        None
    };
    match &token {
        Some(token) => println!(
            "OBS-RS browser frontend listening on http://{bind_address}/?token={token}{}\n\
             Open the URL above; requests without the token are refused.",
            if once { " (one request)" } else { "" }
        ),
        None => println!(
            "OBS-RS browser frontend listening on http://{bind_address}{} \
             -- WARNING: authentication disabled, any local process can drive the engine",
            if once { " (one request)" } else { "" }
        ),
    }

    let state = Arc::new(Mutex::new(DesktopState::new(initial_project()?)));
    let token = Arc::new(token);
    let (sender, receiver) = mpsc::sync_channel::<TcpStream>(CONNECTION_BACKLOG);
    let receiver = Arc::new(Mutex::new(receiver));

    let workers = (0..WORKER_THREADS)
        .map(|_| {
            let receiver = Arc::clone(&receiver);
            let state = Arc::clone(&state);
            let token = Arc::clone(&token);
            thread::spawn(move || {
                loop {
                    // The lock is held only to take the next connection, never
                    // while serving one, so workers do not serialize on it.
                    let next = receiver
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .recv();
                    let Ok(mut stream) = next else {
                        return;
                    };
                    if let Err(error) = serve_request(&mut stream, &state, token.as_ref().as_ref())
                    {
                        eprintln!("request error: {error}");
                    }
                }
            })
        })
        .collect::<Vec<_>>();

    for incoming in listener.incoming() {
        let stream = incoming?;
        if sender.send(stream).is_err() {
            break;
        }
        if once {
            break;
        }
    }

    drop(sender);
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

/// Generates the per-session bearer token as lowercase hex.
fn session_token() -> Result<String, Box<dyn Error>> {
    let mut bytes = [0_u8; SESSION_TOKEN_BYTES];
    obs_rs_util::fill_random(&mut bytes)?;
    Ok(bytes.iter().fold(String::new(), |mut token, byte| {
        use std::fmt::Write as _;
        let _ = write!(token, "{byte:02x}");
        token
    }))
}

/// Compares two tokens without leaking their common prefix through timing.
fn tokens_match(expected: &str, presented: &str) -> bool {
    if expected.len() != presented.len() {
        return false;
    }
    expected
        .bytes()
        .zip(presented.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

/// Extracts the token a request presents, from the header or the query string.
///
/// The header is what a scripted client should use. The query parameter exists
/// because the operator's first contact is a URL pasted into a browser, which
/// cannot set a header.
fn presented_token(request: &str) -> Option<&str> {
    let mut lines = request.split("\r\n");
    let request_line = lines.next()?;

    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("authorization") {
                if let Some(token) = value.trim().strip_prefix("Bearer ") {
                    return Some(token.trim());
                }
            }
        }
    }

    let target = request_line.split(' ').nth(1)?;
    let query = target.split_once('?').map(|(_, query)| query)?;
    query.split('&').find_map(|pair| {
        pair.split_once('=')
            .filter(|(name, _)| *name == "token")
            .map(|(_, value)| value)
    })
}

fn parse_arguments() -> Result<(String, bool, bool), Box<dyn Error>> {
    let mut bind_address = DEFAULT_BIND_ADDRESS.to_owned();
    let mut once = false;
    let mut authenticate = true;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--once" => once = true,
            "--no-auth" => authenticate = false,
            "--bind" => {
                bind_address = arguments.next().ok_or("--bind requires an address")?;
            }
            "--help" | "-h" => {
                println!(
                    "Usage: obs-rs-web [--bind ADDRESS] [--once] [--no-auth]\n\n\
                     Default address: {DEFAULT_BIND_ADDRESS}\n\
                     A session token is generated at startup and printed with the URL.\n\
                     Pass it as ?token=... or an `Authorization: Bearer` header.\n\
                     --no-auth disables the check; only use it on a trusted host."
                );
                std::process::exit(0);
            }
            value => return Err(format!("unknown argument {value}").into()),
        }
    }
    Ok((bind_address, once, authenticate))
}

fn serve_request(
    stream: &mut TcpStream,
    state: &Mutex<DesktopState>,
    token: Option<&String>,
) -> io::Result<()> {
    let request = read_request(stream)?;

    if let Some(expected) = token {
        let authorized = std::str::from_utf8(&request)
            .ok()
            .and_then(presented_token)
            .is_some_and(|presented| tokens_match(expected, presented));
        if !authorized {
            let refusal = response(
                "401 Unauthorized",
                "text/plain; charset=utf-8",
                "a valid session token is required; use the URL printed at startup\n",
            );
            return stream.write_all(refusal.as_bytes());
        }
    }

    let state = &mut *state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    "Commands:\n  show | snapshot\n  preview <scene-id>\n  program <scene-id>\n  profile <profile-id>\n  swap\n  transition cut | transition fade <0..1000>\n  take cut | take fade <0..1000>\n  language en|es\n  record start|stop\n  stream start|stop\n  help".to_owned()
}

fn initial_project() -> Result<Project, Box<dyn Error>> {
    let format = VideoFormat::new(640, 360, FrameRate::new(30, 1)?)?;
    let mut project = Project::new("OBS-RS browser")?;
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
