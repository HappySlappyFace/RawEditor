//! The AI denoise modal (`handlers::ai_denoise`): a 1:1 preview you can drag,
//! Amount, a hold-to-compare Before, and Apply for the whole image.

use iced::font::Font;
use iced::widget::{column, container, mouse_area, row, slider, stack, text, Space};
use iced::{Alignment, Border, Color, Element, Length};

use crate::app::handlers::ai_denoise::PREVIEW_SIZE;
use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::ui;
use crate::ui::widgets::button;
use crate::ui::theme;


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
                            .padding([3, 8])
                            .style(|_| container::Style {
                                background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.7).into()),
                                text_color: Some(theme::colours().on_photo),
                                ..Default::default()
                            }),
                    )
                    .padding(8),
                );
            }
            layers.into()
        }
        None => container(ui::text::tinted(status.clone().unwrap_or_default(), 12, theme::colours().text_muted))
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
        ui::text::tinted("Amount", 13, theme::colours().text_secondary).width(Length::Fixed(70.0)),
        slider(0.0..=100.0, st.amount * 100.0, |v: f32| Message::AiDenoiseAmount(v / 100.0)).step(1.0_f32),
        ui::text::tinted(format!("{:.0}%", st.amount * 100.0), 12, theme::colours().text_secondary).font(Font::MONOSPACE).width(Length::Fixed(44.0)),
    ]
    .spacing(theme::space::L)
    .align_y(Alignment::Center);

    // Hold to compare: press shows the original, release goes back.
    let before = mouse_area(
        container(text("Hold for Before").size(theme::size::SMALL))
            .padding([6, 12])
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
        Some(progress) => column![
            iced::widget::progress_bar(0.0..=1.0, progress.fraction()).height(6),
            row![
                ui::text::tinted(format!("Denoising the whole image… {:.0}%", progress.fraction() * 100.0), 12, theme::colours().text_muted),
                Space::with_width(Length::Fill),
                button(text("Stop").size(theme::size::BODY))
                    .on_press(Message::AiDenoiseCancel)
                    .padding([8, 14])
                    .style(ui::styles::NeutralButton::style),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(theme::space::L)
        .into(),
        None => {
            let mut r = row![].spacing(theme::space::L).align_y(Alignment::Center);
            if applied > 0.0 {
                r = r.push(
                    button(text("Remove").size(theme::size::BODY))
                        .on_press(Message::AiDenoiseRemove)
                        .padding([8, 14])
                        .style(ui::styles::NeutralButton::style),
                );
            }
            r.push(Space::with_width(Length::Fill))
                .push(
                    button(text("Cancel").size(theme::size::BODY))
                        .on_press(Message::AiDenoiseCancel)
                        .padding([8, 14])
                        .style(ui::styles::NeutralButton::style),
                )
                .push(
                    button(text("Apply").size(theme::size::BODY).align_x(iced::alignment::Horizontal::Center))
                        .on_press_maybe(ready.then_some(Message::AiDenoiseApply))
                        .padding([8, 14])
                        .width(Length::Fixed(110.0))
                        .style(ui::styles::AccentButton::style),
                )
                .into()
        }
    };

    column![
        ui::text::title("AI Denoise"),
        ui::text::tinted("Shown at 1:1 with every other edit. Drag the preview to look elsewhere.", 12, theme::colours().text_muted),
        row![picture].width(Length::Fill).align_y(Alignment::Center),
        amount_row,
        row![before, Space::with_width(Length::Fill), ui::text::tinted("PMRID · runs on this computer", 11, theme::colours().text_hint)]
            .align_y(Alignment::Center),
        iced::widget::horizontal_rule(1).style(ui::styles::rule),
        actions,
    ]
    .spacing(theme::space::XL)
    .width(Length::Fixed(PREVIEW_SIZE + 2.0))
    .into()
}
