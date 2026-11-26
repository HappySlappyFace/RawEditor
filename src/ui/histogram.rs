/// Phase 21: Real-Time Histogram
/// Displays RGB histogram for visual exposure feedback
use iced::widget::canvas::{self, Stroke};
use iced::{Color, Point, Rectangle};

use crate::app::message::Message;

/// Histogram data structure
#[derive(Debug, Clone)]
pub struct Histogram {
    /// RGB histogram data: [R[256], G[256], B[256]]
    pub data: [[u32; 256]; 3],
}

impl canvas::Program<Message> for Histogram {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // Scaled to the tallest bin but the clipped ends, which are drawn
        // clamped (`core::histogram::display_peak`).
        let max_value = crate::core::histogram::display_peak(&self.data) as f32;

        if max_value < 1.0 {
            return vec![frame.into_geometry()];
        }

        let width = bounds.width;
        let height = bounds.height;
        let bar_width = width / 256.0;

        // Draw three histogram channels (R, G, B)
        let colors = [
            Color::from_rgba(1.0, 0.0, 0.0, 0.5), // Red
            Color::from_rgba(0.0, 1.0, 0.0, 0.5), // Green
            Color::from_rgba(0.0, 0.0, 1.0, 0.5), // Blue
        ];

        for (channel_idx, channel_data) in self.data.iter().enumerate() {
            let mut path_builder = canvas::path::Builder::new();

            for (i, &count) in channel_data.iter().enumerate() {
                if count > 0 {
                    let normalized = (count as f32 / max_value).min(1.0);
                    let bar_height = normalized * height;
                    let x = i as f32 * bar_width;
                    let y = height - bar_height;

                    // Draw vertical line for this bin
                    path_builder.move_to(Point::new(x, height));
                    path_builder.line_to(Point::new(x, y));
                }
            }

            let path = path_builder.build();
            frame.stroke(
                &path,
                Stroke::default()
                    .with_color(colors[channel_idx])
                    .with_width(bar_width.max(1.0)),
            );
        }

        vec![frame.into_geometry()]
    }
}
