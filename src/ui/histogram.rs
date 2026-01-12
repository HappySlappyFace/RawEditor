/// Phase 21: Real-Time Histogram
/// Displays RGB histogram for visual exposure feedback
use iced::widget::canvas::{self, Stroke};
use iced::{Color, Point, Rectangle};

use crate::app::message::Message;

/// Histogram data structure
#[derive(Debug, Clone)]
pub struct Histogram {
    /// Histogram data
    pub data: crate::core::histogram::HistogramData,
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

        // Find maximum value across all channels for normalization
        let max_r = self.data.r.iter().max().copied().unwrap_or(0);
        let max_g = self.data.g.iter().max().copied().unwrap_or(0);
        let max_b = self.data.b.iter().max().copied().unwrap_or(0);
        let max_value = max_r.max(max_g).max(max_b).max(1) as f32;

        if max_value < 1.0 {
            return vec![frame.into_geometry()];
        }

        let width = bounds.width;
        let height = bounds.height;
        let bar_width = width / 256.0;

        // Draw three histogram channels (R, G, B)
        let channels = [
            (&self.data.r, Color::from_rgba(1.0, 0.0, 0.0, 0.5)),
            (&self.data.g, Color::from_rgba(0.0, 1.0, 0.0, 0.5)),
            (&self.data.b, Color::from_rgba(0.0, 0.0, 1.0, 0.5)),
        ];

        for (channel_data, color) in channels.iter() {
            let mut path_builder = canvas::path::Builder::new();

            for (i, &count) in channel_data.iter().enumerate() {
                if count > 0 {
                    let normalized = count as f32 / max_value;
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
                    .with_color(*color)
                    .with_width(bar_width.max(1.0)),
            );
        }

        vec![frame.into_geometry()]
    }
}
