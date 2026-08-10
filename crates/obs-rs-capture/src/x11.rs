//! Direct Linux X11 screen capture split into connection, protocol, image, and
//! lifecycle modules.

mod connection;
mod error;
mod image;
mod protocol;
mod screen;

#[cfg(test)]
mod tests;

pub use screen::X11CaptureDevice;
