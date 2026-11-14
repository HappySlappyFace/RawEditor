//! A thin indeterminate progress line: a short segment glides along a faint
//! track and starts over, for work whose length is unknown (the start-up
//! splash). It animates itself on `RedrawRequested` like the other widgets,
//! and with reduced motion on it stands still as a filled line.

use std::time::Instant;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{tree, Tree, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::event::{self, Event};
use iced::{mouse, window};
use iced::{Border, Color, Element, Length, Rectangle, Size, Theme};

use crate::ui::motion::{self, ease};

type Renderer = iced::Renderer;

/// Seconds for the segment to cross the track once.
const PERIOD: f32 = 1.7;
/// The segment's length as a fraction of the track.
const SEGMENT: f32 = 0.3;

pub struct SweepBar {
    height: f32,
    track: Color,
    fill: Color,
}

pub fn sweep_bar(height: f32, track: Color, fill: Color) -> SweepBar {
    SweepBar { height, track, fill }
}

#[derive(Default)]
struct State {
    start: Option<Instant>,
    now: Option<Instant>,
}

/// The segment's span along a track of `width`, at linear `phase` in `[0, 1)`:
/// it enters from beyond the left end and leaves past the right, eased so it
/// speeds up through the middle. `None` while it is wholly off the track.
fn span(phase: f32, width: f32) -> Option<(f32, f32)> {
    let len = width * SEGMENT;
    let x = motion::lerp(-len, width, ease::STANDARD.ease(phase.clamp(0.0, 1.0)));
    let (a, b) = (x.max(0.0), (x + len).min(width));
    (b > a).then_some((a, b))
}

impl<Message> Widget<Message, Theme, Renderer> for SweepBar {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height))
    }

    fn layout(&self, _tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::atomic(limits, Length::Fill, Length::Fixed(self.height))
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) -> event::Status {
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<State>();
            state.start.get_or_insert(now);
            state.now = Some(now);
            if !motion::reduced() {
                shell.request_redraw(window::RedrawRequest::NextFrame);
            }
        }
        event::Status::Ignored
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let border = Border { radius: (bounds.height / 2.0).into(), ..Border::default() };
        renderer.fill_quad(
            renderer::Quad { bounds, border, ..renderer::Quad::default() },
            self.track,
        );

        let state = tree.state.downcast_ref::<State>();
        let segment = if motion::reduced() {
            Some((0.0, bounds.width))
        } else {
            match (state.start, state.now) {
                (Some(start), Some(now)) => {
                    let phase = (motion::elapsed(start, now) / PERIOD).fract();
                    span(phase, bounds.width)
                }
                _ => None,
            }
        };
        if let Some((a, b)) = segment {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle { x: bounds.x + a, width: b - a, ..bounds },
                    border,
                    ..renderer::Quad::default()
                },
                self.fill,
            );
        }
    }
}

impl<'a, Message: 'a> From<SweepBar> for Element<'a, Message> {
    fn from(bar: SweepBar) -> Self {
        Self::new(bar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_segment_enters_crosses_and_leaves() {
        let w = 200.0;
        // Just started: wholly off the left end.
        assert_eq!(span(0.0, w), None);
        // Somewhere along the way it is wholly on the track, at its full length.
        let full = (0..=100).filter_map(|i| span(i as f32 / 100.0, w)).any(|(a, b)| {
            (b - a - w * SEGMENT).abs() < 1.0
        });
        assert!(full, "the segment is never seen at its full length");
        // Never outside the track, anywhere.
        for i in 0..=100 {
            if let Some((a, b)) = span(i as f32 / 100.0, w) {
                assert!(a >= 0.0 && b <= w && b > a);
            }
        }
    }
}
