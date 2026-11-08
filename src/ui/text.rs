//! The text roles the UI repeats, defined once: before these existed each
//! view kept its own `hint`, `group_label`, `heading` or `modal_title`, at
//! slightly different sizes and greys.
//!
//! Each returns an `iced::widget::Text`, so callers can still chain
//! `.width(..)`, `.align_x(..)` and so on.

use iced::widget::text::{IntoFragment, Style};
use iced::widget::{text, Text};
use iced::Color;

use super::theme;

/// A dialog's title, and a page's (Library).
pub fn title<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    text(s).size(theme::size::TITLE).font(theme::font::BOLD).style(|_| Style { color: Some(theme::colours().text) })
}

/// A page heading inside a dialog (a Preferences page, a card).
pub fn heading<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    text(s).size(theme::size::HEADING).font(theme::font::BOLD).style(|_| Style { color: Some(theme::colours().text) })
}

/// Names a group of controls inside a section ("Noise reduction",
/// "Workflow").
pub fn group_label<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    text(s).size(theme::size::CAPTION).font(theme::font::BOLD).style(|_| Style { color: Some(theme::colours().text_muted) })
}

/// One or two lines explaining the control above it.
pub fn hint<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    text(s).size(theme::size::CAPTION).style(|_| Style { color: Some(theme::colours().text_hint) })
}

/// Running text in a dialog.
pub fn body<'a>(s: impl IntoFragment<'a>) -> Text<'a> {
    text(s).size(theme::size::BODY).style(|_| Style { color: Some(theme::colours().text_secondary) })
}

/// Text at a size in a given colour: for the cases the roles above don't
/// cover. Pass a token (`theme::colours().ok`), not a literal.
pub fn tinted<'a>(s: impl IntoFragment<'a>, size: u16, colour: Color) -> Text<'a> {
    text(s).size(size).style(move |_| Style { color: Some(colour) })
}

/// A centred message where content would be: "No image selected", "Preview
/// failed". The title is a message, so sentence case. `detail` is usually
/// `body(..)`, or `tinted(.., theme::colours().error)` for a failure.
pub fn empty_state<'a, Message: 'a>(title: impl IntoFragment<'a>, detail: Text<'a>) -> iced::widget::Column<'a, Message> {
    iced::widget::column![
        text(title).size(theme::size::TITLE).font(theme::font::BOLD).style(|_| Style { color: Some(theme::colours().text_secondary) }),
        detail,
    ]
    .spacing(theme::space::M)
    .align_x(iced::Alignment::Center)
}
