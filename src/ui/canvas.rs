use iced::widget::canvas::{self, Cache, Frame, Geometry, Program};
use iced::mouse::Cursor;
use iced::{Rectangle, Renderer, Theme};
use std::sync::Arc;

use crate::gpu;
use crate::Message;

/// GPU-accelerated canvas renderer for RAW images
pub struct GpuRenderer {
    /// The GPU rendering pipeline
    pub pipeline: Arc<gpu::RenderPipeline>,
    /// Cache to avoid re-rendering every frame
    cache: Cache,
}

impl GpuRenderer {
    /// Create a new GPU renderer
    pub fn new(pipeline: Arc<gpu::RenderPipeline>) -> Self {
        Self {
            pipeline,
            cache: Cache::new(),
        }
    }
    
    /// Request a redraw (invalidate cache)
    pub fn request_redraw(&mut self) {
        self.cache.clear();
    }
}

impl Program<Message> for GpuRenderer {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        // Use cache to avoid redrawing every frame
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            // Render the GPU pipeline output to the canvas
            self.pipeline.render(frame, bounds);
        });

        vec![geometry]
    }

    fn update(
        &self,
        _state: &mut Self::State,
        _event: canvas::Event,
        _bounds: Rectangle,
        _cursor: Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        // No user interaction needed for now
        (canvas::event::Status::Ignored, None)
    }
}
