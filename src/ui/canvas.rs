use iced::widget::canvas::{self, Program};
use iced::mouse::{self, Cursor};
use iced::{Rectangle, Renderer, Theme, Point};
use std::sync::Arc;

use crate::gpu;
use crate::Message;

/// GPU-accelerated canvas renderer for RAW images
/// Phase 25: Direct wgpu rendering with zoom/pan support
pub struct GpuRenderer {
    /// The GPU rendering pipeline
    pub pipeline: Arc<gpu::RenderPipeline>,
    /// Zoom level (1.0 = 100%)
    pub zoom: f32,
    /// Pan offset in normalized coordinates
    pub offset: cgmath::Vector2<f32>,
}

impl Program<Message> for GpuRenderer {
    type State = DragState;

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<canvas::Geometry> {
        // Phase 25: CRITICAL - Direct GPU rendering to screen!
        // This is where the magic happens - zero CPU readback!
        
        // Get wgpu backend from iced renderer
        // Note: This will be a direct call to render_to_target in pipeline.rs
        // The Canvas::draw() in iced calls this, and we'll hook into wgpu directly
        
        // For now, return empty geometry - the actual rendering happens
        // via custom primitive/layer in iced's rendering pipeline
        // TODO: Integrate with iced's wgpu backend using custom layer
        
        vec![]
    }

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        _bounds: Rectangle,
        cursor: Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        // Phase 25: Handle zoom and pan interactions
        match event {
            // Mouse wheel for zooming
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let zoom_delta = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y * 0.1,
                    mouse::ScrollDelta::Pixels { y, .. } => y * 0.01,
                };
                return (canvas::event::Status::Captured, Some(Message::Zoom(zoom_delta)));
            }
            
            // Mouse button press - start dragging
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(pos) = cursor.position() {
                    state.is_dragging = true;
                    state.last_position = Some(pos);
                    return (canvas::event::Status::Captured, None);
                }
            }
            
            // Mouse button release - stop dragging
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.is_dragging = false;
                state.last_position = None;
                return (canvas::event::Status::Captured, None);
            }
            
            // Mouse move - pan if dragging
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.is_dragging {
                    if let Some(current_pos) = cursor.position() {
                        if let Some(last_pos) = state.last_position {
                            // Calculate pan delta in screen space
                            let delta_x = current_pos.x - last_pos.x;
                            let delta_y = current_pos.y - last_pos.y;
                            
                            // Convert to normalized coordinates (adjust for zoom)
                            let delta = cgmath::Vector2::new(
                                delta_x * 0.001, // Sensitivity factor
                                delta_y * 0.001,
                            );
                            
                            state.last_position = Some(current_pos);
                            return (canvas::event::Status::Captured, Some(Message::Pan(delta)));
                        }
                    }
                }
            }
            
            _ => {}
        }
        
        (canvas::event::Status::Ignored, None)
    }
}

/// State for drag interactions
#[derive(Debug, Clone, Default)]
pub struct DragState {
    pub is_dragging: bool,
    pub last_position: Option<Point>,
}
