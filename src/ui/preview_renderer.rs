use iced::widget::canvas::{self, Program};
use iced::mouse::Cursor;
use iced::{Rectangle, Renderer, Theme};

use crate::Message;

/// Canvas-based JPEG preview renderer (Phase 32)
/// 
/// This renderer displays JPEG previews using Canvas instead of Image widget.
/// It uses the SAME coordinate system as GpuRenderer, ensuring zoom/pan state
/// transfers seamlessly when transitioning from preview to RAW.
pub struct PreviewRenderer {
    /// The JPEG image handle to display
    pub handle: iced::widget::image::Handle,
    /// Current zoom level (1.0 = 100%, 2.0 = 200%, etc.)
    pub zoom: f32,
    /// Pan offset in normalized coordinates
    pub offset: cgmath::Vector2<f32>,
}

impl Program<Message> for PreviewRenderer {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // The bounds give us the viewport size (canvas container size)
        let viewport_width = bounds.width;
        let viewport_height = bounds.height;

        // For JPEG previews, we assume a standard aspect ratio
        // Since we're loading from cache (1280px width typically), we'll assume 3:2 ratio
        // This is a reasonable assumption for most RAW files
        // TODO: If we need exact dimensions, we could store them when loading the handle
        let image_aspect = 3.0 / 2.0; // Standard 3:2 camera aspect ratio
        let viewport_aspect = viewport_width / viewport_height;

        // Calculate fitted size (contain mode - image fits entirely within viewport)
        let (fitted_width, fitted_height) = if image_aspect > viewport_aspect {
            // Image is wider - fit to width
            let w = viewport_width;
            let h = w / image_aspect;
            (w, h)
        } else {
            // Image is taller - fit to height
            let h = viewport_height;
            let w = h * image_aspect;
            (w, h)
        };

        // Start with the image centered in the viewport
        let center_x = viewport_width / 2.0;
        let center_y = viewport_height / 2.0;

        // Apply zoom to the fitted size
        let zoomed_width = fitted_width * self.zoom;
        let zoomed_height = fitted_height * self.zoom;

        // Apply pan offset
        // CRITICAL: Pan offset in GPU shader is in texture coordinate space (relative to image)
        // So we scale by the FITTED image dimensions, not viewport dimensions
        // This matches how the GPU shader interprets pan_x/pan_y relative to the image
        let pan_x = self.offset.x * fitted_width;
        let pan_y = self.offset.y * fitted_height;

        // Calculate final image bounds
        let image_x = center_x - (zoomed_width / 2.0) + pan_x;
        let image_y = center_y - (zoomed_height / 2.0) + pan_y;

        let image_bounds = Rectangle {
            x: image_x,
            y: image_y,
            width: zoomed_width,
            height: zoomed_height,
        };

        // Draw the image
        // Note: frame.draw_image signature is draw_image(bounds: Rectangle, image: impl Into<Image>)
        let canvas_image = iced::widget::canvas::Image::new(self.handle.clone());
        frame.draw_image(image_bounds, canvas_image);

        vec![frame.into_geometry()]
    }

    fn update(
        &self,
        _state: &mut Self::State,
        _event: canvas::Event,
        _bounds: Rectangle,
        _cursor: Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        // No event handling here - mouse interactions are handled by the parent container
        // in main.rs (zoom/pan messages are sent from mouse_area wrapper)
        (canvas::event::Status::Ignored, None)
    }
}
