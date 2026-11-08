//! The detail loupe's drag surface (`handlers::loupe`).
//!
//! Sits over the loupe image and turns press-and-drag into
//! `Message::LoupePan`. It emits nothing while the pointer merely hovers: a
//! message per mouse move rebuilds the whole view (CLAUDE.md §E), so only an
//! actual drag — which, like a slider's, is meant to update — sends any.

use iced::mouse::{self, Cursor};
use iced::widget::canvas;
use iced::{Point, Rectangle};

use crate::app::message::Message;

pub struct LoupeDrag {
    /// The message a drag of `(dx, dy)` logical pixels sends.
    pub on_pan: fn(f32, f32) -> Message,
}

#[derive(Debug, Default)]
pub struct DragState {
    /// The pointer's last position in window coordinates — the
    /// `CursorMoved` event's own. Never `cursor.position()`: inside the
    /// sidebar's scrollable that is shifted by the scroll offset, so mixing
    /// the two made every drag's first move jump by it.
    pointer: Option<Point>,
    dragging: bool,
}

impl DragState {
    /// Feed one event; the pan it makes, in logical pixels, if any.
    fn on(&mut self, event: &mouse::Event, over: bool) -> Option<Option<(f32, f32)>> {
        match *event {
            mouse::Event::ButtonPressed(mouse::Button::Left) if over => {
                self.dragging = true;
                Some(None)
            }
            // Followed past the loupe's edge too: the canvas sees every move.
            mouse::Event::CursorMoved { position } => {
                let last = self.pointer.replace(position);
                if !self.dragging {
                    return None;
                }
                Some(last.map(|l| (position.x - l.x, position.y - l.y)))
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) if self.dragging => {
                self.dragging = false;
                Some(None)
            }
            _ => None,
        }
    }
}

impl canvas::Program<Message> for LoupeDrag {
    type State = DragState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use canvas::event::Status;
        let canvas::Event::Mouse(event) = event else { return (Status::Ignored, None) };
        match state.on(&event, cursor.is_over(bounds)) {
            Some(pan) => (Status::Captured, pan.map(|(dx, dy)| (self.on_pan)(dx, dy))),
            None => (Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        _renderer: &iced::Renderer,
        _theme: &iced::Theme,
        _bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<canvas::Geometry> {
        Vec::new()
    }

    fn mouse_interaction(&self, state: &Self::State, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        if state.dragging {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(x: f32, y: f32) -> mouse::Event {
        mouse::Event::CursorMoved { position: Point::new(x, y) }
    }

    /// A drag moves by exactly what the pointer moved, from its first move —
    /// whatever the press's `cursor` said (a scrolled sidebar shifts it).
    #[test]
    fn a_drag_pans_by_the_pointer_movement_only() {
        let mut s = DragState::default();
        assert_eq!(s.on(&moved(100.0, 200.0), false), None, "hovering emitted");
        assert_eq!(s.on(&mouse::Event::ButtonPressed(mouse::Button::Left), true), Some(None));
        assert_eq!(s.on(&moved(103.0, 198.0), true), Some(Some((3.0, -2.0))));
        // Past the loupe's edge, still followed.
        assert_eq!(s.on(&moved(90.0, 250.0), false), Some(Some((-13.0, 52.0))));
        assert_eq!(s.on(&mouse::Event::ButtonReleased(mouse::Button::Left), false), Some(None));
        assert_eq!(s.on(&moved(0.0, 0.0), false), None);
    }

    /// A press outside the loupe is someone else's.
    #[test]
    fn a_press_elsewhere_is_ignored() {
        let mut s = DragState::default();
        assert_eq!(s.on(&mouse::Event::ButtonPressed(mouse::Button::Left), false), None);
        assert_eq!(s.on(&moved(5.0, 5.0), false), None);
    }
}
