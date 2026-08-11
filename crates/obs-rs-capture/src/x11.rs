//! Direct Linux X11 screen capture split into connection, protocol, image, and
//! lifecycle modules.

mod connection;
mod error;
mod image;
mod protocol;
mod randr;
mod screen;
mod window;

#[cfg(test)]
mod tests;

pub use randr::X11Monitor;
pub use screen::{x11_monitors, X11CaptureDevice};
pub use window::{parse_window_id, x11_windows, X11Window};
