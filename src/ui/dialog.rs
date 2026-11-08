//! The parts every dialog repeats, defined once: the frame (title, rule,
//! body, actions), its two widths, and the buttons along its bottom edge.
//! Before this, each dialog picked its own width (380–460), gap above the
//! buttons, and button padding.

use iced::widget::text::IntoFragment;
use iced::widget::{column, row, text, Column, Row, Space};
use iced::{Element, Length};

use super::theme;
use super::widgets::button::{button, Button};

/// A dialog's width.
pub const WIDTH: f32 = 420.0;
/// For a dialog with three actions or long paths.
pub const WIDE: f32 = 460.0;

/// What an action does, which picks its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Cancel, Close, Not Now, and secondary choices.
    Neutral,
    /// The one action the dialog is for (Enter usually confirms it).
    Primary,
    /// Removes the user's work for good.
    Danger,
}

/// A dialog: title, rule, body, then its actions. `WIDTH` wide; chain
/// `.width(dialog::WIDE)` for a wider one.
pub fn frame<'a, Message: 'a>(
    title: impl IntoFragment<'a>,
    body: impl Into<Element<'a, Message>>,
    actions: impl Into<Element<'a, Message>>,
) -> Column<'a, Message> {
    column![
        super::text::title(title),
        iced::widget::horizontal_rule(1).style(super::styles::rule),
        body.into(),
        Space::with_height(theme::space::XS),
        actions.into(),
    ]
    .spacing(theme::space::XL)
    .width(WIDTH)
}

/// One of the buttons along a dialog's bottom edge. They share the row
/// equally, so a dialog's actions are always the same width.
pub fn action<'a, Message: Clone + 'a>(label: impl IntoFragment<'a>, kind: Kind) -> Button<'a, Message> {
    let b = button(
        text(label)
            .size(theme::size::BODY)
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center),
    )
    .padding(theme::space::L)
    .width(Length::Fill);
    match kind {
        Kind::Neutral => b.style(super::styles::NeutralButton::style),
        Kind::Primary => b.style(super::styles::AccentButton::style),
        Kind::Danger => b.style(super::styles::DangerButton::style),
    }
}

/// The row of actions, neutral ones first and the primary last.
pub fn actions<'a, Message: 'a>(buttons: impl IntoIterator<Item = Element<'a, Message>>) -> Row<'a, Message> {
    Row::with_children(buttons).spacing(theme::space::L)
}

/// A small button beside a progress bar ("Cancel", "Stop"): not one of the
/// dialog's actions, so it doesn't fill the row.
pub fn inline_action<'a, Message: Clone + 'a>(label: impl IntoFragment<'a>) -> Button<'a, Message> {
    button(text(label).size(theme::size::BODY))
        .padding([theme::space::S, theme::space::XL])
        .style(super::styles::NeutralButton::style)
}

/// A progress bar with a status line and an inline action under it: the
/// shape every long job in a dialog takes (model download, AI denoise).
pub fn progress<'a, Message: Clone + 'a>(
    fraction: f32,
    status: String,
    stop: Button<'a, Message>,
) -> Column<'a, Message> {
    column![
        iced::widget::progress_bar(0.0..=1.0, fraction).height(6),
        row![
            super::text::tinted(status, theme::size::SMALL, theme::colours().text_muted),
            Space::with_width(Length::Fill),
            stop,
        ]
        .align_y(iced::Alignment::Center),
    ]
    .spacing(theme::space::L)
}
