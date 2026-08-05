use crate::app::message::Message;
use crate::core::profiler::Profiler;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke};
use iced::widget::{column, container, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

/// Phase 105: Accept the shared canvas::Cache so the caller controls invalidation.
/// What the render is currently being asked to produce — shown alongside the
/// timings so the cost can be read against the work, rather than guessed at.
pub struct RenderStats {
    /// Magnification where 100% is one sensor pixel per screen pixel.
    pub zoom_percent: f32,
    /// Size of the render target, i.e. the windowed region actually drawn.
    pub target: (u32, u32),
    /// Fraction of the image that window covers, 0..1 on each axis.
    pub view_extent: (f32, f32),
}

/// `stats` is `None` outside the Develop tab: zoom, render-target size and
/// window extent all describe the develop render specifically, and reporting
/// the last develop frame's numbers while the user is looking at the Library
/// would be worse than reporting nothing. The timing graph itself is
/// tab-agnostic and always shown.
pub fn view_profiler_overlay<'a>(
    profiler: &'a Profiler,
    cache: &'a iced::widget::canvas::Cache,
    stats: Option<RenderStats>,
) -> Element<'a, Message> {
    let avg_total = if profiler.history.is_empty() {
        0.0
    } else {
        profiler.history.iter().map(|f| f.total_ms).sum::<f32>() / profiler.history.len() as f32
    };

    let avg_color = if avg_total < 16.6 {
        Color::from_rgb(0.0, 1.0, 0.0) // Green — 60 fps
    } else if avg_total < 33.3 {
        Color::from_rgb(1.0, 1.0, 0.0) // Yellow — 30 fps
    } else {
        Color::from_rgb(1.0, 0.3, 0.3) // Red — below 30 fps
    };

    let avg_header = text(format!("Avg: {:.1} ms", avg_total))
        .size(13)
        .style(move |_| iced::widget::text::Style {
            color: Some(avg_color),
        });

    // Phase 105: Per-frame breakdown from the most recent sample
    let frame_detail = if let Some(last) = profiler.history.back() {
        text(format!(
            "CPU {:.1}ms  |  Encode {:.1}ms  |  GPU {:.1}ms  |  Total {:.1}ms",
            last.update_ms, last.upload_ms, last.render_ms, last.total_ms
        ))
        .size(11)
        .style(|_| iced::widget::text::Style {
            color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.85)),
        })
    } else {
        text("CPU --  |  Encode --  |  GPU --  |  Total --")
            .size(11)
            .style(|_| iced::widget::text::Style {
                color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.5)),
            })
    };

    let render_stats = match &stats {
        Some(s) => {
            let megapixels = (s.target.0 as f64 * s.target.1 as f64) / 1.0e6;
            text(format!(
                "Zoom {:.0}%  |  Render {}×{} ({:.2} MP)  |  Window {:.0}%×{:.0}%",
                s.zoom_percent,
                s.target.0,
                s.target.1,
                megapixels,
                s.view_extent.0 * 100.0,
                s.view_extent.1 * 100.0,
            ))
        }
        None => text("Render stats available in Develop"),
    }
    .size(11)
    .style(|_| iced::widget::text::Style {
        color: Some(Color::from_rgba(0.98, 0.45, 0.09, 0.95)),
    });

    // "Upload" is CPU-side resource creation and command recording; every
    // submit and the readback wait are counted under GPU.
    let legend = text("■ CPU   ■ Encode   ■ GPU")
        .size(10)
        .style(|_| iced::widget::text::Style {
            color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.6)),
        });

    let graph = Canvas::new(ProfilerGraph { profiler, cache })
        .width(Length::Fill)
        .height(Length::Fixed(100.0));

    container(
        column![avg_header, frame_detail, render_stats, graph, legend]
            .spacing(4)
            .padding(10),
    )
    .style(|_| container::Style {
        background: Some(iced::Background::Color(Color::from_rgba(
            0.05, 0.05, 0.05, 0.82,
        ))),
        border: iced::Border {
            radius: 6.0.into(),
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.15),
            width: 1.0,
        },
        ..Default::default()
    })
    .width(Length::Fixed(340.0))
    .into()
}

struct ProfilerGraph<'a> {
    profiler: &'a Profiler,
    cache: &'a iced::widget::canvas::Cache,
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
        // Phase 105: Only re-rasterise when the cache has been cleared (i.e. new data arrived).
        let geometry = self.cache.draw(renderer, bounds.size(), |frame: &mut Frame| {
            if self.profiler.history.is_empty() {
                return;
            }

            let bar_width = bounds.width / self.profiler.capacity as f32;
            // Fixed 100 ms scale — keeps the graph stable and comparable frame-to-frame.
            let max_ms = 100.0_f32;
            let scale_y = bounds.height / max_ms;

            for (i, frame_data) in self.profiler.history.iter().enumerate() {
                let x = i as f32 * bar_width;

                // CPU update (blue)
                let h_cpu = (frame_data.update_ms * scale_y).min(bounds.height);
                frame.fill_rectangle(
                    Point::new(x, bounds.height - h_cpu),
                    Size::new(bar_width - 0.5, h_cpu),
                    Color::from_rgb(0.25, 0.45, 1.0),
                );

                // PCIe Upload (yellow)
                let h_upload =
                    (frame_data.upload_ms * scale_y).min(bounds.height - h_cpu);
                frame.fill_rectangle(
                    Point::new(x, bounds.height - h_cpu - h_upload),
                    Size::new(bar_width - 0.5, h_upload),
                    Color::from_rgb(1.0, 0.85, 0.0),
                );

                // GPU Render (red)
                let h_render =
                    (frame_data.render_ms * scale_y).min(bounds.height - h_cpu - h_upload);
                frame.fill_rectangle(
                    Point::new(x, bounds.height - h_cpu - h_upload - h_render),
                    Size::new(bar_width - 0.5, h_render),
                    Color::from_rgb(1.0, 0.25, 0.25),
                );
            }

            // 16.6 ms reference line (60 FPS)
            let y_16ms = bounds.height - (16.6 * scale_y);
            frame.stroke(
                &Path::line(Point::new(0.0, y_16ms), Point::new(bounds.width, y_16ms)),
                Stroke::default()
                    .with_color(Color::from_rgba(0.0, 1.0, 0.0, 0.55))
                    .with_width(1.0),
            );

            // 33.3 ms reference line (30 FPS)
            let y_33ms = bounds.height - (33.3 * scale_y);
            frame.stroke(
                &Path::line(Point::new(0.0, y_33ms), Point::new(bounds.width, y_33ms)),
                Stroke::default()
                    .with_color(Color::from_rgba(1.0, 1.0, 0.0, 0.55))
                    .with_width(1.0),
            );
        });

        vec![geometry]
    }
}
