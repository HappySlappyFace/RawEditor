//! The Export tab: what will be written, where, and how it's going.
//!
//! The batch machinery itself lives in `handlers::export` and predates this
//! view — it already queued, rendered and saved many images sequentially. This
//! tab makes that visible: the settings that used to be trapped in a modal,
//! the list of images the current selection would export, and live per-image
//! progress once a run starts.
//!
//! The Export modal is deliberately kept as the quick path from Library and
//! Develop; both drive the same `ExportConfirmed` handler.

use iced::font::{Font, Weight};
use iced::widget::{button, checkbox, column, container, radio, row, scrollable, slider, text, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::message::Message;
use crate::app::state::{ExportFormat, ExportJobState, RawEditor};
use crate::ui;
use crate::ui::icons::ICON_FONT;

pub fn view_export(editor: &RawEditor) -> Element<'_, Message> {
    let settings_panel = container(scrollable(view_settings(editor)))
        .width(Length::Fixed(320.0))
        .height(Length::Fill)
        .padding(14)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.09, 0.09, 0.09))),
            border: Border {
                width: 1.0,
                color: Color::from_rgb(0.16, 0.16, 0.16),
                radius: 0.0.into(),
            },
            ..Default::default()
        });

    row![view_queue(editor), settings_panel]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn section_header(label: &str) -> Element<'_, Message> {
    text(label)
        .size(12)
        .font(Font { weight: Weight::Bold, ..Default::default() })
        .style(|_| text::Style { color: Some(Color::from_rgb(0.5, 0.5, 0.5)) })
        .into()
}

fn body_text(s: String) -> Element<'static, Message> {
    text(s)
        .size(12)
        .style(|_: &Theme| text::Style { color: Some(Color::from_rgb(0.7, 0.7, 0.7)) })
        .into()
}

fn hint(s: String) -> Element<'static, Message> {
    text(s)
        .size(11)
        .style(|_: &Theme| text::Style { color: Some(Color::from_rgb(0.45, 0.45, 0.45)) })
        .into()
}

fn view_settings(editor: &RawEditor) -> Element<'_, Message> {
    let s = &editor.export_settings;

    let format_row = row![
        radio("JPEG", ExportFormat::Jpeg, Some(s.format), Message::SetExportFormat)
            .size(15)
            .text_size(12)
            .style(ui::styles::radio_style),
        radio("PNG", ExportFormat::Png, Some(s.format), Message::SetExportFormat)
            .size(15)
            .text_size(12)
            .style(ui::styles::radio_style),
        radio("TIFF", ExportFormat::Tiff, Some(s.format), Message::SetExportFormat)
            .size(15)
            .text_size(12)
            .style(ui::styles::radio_style),
    ]
    .spacing(12);

    let quality: Element<'_, Message> = if s.format == ExportFormat::Jpeg {
        column![
            row![
                body_text("Quality".to_string()),
                iced::widget::Space::with_width(Length::Fill),
                body_text(s.quality.to_string()),
            ],
            slider(10..=100, s.quality, Message::SetExportQuality)
                .style(crate::ui::styles::ProSlider::style),
        ]
        .spacing(4)
        .into()
    } else {
        column![].into()
    };

    let bit_depth_note: Element<'_, Message> = match s.format {
        ExportFormat::Tiff => hint("16-bit per channel — smoother gradation in shadows.".to_string()),
        ExportFormat::Png => hint("8-bit, lossless. Larger files than JPEG.".to_string()),
        ExportFormat::Jpeg => column![].into(),
    };

    let resize_field: Element<'_, Message> = if s.resize {
        row![
            body_text("Max long edge".to_string()),
            iced::widget::Space::with_width(Length::Fill),
            text_input("2048", &s.max_width.to_string())
                .on_input(|v| Message::SetExportWidth(v.parse().unwrap_or(0)))
                .width(Length::Fixed(90.0))
                .size(12)
                .style(ui::styles::text_input_style),
        ]
        .align_y(Alignment::Center)
        .into()
    } else {
        column![].into()
    };

    let full_path = s.base_path.join(&s.subfolder);

    column![
        section_header("Format"),
        format_row,
        quality,
        bit_depth_note,
        iced::widget::Space::with_height(Length::Fixed(8.0)),
        section_header("Size"),
        checkbox("Resize long edge", s.resize)
            .on_toggle(Message::ToggleExportResize)
            .size(15)
            .text_size(12)
            .style(ui::styles::checkbox_style),
        resize_field,
        hint("Only ever shrinks — a smaller image is never upscaled.".to_string()),
        iced::widget::Space::with_height(Length::Fixed(8.0)),
        section_header("Destination"),
        button(
            row![text(ui::icons::FOLDER_OPEN).font(ICON_FONT).size(12), text("Choose Folder").size(12)]
                .spacing(8)
                .align_y(Alignment::Center)
        )
        .on_press(Message::PickExportBasePath)
        .padding([8, 10])
        .width(Length::Fill)
        .style(ui::styles::NeutralButton::style),
        row![
            body_text("Subfolder".to_string()),
            iced::widget::Space::with_width(Length::Fill),
            text_input("Export", &s.subfolder)
                .on_input(Message::SetExportSubfolder)
                .width(Length::Fixed(130.0))
                .size(12)
                .style(ui::styles::text_input_style),
        ]
        .align_y(Alignment::Center),
        hint(full_path.to_string_lossy().to_string()),
        hint("Existing files are never overwritten — a numbered suffix is added.".to_string()),
    ]
    .spacing(8)
    .into()
}

fn view_queue(editor: &RawEditor) -> Element<'_, Message> {
    // Before a run, show what the current selection WOULD export. During and
    // after, show the actual batch with its progress.
    let showing_batch = !editor.export_jobs.is_empty();
    let pending_ids = crate::app::handlers::export::export_target_ids(editor);

    let header = if showing_batch {
        let done = editor
            .export_jobs
            .iter()
            .filter(|j| matches!(j.state, ExportJobState::Done(_)))
            .count();
        let failed = editor
            .export_jobs
            .iter()
            .filter(|j| matches!(j.state, ExportJobState::Failed(_)))
            .count();
        let total = editor.export_jobs.len();
        if editor.is_exporting {
            format!("Exporting — {done} of {total} done")
        } else if failed > 0 {
            format!("Finished — {done} exported, {failed} failed")
        } else {
            format!("Finished — {done} exported")
        }
    } else {
        match pending_ids.len() {
            0 => "Nothing selected".to_string(),
            1 => "1 image selected".to_string(),
            n => format!("{n} images selected"),
        }
    };

    let mut list = column![].spacing(2);
    if showing_batch {
        for job in &editor.export_jobs {
            let (mark, colour, detail) = match &job.state {
                ExportJobState::Pending => (
                    "·",
                    Color::from_rgb(0.45, 0.45, 0.45),
                    "waiting".to_string(),
                ),
                ExportJobState::Working => (
                    "»",
                    Color::from_rgb(0.98, 0.45, 0.09),
                    "rendering…".to_string(),
                ),
                ExportJobState::Done(path) => (
                    "✓",
                    Color::from_rgb(0.35, 0.75, 0.4),
                    path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                ),
                ExportJobState::Failed(err) => {
                    ("✕", Color::from_rgb(0.85, 0.35, 0.35), err.clone())
                }
            };
            list = list.push(
                container(
                    row![
                        text(mark).size(12).style(move |_: &Theme| text::Style {
                            color: Some(colour)
                        }),
                        text(job.filename.clone())
                            .size(12)
                            .width(Length::Fixed(220.0))
                            .style(|_: &Theme| text::Style {
                                color: Some(Color::from_rgb(0.8, 0.8, 0.8))
                            }),
                        text(detail).size(11).style(|_: &Theme| text::Style {
                            color: Some(Color::from_rgb(0.5, 0.5, 0.5))
                        }),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding([5, 8]),
            );
        }
    } else {
        for id in &pending_ids {
            if let Some(img) = editor.images.iter().find(|i| i.id == *id) {
                list = list.push(
                    container(
                        text(img.filename.clone())
                            .size(12)
                            .style(|_: &Theme| text::Style {
                                color: Some(Color::from_rgb(0.7, 0.7, 0.7))
                            }),
                    )
                    .padding([5, 8]),
                );
            }
        }
    }

    let empty_hint: Element<'_, Message> = if pending_ids.is_empty() && !showing_batch {
        column![
            iced::widget::Space::with_height(Length::Fixed(20.0)),
            hint("Select images in Library or Cull, then come back here.".to_string()),
            hint("Ctrl+A selects everything visible; Shift+Click selects a range.".to_string()),
        ]
        .spacing(6)
        .into()
    } else {
        column![].into()
    };

    // Start is disabled during a run — the handler also guards, but a live
    // button that silently does nothing is worse than a greyed-out one.
    let start: Element<'_, Message> = if editor.is_exporting {
        button(text("Cancel").align_x(iced::alignment::Horizontal::Center))
            .on_press(Message::CancelExport)
            .padding(10)
            .width(Length::Fixed(180.0))
            .style(ui::styles::DangerButton::style)
            .into()
    } else {
        button(
            text(match pending_ids.len() {
                1 => "Export 1 Image".to_string(),
                n => format!("Export {n} Images"),
            })
            .align_x(iced::alignment::Horizontal::Center),
        )
        .on_press_maybe((!pending_ids.is_empty()).then_some(Message::ExportConfirmed))
        .padding(10)
        .width(Length::Fixed(180.0))
        .style(ui::styles::AccentButton::style)
        .into()
    };

    container(
        column![
            row![
                text(header)
                    .size(15)
                    .font(Font { weight: Weight::Bold, ..Default::default() })
                    .style(|_: &Theme| text::Style {
                        color: Some(Color::from_rgb(0.85, 0.85, 0.85))
                    }),
                iced::widget::Space::with_width(Length::Fill),
                start,
            ]
            .align_y(Alignment::Center),
            iced::widget::horizontal_rule(1.0).style(|_| iced::widget::rule::Style {
                color: Color::from_rgb(0.22, 0.22, 0.22),
                width: 1,
                radius: 0.0.into(),
                fill_mode: iced::widget::rule::FillMode::Full,
            }),
            empty_hint,
            scrollable(list).height(Length::Fill),
        ]
        .spacing(10),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(16)
    .into()
}
