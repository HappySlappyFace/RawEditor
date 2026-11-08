//! The AI denoise modal (`handlers::ai_denoise`): a 1:1 preview you can drag,
//! Amount, a hold-to-compare Before, and Apply for the whole image.

use iced::font::Font;
use iced::widget::{column, container, mouse_area, row, stack, text, Space};
use iced::{Alignment, Border, Color, Element, Length};

use crate::app::handlers::ai_denoise::PREVIEW_SIZE;
use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::ui;
use crate::ui::theme;
use crate::ui::dialog::Kind;


pub fn view(editor: &RawEditor) -> Element<'_, Message> {
    let st = &editor.ai_denoise;
    let applied = editor.current_edit_params.ai_denoise;

    let status: Option<String> = if let Some(e) = &st.error {
        Some(format!("Unavailable: {e}"))
    } else if st.preparing() {
        Some("Preparing…".into())
    } else if st.frame.is_none() || st.thinking() {
        Some("Denoising this area…".into())
    } else {
        None
    };
    let picture: Element<'_, Message> = match &st.frame {
        Some(f) => {
            let mut layers = stack![
                iced::widget::image(f.handle.clone())
                    .width(Length::Fixed(PREVIEW_SIZE))
                    .height(Length::Fixed(PREVIEW_SIZE)),
                iced::widget::canvas::Canvas::new(ui::loupe::LoupeDrag { on_pan: Message::AiDenoisePan })
                    .width(Length::Fixed(PREVIEW_SIZE))
                    .height(Length::Fixed(PREVIEW_SIZE)),
            ];
            let badge = if st.before { Some("BEFORE".to_string()) } else { status.clone() };
            if let Some(b) = badge {
                layers = layers.push(
                    container(
                        container(text(b).size(theme::size::CAPTION).font(Font::MONOSPACE))
                            .padding([theme::space::XS, theme::space::M])
                            .style(|_| container::Style {
                                background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.7).into()),
                                text_color: Some(theme::colours().on_photo),
                                ..Default::default()
                            }),
                    )
                    .padding(theme::space::M),
                );
            }
            layers.into()
        }
        None => container(ui::text::tinted(status.clone().unwrap_or_default(), theme::size::SMALL, theme::colours().text_muted))
            .center_x(Length::Fixed(PREVIEW_SIZE))
            .center_y(Length::Fixed(PREVIEW_SIZE))
            .into(),
    };
    let picture = container(picture).style(|_| container::Style {
        border: Border { color: theme::colours().border, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    });

    let ready = st.engine_ready();
    let amount_row = row![
        ui::text::tinted("Amount", theme::size::BODY, theme::colours().text_secondary).width(Length::Fixed(70.0)),
        ui::widgets::slider::slider(0.0..=100.0, st.amount * 100.0, |v: f32| Message::AiDenoiseAmount(v / 100.0))
            .step(1.0)
            .style(ui::styles::ProSlider::style),
        ui::text::tinted(format!("{:.0}%", st.amount * 100.0), theme::size::SMALL, theme::colours().text_secondary).font(Font::MONOSPACE).width(Length::Fixed(44.0)),
    ]
    .spacing(theme::space::L)
    .align_y(Alignment::Center);

    // Hold to compare: press shows the original, release goes back.
    let before = mouse_area(
        container(text("Hold for Before").size(theme::size::SMALL))
            .padding([theme::space::S, theme::space::XL])
            .style(move |_| container::Style {
                background: Some(theme::colours().control.into()),
                text_color: Some(theme::colours().text),
                border: Border { color: theme::colours().border_strong, width: 1.0, radius: 0.0.into() },
                ..Default::default()
            }),
    )
    .on_press(Message::AiDenoiseBefore(true))
    .on_release(Message::AiDenoiseBefore(false))
    .on_exit(Message::AiDenoiseBefore(false));

    let actions: Element<'_, Message> = match &st.job {
        Some(progress) => ui::dialog::progress(
            progress.fraction(),
            format!("Denoising the whole image… {:.0}%", progress.fraction() * 100.0),
            ui::dialog::inline_action("Stop").on_press(Message::AiDenoiseCancel),
        )
        .into(),
        None => {
            let mut buttons: Vec<Element<'_, Message>> = Vec::new();
            if applied > 0.0 {
                buttons.push(ui::dialog::action("Remove", Kind::Neutral).on_press(Message::AiDenoiseRemove).into());
            }
            buttons.push(ui::dialog::action("Cancel", Kind::Neutral).on_press(Message::AiDenoiseCancel).into());
            buttons.push(
                ui::dialog::action("Apply", Kind::Primary)
                    .on_press_maybe(ready.then_some(Message::AiDenoiseApply))
                    .into(),
            );
            ui::dialog::actions(buttons).into()
        }
    };

    let body = column![
        ui::text::tinted("Shown at 1:1 with every other edit. Drag the preview to look elsewhere.", theme::size::SMALL, theme::colours().text_muted),
        row![picture].width(Length::Fill).align_y(Alignment::Center),
        amount_row,
        row![before, Space::with_width(Length::Fill), ui::text::tinted("PMRID · runs on this computer", theme::size::CAPTION, theme::colours().text_hint)]
            .align_y(Alignment::Center),
    ]
    .spacing(theme::space::XL);

    ui::dialog::frame("AI Denoise", body, actions)
        .width(Length::Fixed(PREVIEW_SIZE + 2.0))
        .into()
}
