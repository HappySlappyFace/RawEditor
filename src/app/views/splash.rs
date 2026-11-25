//! The start-up splash: the photo fills the left of the window and dissolves
//! into the chrome colour on its right, where the brand sits (wordmark, a
//! status line and a thin progress line, the version at the foot). The pieces arrive one
//! after another over under a second, and the screen stays up for
//! `state::SPLASH_MIN` whatever the database does.
//!
//! The dissolve is a gradient laid over the photo rather than the photo's own
//! alpha, so it holds at any window shape: `Cover` crops the photo to the
//! window, which would cut an alpha fade off the edge. Everything sits on
//! `chrome`, the colour the logo's own background is drawn in, so the opaque
//! logo has no visible box in either theme.

use iced::gradient::Linear;
use iced::widget::{column, container, row, stack, Image, Space};
use iced::{Background, Color, ContentFit, Element, Gradient, Length, Radians};

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::ui::text as ui_text;
use crate::ui::theme;
use crate::ui::widgets::sweep_bar::sweep_bar;
use crate::ui::widgets::transition::fade_through;

use super::layout::{logo, splash};

/// The window's width is shared between the photo and the brand column, in
/// portions. The photo is cropped to its share (the subject sits left of the
/// middle), so its dissolve ends where the brand column begins.
const PHOTO_SHARE: u16 = 62;
const BRAND_SHARE: u16 = 100 - PHOTO_SHARE;
/// Within the photo's share: clear up to here, solid background from `SOLID`.
const SCRIM_CLEAR: f32 = 0.72;
const SCRIM_SOLID: f32 = 0.97;

pub fn view(editor: &RawEditor) -> Element<'_, Message> {
    let c = theme::colours();
    let bg = c.chrome;

    let photo = fade_through(
        1,
        bg,
        Image::new(splash())
            .width(Length::Fill)
            .height(Length::Fill)
            .content_fit(ContentFit::Cover),
    )
    .duration(0.7);

    let scrim = container(Space::new(Length::Fill, Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Gradient(Gradient::Linear(scrim_gradient(bg)))),
            ..Default::default()
        });

    let photo_panel = stack![photo, scrim]
        .width(Length::FillPortion(PHOTO_SHARE))
        .height(Length::Fill);

    let wordmark = fade_through(
        2,
        bg,
        column![
            Image::new(logo()).height(Length::Fixed(44.0)),
            ui_text::tinted("RAW photo editor", theme::size::SMALL, c.text_muted),
        ]
        .spacing(theme::space::L),
    )
    .delay(0.2)
    .duration(0.5);

    let progress = fade_through(
        3,
        bg,
        column![
            ui_text::tinted(&editor.session.status, theme::size::SMALL, c.text_muted),
            Space::with_height(theme::space::L),
            sweep_bar(2.0, c.border_subtle, c.accent),
        ]
        .width(Length::Fill),
    )
    .delay(0.4)
    .duration(0.4);

    let version = fade_through(
        4,
        bg,
        ui_text::tinted(
            format!("Version {}", crate::app::APP_VERSION),
            theme::size::CAPTION,
            c.text_disabled,
        ),
    )
    .delay(0.5)
    .duration(0.4);

    let brand = column![
        Space::with_height(Length::FillPortion(5)),
        wordmark,
        Space::with_height(Length::FillPortion(6)),
        progress,
        Space::with_height(theme::space::XXL),
        version,
    ]
    .padding([40, 44])
    .width(Length::Fill)
    .height(Length::Fill);

    stack![
        container(Space::new(Length::Fill, Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Background::Color(bg)),
                ..Default::default()
            }),
        row![
            photo_panel,
            container(brand).width(Length::FillPortion(BRAND_SHARE)).height(Length::Fill),
        ],
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Left to right across the photo's share: clear, then easing into solid
/// background. Alpha-only stops of one colour, so no grey creeps in mid-way.
fn scrim_gradient(bg: Color) -> Linear {
    let a = |alpha: f32| Color { a: alpha, ..bg };
    Linear::new(Radians(std::f32::consts::FRAC_PI_2))
        .add_stop(0.0, a(0.0))
        .add_stop(SCRIM_CLEAR, a(0.0))
        .add_stop(SCRIM_CLEAR + (SCRIM_SOLID - SCRIM_CLEAR) * 0.35, a(0.45))
        .add_stop(SCRIM_CLEAR + (SCRIM_SOLID - SCRIM_CLEAR) * 0.70, a(0.88))
        .add_stop(SCRIM_SOLID, a(1.0))
        .add_stop(1.0, a(1.0))
}
