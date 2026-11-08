//! The edit sidebar's slider: iced 0.13's `Slider` (MIT), for `f32`, with
//! two additions.
//!
//! - **A gradient rail** (`gradient`): colour stops drawn along the whole
//!   track, saying what the slider does before it is touched: darker → lighter
//!   for Exposure, blue → yellow for Temperature, green → magenta for Tint.
//! - **Fine control**: holding Alt (or Shift, for desktops where Alt-drag moves
//!   the window) moves the value [`FINE`]× slower. Dragging is relative to
//!   where the drag (or the last modifier change) began, so pressing or
//!   releasing the key mid-drag never jumps the value to the cursor; an
//!   Alt-press on the track starts a fine drag from the current value instead
//!   of jumping to the click.
//!
//! Same builder and style (`iced::widget::slider::Style`) as iced's, so
//! `ui::styles::ProSlider::style` applies unchanged.

use std::ops::RangeInclusive;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{tree, Tree, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::event::{self, Event};
use iced::keyboard;
use iced::mouse;
use iced::widget::slider::{HandleShape, Status, Style};
use iced::{gradient, Border, Color, Element, Length, Point, Radians, Rectangle, Size, Theme};

/// How much slower a drag moves with the fine-control modifier held.
pub const FINE: f32 = 10.0;

pub struct Slider<'a, Message> {
    range: RangeInclusive<f32>,
    step: f32,
    value: f32,
    default: Option<f32>,
    on_change: Box<dyn Fn(f32) -> Message + 'a>,
    on_release: Option<Message>,
    width: Length,
    height: f32,
    gradient: Option<&'a [Color]>,
    style: StyleFn<'a>,
}

type StyleFn<'a> = Box<dyn Fn(&Theme, Status) -> Style + 'a>;

pub fn slider<'a, Message: Clone>(
    range: RangeInclusive<f32>,
    value: f32,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Slider<'a, Message> {
    Slider::new(range, value, on_change)
}

impl<'a, Message: Clone> Slider<'a, Message> {
    pub const DEFAULT_HEIGHT: f32 = 16.0;

    pub fn new(range: RangeInclusive<f32>, value: f32, on_change: impl Fn(f32) -> Message + 'a) -> Self {
        let (lo, hi) = (*range.start(), *range.end());
        let value = value.clamp(lo, hi.max(lo));
        Self {
            range,
            step: 1.0,
            value,
            default: None,
            on_change: Box::new(on_change),
            on_release: None,
            width: Length::Fill,
            height: Self::DEFAULT_HEIGHT,
            gradient: None,
            style: Box::new(iced::widget::slider::default),
        }
    }

    /// Ctrl/Cmd-click resets to this.
    pub fn default(mut self, default: f32) -> Self {
        self.default = Some(default);
        self
    }

    pub fn on_release(mut self, message: Message) -> Self {
        self.on_release = Some(message);
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    /// Colour stops spread evenly along the rail, left to right. The rail
    /// then draws as one gradient instead of the filled / unfilled halves.
    pub fn gradient(mut self, stops: &'a [Color]) -> Self {
        self.gradient = (!stops.is_empty()).then_some(stops);
        self
    }

    pub fn style(mut self, style: impl Fn(&Theme, Status) -> Style + 'a) -> Self {
        self.style = Box::new(style);
        self
    }
}

/// An ongoing drag: the cursor x and value it is measured from, and whether
/// it is in fine mode. Re-anchored whenever fine mode toggles.
#[derive(Debug, Clone, Copy)]
struct Drag {
    anchor_x: f32,
    anchor_value: f32,
    fine: bool,
}

#[derive(Debug, Default)]
struct State {
    drag: Option<Drag>,
    modifiers: keyboard::Modifiers,
}

fn fine_held(m: keyboard::Modifiers) -> bool {
    m.alt() || m.shift()
}

/// Snap `value` to multiples of `step` from `start`, inside the range.
fn snap(value: f32, start: f32, end: f32, step: f32) -> f32 {
    let v = if step > 0.0 { start + ((value - start) / step).round() * step } else { value };
    v.clamp(start, end)
}

/// The value a drag gives at cursor `x`, on a track `travel` pixels long.
/// Pure, so the drag rules are unit-tested without a window.
fn drag_value(d: Drag, x: f32, travel: f32, start: f32, end: f32, step: f32) -> f32 {
    let speed = if d.fine { 1.0 / FINE } else { 1.0 };
    let raw = d.anchor_value + (x - d.anchor_x) / travel.max(1.0) * (end - start) * speed;
    snap(raw, start, end, if d.fine { step / FINE } else { step })
}

impl<Message: Clone> Slider<'_, Message> {
    fn handle_width(&self, theme: &Theme, bounds: Rectangle) -> f32 {
        match (self.style)(theme, Status::Active).handle.shape {
            HandleShape::Circle { radius } => radius * 2.0,
            HandleShape::Rectangle { width, .. } => f32::from(width).min(bounds.width),
        }
    }

    /// The value under cursor `x` (a plain click), as iced places it: the
    /// handle's centre tracks the cursor.
    fn locate(&self, x: f32, bounds: Rectangle, handle: f32) -> f32 {
        let (start, end) = (*self.range.start(), *self.range.end());
        let travel = (bounds.width - handle).max(1.0);
        let t = ((x - bounds.x - handle / 2.0) / travel).clamp(0.0, 1.0);
        snap(start + t * (end - start), start, end, self.step)
    }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for Slider<'_, Message>
where
    Message: Clone,
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size { width: self.width, height: Length::Shrink }
    }

    fn layout(&self, _tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::atomic(limits, self.width, self.height)
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) -> event::Status {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let (start, end) = (*self.range.start(), *self.range.end());
        // The style's handle size depends only on the style, not the theme
        // instance, so the default theme is enough to measure it.
        let handle = self.handle_width(&Theme::Dark, bounds);
        let travel = bounds.width - handle;

        let mut change = |slider: &mut Self, v: f32| {
            if (slider.value - v).abs() > f32::EPSILON {
                slider.value = v;
                shell.publish((slider.on_change)(v));
            }
        };

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(pos) = cursor.position_over(bounds) else { return event::Status::Ignored };
                if state.modifiers.command() {
                    if let Some(d) = self.default {
                        change(self, d);
                    }
                    state.drag = None;
                } else {
                    let fine = fine_held(state.modifiers);
                    if !fine {
                        let v = self.locate(pos.x, bounds, handle);
                        change(self, v);
                    }
                    state.drag = Some(Drag { anchor_x: pos.x, anchor_value: self.value, fine });
                }
                return event::Status::Captured;
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.drag.take().is_some() {
                    if let Some(m) = self.on_release.clone() {
                        shell.publish(m);
                    }
                    return event::Status::Captured;
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(mut d) = state.drag {
                    let fine = fine_held(state.modifiers);
                    if fine != d.fine {
                        // Carry on from here at the new speed: no jump.
                        d = Drag { anchor_x: position.x, anchor_value: self.value, fine };
                        state.drag = Some(d);
                    }
                    let v = drag_value(d, position.x, travel, start, end, self.step);
                    change(self, v);
                    return event::Status::Captured;
                }
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                state.modifiers = m;
                if let (Some(d), Some(Point { x, .. })) = (state.drag, cursor.position()) {
                    let fine = fine_held(m);
                    if fine != d.fine {
                        state.drag = Some(Drag { anchor_x: x, anchor_value: self.value, fine });
                    }
                }
            }
            _ => {}
        }
        event::Status::Ignored
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let status = if state.drag.is_some() {
            Status::Dragged
        } else if cursor.is_over(bounds) {
            Status::Hovered
        } else {
            Status::Active
        };
        let style = (self.style)(theme, status);
        let (handle_w, handle_h, handle_radius) = match style.handle.shape {
            HandleShape::Circle { radius } => (radius * 2.0, radius * 2.0, radius.into()),
            HandleShape::Rectangle { width, border_radius } => (f32::from(width), bounds.height, border_radius),
        };
        let (start, end) = (*self.range.start(), *self.range.end());
        let offset = if start >= end { 0.0 } else { (bounds.width - handle_w) * (self.value - start) / (end - start) };
        let rail_y = bounds.y + bounds.height / 2.0;

        match self.gradient {
            Some(stops) => {
                // One gradient along the whole track; a touch taller than a
                // plain rail so the colour reads.
                let h = style.rail.width.max(5.0);
                let n = stops.len().max(2) - 1;
                let mut linear = gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_2));
                for (i, c) in stops.iter().enumerate() {
                    linear = linear.add_stop(if stops.len() == 1 { 0.0 } else { i as f32 / n as f32 }, *c);
                }
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle { x: bounds.x, y: rail_y - h / 2.0, width: bounds.width, height: h },
                        border: Border { radius: (h / 2.0).into(), ..style.rail.border },
                        ..renderer::Quad::default()
                    },
                    linear,
                );
            }
            None => {
                let h = style.rail.width;
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle { x: bounds.x, y: rail_y - h / 2.0, width: offset + handle_w / 2.0, height: h },
                        border: style.rail.border,
                        ..renderer::Quad::default()
                    },
                    style.rail.backgrounds.0,
                );
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle {
                            x: bounds.x + offset + handle_w / 2.0,
                            y: rail_y - h / 2.0,
                            width: bounds.width - offset - handle_w / 2.0,
                            height: h,
                        },
                        border: style.rail.border,
                        ..renderer::Quad::default()
                    },
                    style.rail.backgrounds.1,
                );
            }
        }

        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle { x: bounds.x + offset, y: rail_y - handle_h / 2.0, width: handle_w, height: handle_h },
                border: Border {
                    radius: handle_radius,
                    width: style.handle.border_width,
                    color: style.handle.border_color,
                },
                ..renderer::Quad::default()
            },
            style.handle.background,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        if state.drag.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<'a, Message, Renderer> From<Slider<'a, Message>> for Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(s: Slider<'a, Message>) -> Self {
        Element::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRAVEL: f32 = 200.0;

    #[test]
    fn a_normal_drag_follows_the_cursor_one_to_one() {
        let d = Drag { anchor_x: 100.0, anchor_value: 0.0, fine: false };
        // Half the track is half the range (-1..1 → 1.0).
        assert!((drag_value(d, 200.0, TRAVEL, -1.0, 1.0, 0.01) - 1.0).abs() < 1e-6);
        assert!((drag_value(d, 50.0, TRAVEL, -1.0, 1.0, 0.01) + 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_fine_drag_moves_ten_times_slower_and_resolves_below_the_step() {
        let d = Drag { anchor_x: 100.0, anchor_value: 0.0, fine: true };
        assert!((drag_value(d, 200.0, TRAVEL, -1.0, 1.0, 0.01) - 0.1).abs() < 1e-6);
        // Exposure's step is 0.1; fine control reaches 0.01 between them.
        let one_px = drag_value(Drag { anchor_x: 0.0, anchor_value: 0.5, fine: true }, 2.0, 200.0, -5.0, 5.0, 0.1);
        assert!((one_px - 0.51).abs() < 1e-5, "{one_px}");
    }

    #[test]
    fn drags_stop_at_the_ends() {
        let d = Drag { anchor_x: 0.0, anchor_value: 0.9, fine: false };
        assert_eq!(drag_value(d, 1000.0, TRAVEL, -1.0, 1.0, 0.01), 1.0);
        assert_eq!(drag_value(d, -1000.0, TRAVEL, -1.0, 1.0, 0.01), -1.0);
    }
}
