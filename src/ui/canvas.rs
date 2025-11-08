use iced::widget::canvas::{self, Frame, Geometry, Program};
use iced::mouse::Cursor;
use iced::{Rectangle, Renderer, Theme};
use std::sync::Arc;

use crate::gpu;
use crate::state::edit::EditParams;
use crate::Message;

/// GPU-accelerated canvas renderer for RAW images
pub struct GpuRenderer {
    /// The GPU rendering pipeline
    pub pipeline: Arc<gpu::RenderPipeline>,
    /// Current edit parameters (for visual feedback)
    pub params: EditParams,
}

impl GpuRenderer {
    /// Create a new GPU renderer
    pub fn new(pipeline: Arc<gpu::RenderPipeline>, params: EditParams) -> Self {
        Self {
            pipeline,
            params,
        }
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
        // Render every frame to show real-time updates
        let mut frame = Frame::new(renderer, bounds.size());
        
        // Render the GPU pipeline output with current parameters
        self.pipeline.render(&mut frame, bounds, &self.params);
        
        vec![frame.into_geometry()]
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
