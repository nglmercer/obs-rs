use std::fmt;

use super::{MAX_CONSOLE_COMMAND_BYTES, MAX_WEB_REQUEST_BYTES};

/// A route understood by the local browser presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebRoute {
    /// Serve the accessible control page.
    Home,
    /// Return the current labeled state snapshot as plain text.
    Snapshot,
    /// Parse and dispatch one line-oriented desktop command.
    Command(String),
}

/// Errors raised while parsing a bounded local HTTP request.
#[derive(Debug, Eq, PartialEq)]
pub enum WebRequestError {
    /// The request exceeded [`MAX_WEB_REQUEST_BYTES`].
    TooLarge,
    /// The request was not valid UTF-8.
    InvalidUtf8,
    /// The request line or required headers were malformed.
    Malformed,
    /// The HTTP method is not supported by the local frontend.
    UnsupportedMethod(String),
    /// The path is not a supported local frontend route.
    InvalidPath(String),
    /// The request body exceeded [`MAX_CONSOLE_COMMAND_BYTES`].
    BodyTooLong,
    /// The declared body size did not match the received bytes.
    ContentLengthMismatch { expected: usize, actual: usize },
}

impl fmt::Display for WebRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("web request is too large"),
            Self::InvalidUtf8 => formatter.write_str("web request is not valid UTF-8"),
            Self::Malformed => formatter.write_str("web request is malformed"),
            Self::UnsupportedMethod(method) => {
                write!(formatter, "web method {method} is not supported")
            }
            Self::InvalidPath(path) => write!(formatter, "web path {path} is not supported"),
            Self::BodyTooLong => formatter.write_str("web command body is too long"),
            Self::ContentLengthMismatch { expected, actual } => write!(
                formatter,
                "web content length declares {expected} bytes but received {actual}"
            ),
        }
    }
}

impl std::error::Error for WebRequestError {}
/// Parses one bounded HTTP/1.x request for the local browser frontend.
///
/// Only `GET /`, `GET /snapshot`, and `POST /command` are accepted. Chunked
/// requests, arbitrary paths, and bodies larger than the terminal command limit are
/// rejected so the browser surface shares the same validated command model.
///
/// # Errors
///
/// Returns [`WebRequestError`] when the request is malformed, oversized, or uses an
/// unsupported route or method.
pub fn parse_web_request(request: &[u8]) -> Result<WebRoute, WebRequestError> {
    if request.len() > MAX_WEB_REQUEST_BYTES {
        return Err(WebRequestError::TooLarge);
    }
    let request = std::str::from_utf8(request).map_err(|_| WebRequestError::InvalidUtf8)?;
    let (head, body) = request
        .split_once("\r\n\r\n")
        .ok_or(WebRequestError::Malformed)?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(WebRequestError::Malformed)?;
    let mut request_words = request_line.split_whitespace();
    let method = request_words.next().ok_or(WebRequestError::Malformed)?;
    let path = request_words.next().ok_or(WebRequestError::Malformed)?;
    let version = request_words.next().ok_or(WebRequestError::Malformed)?;
    if request_words.next().is_some() || (version != "HTTP/1.0" && version != "HTTP/1.1") {
        return Err(WebRequestError::Malformed);
    }

    let mut content_length = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(WebRequestError::Malformed)?;
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(WebRequestError::Malformed);
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| WebRequestError::Malformed)?,
            );
        }
    }
    let actual_length = body.len();
    let expected_length = content_length.unwrap_or(0);
    if expected_length != actual_length {
        return Err(WebRequestError::ContentLengthMismatch {
            expected: expected_length,
            actual: actual_length,
        });
    }

    match method {
        "GET" => {
            if !body.is_empty() {
                return Err(WebRequestError::Malformed);
            }
            match path {
                "/" => Ok(WebRoute::Home),
                "/snapshot" => Ok(WebRoute::Snapshot),
                _ => Err(WebRequestError::InvalidPath(path.to_owned())),
            }
        }
        "POST" => {
            if path != "/command" {
                return Err(WebRequestError::InvalidPath(path.to_owned()));
            }
            if body.len() > MAX_CONSOLE_COMMAND_BYTES {
                return Err(WebRequestError::BodyTooLong);
            }
            if body.trim().is_empty() {
                return Err(WebRequestError::Malformed);
            }
            Ok(WebRoute::Command(body.to_owned()))
        }
        _ => Err(WebRequestError::UnsupportedMethod(method.to_owned())),
    }
}
