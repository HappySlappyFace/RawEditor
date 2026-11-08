//! Point-curve editor: a canvas over which the active channel's curve is
//! drawn and edited.
//!
//! - click empty space: add a point there and start dragging it
//! - drag a point: move it (constrained between its neighbours)
//! - double-click or right-click a point: delete it (end points stay)
//!
//! Every change emits the whole new curve (`Message::CurveChanged`), so the
//! handler is a plain assignment. A drag commits one undo step on release.

use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Frame, Path, Stroke};
use iced::{Color, Point, Rectangle, Size};

use crate::app::message::Message;
use crate::core::curves::{Curve, CurveChannel, ToneCurves};
use super::theme;

/// Inset so points at 0 and 1 are fully visible and grabbable.
const PAD: f32 = 8.0;
/// Hit radius for grabbing a point, in screen pixels.
const HIT: f32 = 9.0;
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(350);

pub struct CurveEditor {
    pub curves: ToneCurves,
    pub channel: CurveChannel,
    pub histogram: [[u32; 256]; 3],
}

#[derive(Default)]
pub struct EditorState {
    dragging: Option<usize>,
    last_click: Option<(std::time::Instant, usize)>,
}

fn channel_color(ch: CurveChannel, alpha: f32) -> Color {
    match ch {
        CurveChannel::Master => Color::from_rgba(0.92, 0.92, 0.92, alpha),
        CurveChannel::Red => Color::from_rgba(0.95, 0.35, 0.30, alpha),
        CurveChannel::Green => Color::from_rgba(0.35, 0.85, 0.40, alpha),
        CurveChannel::Blue => Color::from_rgba(0.40, 0.55, 1.0, alpha),
    }
}

/// The plot area inside `size`.
fn plot(size: Size) -> Rectangle {
    Rectangle {
        x: PAD,
        y: PAD,
        width: (size.width - 2.0 * PAD).max(1.0),
        height: (size.height - 2.0 * PAD).max(1.0),
    }
}

fn to_screen(r: &Rectangle, x: f32, y: f32) -> Point {
    Point::new(r.x + x * r.width, r.y + (1.0 - y) * r.height)
}

fn to_curve(r: &Rectangle, p: Point) -> (f32, f32) {
    (
        ((p.x - r.x) / r.width).clamp(0.0, 1.0),
        (1.0 - (p.y - r.y) / r.height).clamp(0.0, 1.0),
    )
}

impl CurveEditor {
    fn curve(&self) -> &Curve {
        self.curves.get(self.channel)
    }

    fn hit(&self, r: &Rectangle, p: Point) -> Option<usize> {
        self.curve()
            .live()
            .iter()
            .enumerate()
            .map(|(i, q)| (i, to_screen(r, q[0], q[1]).distance(p)))
            .filter(|(_, d)| *d <= HIT)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    fn changed(&self, curve: Curve, commit: bool) -> Message {
        Message::CurveChanged(self.channel, curve, commit)
    }
}

impl canvas::Program<Message> for CurveEditor {
    type State = EditorState;

    fn update(
        &self,
        state: &mut EditorState,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use canvas::event::Status;
        let r = plot(bounds.size());
        // Relative cursor even outside the widget, so a drag that strays past
        // the edge keeps tracking (clamped to the square) instead of freezing.
        let rel = cursor.position().map(|p| Point::new(p.x - bounds.x, p.y - bounds.y));
        let inside = cursor.position_in(bounds).is_some();

        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if inside => {
                let Some(p) = rel else { return (Status::Ignored, None) };
                if let Some(i) = self.hit(&r, p) {
                    let now = std::time::Instant::now();
                    let double = state
                        .last_click
                        .is_some_and(|(t, j)| j == i && now.duration_since(t) < DOUBLE_CLICK);
                    state.last_click = Some((now, i));
                    if double {
                        state.dragging = None;
                        let mut c = *self.curve();
                        let n = c.count as usize;
                        if i > 0 && i + 1 < n {
                            c.remove(i);
                            return (Status::Captured, Some(self.changed(c, true)));
                        }
                        return (Status::Captured, None);
                    }
                    state.dragging = Some(i);
                    return (Status::Captured, None);
                }
                let (x, y) = to_curve(&r, p);
                let mut c = *self.curve();
                if let Some(i) = c.insert(x, y) {
                    state.dragging = Some(i);
                    state.last_click = Some((std::time::Instant::now(), i));
                    return (Status::Captured, Some(self.changed(c, false)));
                }
                (Status::Captured, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) if inside => {
                let Some(p) = rel else { return (Status::Ignored, None) };
                let mut c = *self.curve();
                match self.hit(&r, p) {
                    Some(i) if i > 0 && i + 1 < c.count as usize => {
                        c.remove(i);
                        (Status::Captured, Some(self.changed(c, true)))
                    }
                    _ => (Status::Captured, None),
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let (Some(i), Some(p)) = (state.dragging, rel) else {
                    return (Status::Ignored, None);
                };
                let (x, y) = to_curve(&r, p);
                let mut c = *self.curve();
                c.move_point(i, x, y);
                (Status::Captured, Some(self.changed(c, false)))
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.dragging.take().is_some() {
                    return (Status::Captured, Some(Message::CommitEdit));
                }
                (Status::Ignored, None)
            }
            _ => (Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        state: &EditorState,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let r = plot(bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::colours().bg);

        // Histogram of the edited channel (luma-ish max for the master).
        let hist: Vec<f32> = (0..256)
            .map(|i| match self.channel {
                CurveChannel::Master => {
                    self.histogram.iter().map(|c| c[i]).max().unwrap_or(0) as f32
                }
                CurveChannel::Red => self.histogram[0][i] as f32,
                CurveChannel::Green => self.histogram[1][i] as f32,
                CurveChannel::Blue => self.histogram[2][i] as f32,
            })
            .collect();
        // sqrt scale: a linear histogram is one spike and a flat line.
        let max = hist.iter().cloned().fold(0.0f32, f32::max).sqrt();
        if max > 0.0 {
            let path = Path::new(|b| {
                b.move_to(to_screen(&r, 0.0, 0.0));
                for (i, v) in hist.iter().enumerate() {
                    b.line_to(to_screen(&r, i as f32 / 255.0, v.sqrt() / max * 0.9));
                }
                b.line_to(to_screen(&r, 1.0, 0.0));
                b.close();
            });
            frame.fill(&path, channel_color(self.channel, 0.10));
        }

        // Quarter grid + identity diagonal.
        let grid = Stroke::default().with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.08)).with_width(1.0);
        for k in 1..4 {
            let t = k as f32 / 4.0;
            frame.stroke(&Path::line(to_screen(&r, t, 0.0), to_screen(&r, t, 1.0)), grid);
            frame.stroke(&Path::line(to_screen(&r, 0.0, t), to_screen(&r, 1.0, t)), grid);
        }
        frame.stroke(&Path::line(to_screen(&r, 0.0, 0.0), to_screen(&r, 1.0, 1.0)), grid);
        frame.stroke(
            &Path::rectangle(Point::new(r.x, r.y), r.size()),
            Stroke::default().with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.15)).with_width(1.0),
        );

        let curve_path = |c: &Curve| {
            Path::new(|b| {
                for i in 0..=128 {
                    let x = i as f32 / 128.0;
                    let p = to_screen(&r, x, c.eval(x));
                    if i == 0 { b.move_to(p) } else { b.line_to(p) }
                }
            })
        };

        // Other channels' edits, ghosted, so a colour curve is never invisible.
        for ch in CurveChannel::ALL {
            if ch != self.channel && !self.curves.get(ch).is_identity() {
                frame.stroke(
                    &curve_path(self.curves.get(ch)),
                    Stroke::default().with_color(channel_color(ch, 0.35)).with_width(1.0),
                );
            }
        }
        frame.stroke(
            &curve_path(self.curve()),
            Stroke::default().with_color(channel_color(self.channel, 0.95)).with_width(1.6),
        );

        let hover = cursor
            .position_in(bounds)
            .and_then(|p| self.hit(&r, p));
        for (i, q) in self.curve().live().iter().enumerate() {
            let p = to_screen(&r, q[0], q[1]);
            let active = state.dragging == Some(i) || hover == Some(i);
            let s = if active { 9.0 } else { 7.0 };
            let fill = if active { theme::colours().accent } else { Color::WHITE };
            frame.fill_rectangle(Point::new(p.x - s / 2.0, p.y - s / 2.0), Size::new(s, s), fill);
            frame.stroke(
                &Path::rectangle(Point::new(p.x - s / 2.0, p.y - s / 2.0), Size::new(s, s)),
                Stroke::default().with_color(Color::BLACK).with_width(1.0),
            );
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &EditorState, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        if state.dragging.is_some() {
            return mouse::Interaction::Grabbing;
        }
        match cursor.position_in(bounds) {
            Some(p) if self.hit(&plot(bounds.size()), p).is_some() => mouse::Interaction::Grab,
            Some(_) => mouse::Interaction::Crosshair,
            None => mouse::Interaction::default(),
        }
    }
}
