//! Publishes a message on a double click anywhere over its content — the
//! Develop sliders' reset (`views::develop::slider_field`).
//!
//! `mouse_area` can't do it: iced 0.13's has no double click, and a slider
//! captures the press before any wrapper outside it would see the event.
//! This one looks first. A single press goes to the content untouched (the
//! slider jumps and drags as always); the second press of a double click is
//! swallowed and becomes the message, so the reset isn't followed by a jump
//! to wherever the pointer happened to be.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{tree, Operation, Tree, Widget};
use iced::advanced::{overlay, Clipboard, Shell};
use iced::event::{self, Event};
use iced::mouse;
use iced::{Element, Length, Point, Rectangle, Size, Theme, Vector};
use std::time::{Duration, Instant};

type Renderer = iced::Renderer;

/// Two presses closer than this in time and space are a double click.
/// iced's own `mouse::Click` wants the identical position, which a trackpad
/// rarely delivers twice.
const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(350);
const DOUBLE_CLICK_SLOP: f32 = 4.0;

pub struct DoubleClick<'a, Message> {
    content: Element<'a, Message>,
    on_double_click: Message,
}

pub fn double_click<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    on_double_click: Message,
) -> DoubleClick<'a, Message> {
    DoubleClick { content: content.into(), on_double_click }
}

#[derive(Default)]
struct State {
    last_press: Option<(Point, Instant)>,
}

impl<'a, Message: Clone> Widget<Message, Theme, Renderer> for DoubleClick<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(&self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.content.as_widget().layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if let Some(position) = cursor.position_over(layout.bounds()) {
                let state = tree.state.downcast_mut::<State>();
                let now = Instant::now();
                let double = state.last_press.is_some_and(|(at, when)| {
                    now.duration_since(when) <= DOUBLE_CLICK_TIME && at.distance(position) <= DOUBLE_CLICK_SLOP
                });
                if double {
                    // Start over: a third press is a fresh single click.
                    state.last_press = None;
                    shell.publish(self.on_double_click.clone());
                    return event::Status::Captured;
                }
                state.last_press = Some((position, now));
            }
        }
        self.content.as_widget_mut().on_event(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, translation)
    }
}

impl<'a, Message: Clone + 'a> From<DoubleClick<'a, Message>> for Element<'a, Message> {
    fn from(w: DoubleClick<'a, Message>) -> Self {
        Self::new(w)
    }
}
