//! Slint presentation boundary for rendered video frames.

use obs_rs_media::VideoFrame;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub(crate) trait PreviewPresenter {
    fn present(&mut self, frame: &VideoFrame) -> Image;
}

struct SlintPreviewPresenter;

impl PreviewPresenter for SlintPreviewPresenter {
    fn present(&mut self, frame: &VideoFrame) -> Image {
        let format = frame.format();
        // Slint owns its pixel storage, so one copy out of the engine frame is
        // unavoidable here; `clone_from_slice` performs it as a single block
        // copy. The worker supplies a viewport-sized frame, not the full
        // program canvas.
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            frame.pixels(),
            format.width(),
            format.height(),
        );
        Image::from_rgba8(buffer)
    }
}

pub(crate) fn frame_to_image(frame: &VideoFrame) -> Image {
    SlintPreviewPresenter.present(frame)
}
