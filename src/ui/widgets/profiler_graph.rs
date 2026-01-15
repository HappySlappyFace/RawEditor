use crate::app::message::Message;
use crate::core::profiler::Profiler;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke};
use iced::widget::{column, container, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

pub fn view_profiler_overlay(profiler: &Profiler) -> Element<'_, Message> {
    let avg_total = if profiler.history.is_empty() {
        0.0
    } else {
        profiler.history.iter().map(|f| f.total_ms).sum::<f32>() / profiler.history.len() as f32
    };

    let color = if avg_total < 16.6 {
        Color::from_rgb(0.0, 1.0, 0.0) // Green
    } else if avg_total < 33.3 {
        Color::from_rgb(1.0, 1.0, 0.0) // Yellow
    } else {
        Color::from_rgb(1.0, 0.0, 0.0) // Red
    };

    let header = text(format!("Avg Latency: {:.1} ms", avg_total))
        .size(14)
        .style(move |_| iced::widget::text::Style { color: Some(color) });

    let legend = text("Blue: CPU | Yellow: Upload | Red: GPU")
        .size(10)
        .style(|_| iced::widget::text::Style {
            color: Some(Color::WHITE),
        });

    let graph = Canvas::new(ProfilerGraph { profiler })
        .width(Length::Fill)
        .height(Length::Fixed(100.0));

    container(column![header, graph, legend].spacing(5).padding(10))
        .style(|_| container::Style {
            background: Some(iced::Background::Color(Color::from_rgba(
                0.0, 0.0, 0.0, 0.7,
            ))),
            border: iced::Border {
                radius: 5.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .width(Length::Fixed(300.0))
        .into()
}

struct ProfilerGraph<'a> {
    profiler: &'a Profiler,
}

impl<'a> canvas::Program<Message> for ProfilerGraph<'a> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        if self.profiler.history.is_empty() {
            return vec![frame.into_geometry()];
        }

        let bar_width = bounds.width / self.profiler.capacity as f32;
        let max_ms = 100.0; // Fixed scale for now, or dynamic? Fixed is better for comparison.
        let scale_y = bounds.height / max_ms;

        for (i, frame_data) in self.profiler.history.iter().enumerate() {
            let x = i as f32 * bar_width;

            // CPU (Blue)
            let h_cpu = frame_data.update_ms * scale_y;
            frame.fill_rectangle(
                Point::new(x, bounds.height - h_cpu),
                Size::new(bar_width, h_cpu),
                Color::from_rgb(0.2, 0.2, 1.0),
            );

            // Upload (Yellow)
            let h_upload = frame_data.upload_ms * scale_y;
            frame.fill_rectangle(
                Point::new(x, bounds.height - h_cpu - h_upload),
                Size::new(bar_width, h_upload),
                Color::from_rgb(1.0, 1.0, 0.0),
            );

            // Render (Red)
            let h_render = frame_data.render_ms * scale_y;
            frame.fill_rectangle(
                Point::new(x, bounds.height - h_cpu - h_upload - h_render),
                Size::new(bar_width, h_render),
                Color::from_rgb(1.0, 0.0, 0.0),
            );
        }

        // 16ms line (60 FPS)
        let y_16ms = bounds.height - (16.6 * scale_y);
        frame.stroke(
            &Path::line(Point::new(0.0, y_16ms), Point::new(bounds.width, y_16ms)),
            Stroke::default()
                .with_color(Color::from_rgba(0.0, 1.0, 0.0, 0.5))
                .with_width(1.0),
        );

        // 33ms line (30 FPS)
        let y_33ms = bounds.height - (33.3 * scale_y);
        frame.stroke(
            &Path::line(Point::new(0.0, y_33ms), Point::new(bounds.width, y_33ms)),
            Stroke::default()
                .with_color(Color::from_rgba(1.0, 1.0, 0.0, 0.5))
                .with_width(1.0),
        );

        vec![frame.into_geometry()]
    }
}
