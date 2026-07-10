use iced::font::{Font, Weight};
use iced::widget::{
    button, checkbox, column, container, mouse_area, radio, row, slider, stack, text, text_input,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::ui;

pub fn modal_overlay<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    // The backdrop: semi-transparent black, fills screen, closes on click
    let backdrop = mouse_area(
        container(text(" "))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
                ..Default::default()
            }),
    )
    .on_press(Message::CloseModal);

    // The card: centered content
    let card = container(content).padding(20).style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.10, 0.10, 0.10))),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: Color::from_rgb(0.25, 0.25, 0.25),
        },
        ..Default::default()
    });

    stack![
        backdrop,
        container(
            // Swallow clicks on the card so they don't reach the backdrop
            mouse_area(card).on_press(Message::ModalNoOp)
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
    ]
    .into()
}

pub fn view_help_modal<'a>() -> Element<'a, Message> {
    let shortcut = |key: &str, desc: &str| {
        row![
            text(key.to_string())
                .font(Font {
                    weight: Weight::Bold,
                    ..Default::default()
                })
                .width(Length::Fixed(140.0))
                .size(13)
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.7, 0.7, 0.7))
                }),
            text(desc.to_string()).size(13).style(|_| text::Style {
                color: Some(Color::from_rgb(0.5, 0.5, 0.5))
            })
        ]
        .spacing(8)
    };

    column![
        text("Keyboard Shortcuts")
            .size(18)
            .font(Font {
                weight: Weight::Bold,
                ..Default::default()
            })
            .style(|_| text::Style {
                color: Some(Color::from_rgb(0.85, 0.85, 0.85))
            }),
        iced::widget::horizontal_rule(1.0).style(|_| iced::widget::rule::Style {
            color: Color::from_rgb(0.3, 0.3, 0.3),
            width: 1,
            radius: 0.0.into(),
            fill_mode: iced::widget::rule::FillMode::Full
        }),
        column![
            text("Navigation")
                .size(12)
                .font(Font {
                    weight: Weight::Bold,
                    ..Default::default()
                })
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.5, 0.5, 0.5))
                }),
            shortcut("Arrow Keys", "Previous / Next Image"),
            shortcut("Space", "Toggle Before/After"),
            text("Rating & Culling")
                .size(12)
                .font(Font {
                    weight: Weight::Bold,
                    ..Default::default()
                })
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.5, 0.5, 0.5))
                }),
            shortcut("0 - 5", "Star Rating"),
            shortcut("P", "Pick"),
            shortcut("X", "Reject"),
            shortcut("U", "Unflag"),
            text("Editing")
                .size(12)
                .font(Font {
                    weight: Weight::Bold,
                    ..Default::default()
                })
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.5, 0.5, 0.5))
                }),
            shortcut("R", "Reset Edits"),
            shortcut("Ctrl + Z", "Undo"),
            shortcut("Ctrl + Shift + Z", "Redo"),
            shortcut("Ctrl + C / V", "Copy / Paste Settings"),
            shortcut("Double Click", "Reset Zoom"),
        ]
        .spacing(6),
        button("Close")
            .on_press(Message::CloseModal)
            .padding(8)
            .width(Length::Fill)
            .style(ui::styles::NeutralButton::style)
    ]
    .spacing(12)
    .width(420)
    .into()
}

pub fn view_preferences_modal<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    column![
        text("Preferences")
            .size(18)
            .font(Font {
                weight: Weight::Bold,
                ..Default::default()
            })
            .style(|_| text::Style {
                color: Some(Color::from_rgb(0.85, 0.85, 0.85))
            }),
        iced::widget::horizontal_rule(1.0).style(|_| iced::widget::rule::Style {
            color: Color::from_rgb(0.3, 0.3, 0.3),
            width: 1,
            radius: 0.0.into(),
            fill_mode: iced::widget::rule::FillMode::Full
        }),
        column![
            text("Workflow")
                .size(12)
                .font(Font {
                    weight: Weight::Bold,
                    ..Default::default()
                })
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.5, 0.5, 0.5))
                }),
            checkbox("Auto-Advance on Rate/Flag", editor.auto_advance)
                .on_toggle(|_| Message::ToggleAutoAdvance)
                .size(16)
                .spacing(8)
                .text_size(13)
                .style(|_theme, _status| checkbox::Style {
                    background: Background::Color(Color::from_rgb(0.2, 0.2, 0.2)),
                    icon_color: Color::from_rgb(0.85, 0.85, 0.85),
                    border: Border {
                        color: Color::from_rgb(0.4, 0.4, 0.4),
                        width: 1.0,
                        radius: 3.0.into(),
                    },
                    text_color: Some(Color::from_rgb(0.7, 0.7, 0.7)),
                }),
        ]
        .spacing(8),
        column![
            {
                let used_mb = editor.raw_cache_bytes as f32 / (1024.0 * 1024.0);
                let budget_mb = editor.raw_preload_budget_mb as f32;
                text(format!(
                    "RAW Cache Usage: {:.0} / {:.0} MB",
                    used_mb,
                    budget_mb
                ))
                .size(12)
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.55, 0.75, 0.55))
                })
            },
            text("Performance")
                .size(12)
                .font(Font {
                    weight: Weight::Bold,
                    ..Default::default()
                })
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.5, 0.5, 0.5))
                }),
            row![
                text("Memory Cache Size:").size(13).style(|_| text::Style {
                    color: Some(Color::from_rgb(0.7, 0.7, 0.7))
                }),
                text(format!("{} Images", editor.cache_capacity))
                    .size(13)
                    .font(Font {
                        weight: Weight::Bold,
                        ..Default::default()
                    })
                    .style(|_| text::Style {
                        color: Some(Color::from_rgb(0.85, 0.85, 0.85))
                    }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            slider(
                20.0..=500.0,
                editor.cache_capacity as f32,
                Message::SetCacheCapacity
            )
            .step(10.0),
            text("Controls how many high-res previews are kept in RAM.")
                .size(11)
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.45, 0.45, 0.45))
                }),
            row![
                text("RAW Preload Budget:").size(13).style(|_| text::Style {
                    color: Some(Color::from_rgb(0.7, 0.7, 0.7))
                }),
                text(format!("{} MB", editor.raw_preload_budget_mb))
                    .size(13)
                    .font(Font {
                        weight: Weight::Bold,
                        ..Default::default()
                    })
                    .style(|_| text::Style {
                        color: Some(Color::from_rgb(0.85, 0.85, 0.85))
                    }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            slider(
                0.0..=4096.0,
                editor.raw_preload_budget_mb as f32,
                Message::SetRawPreloadBudget
            )
            .step(64.0),
            text("Develop RAW preloading budget. Set to 0 to disable RAW preloading.")
                .size(11)
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.45, 0.45, 0.45))
                }),
            row![
                text("Preview Preload (Behind/Ahead):")
                    .size(13)
                    .style(|_| text::Style {
                        color: Some(Color::from_rgb(0.7, 0.7, 0.7))
                    }),
                text(format!(
                    "{}/{}",
                    editor.preview_preload_behind, editor.preview_preload_ahead
                ))
                .size(13)
                .font(Font {
                    weight: Weight::Bold,
                    ..Default::default()
                })
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.85, 0.85, 0.85))
                }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            slider(
                0.0..=crate::app::state::PREVIEW_PRELOAD_BEHIND_MAX as f32,
                editor.preview_preload_behind as f32,
                Message::SetPreviewPreloadBehind
            )
            .step(1.0),
            slider(
                0.0..=crate::app::state::PREVIEW_PRELOAD_AHEAD_MAX as f32,
                editor.preview_preload_ahead as f32,
                Message::SetPreviewPreloadAhead
            )
            .step(1.0),
            text("How many working previews stay queued around your current image.")
                .size(11)
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.45, 0.45, 0.45))
                }),
            row![
                text("RAW Preload (Behind/Ahead):")
                    .size(13)
                    .style(|_| text::Style {
                        color: Some(Color::from_rgb(0.7, 0.7, 0.7))
                    }),
                text(format!(
                    "{}/{}",
                    editor.raw_preload_behind, editor.raw_preload_ahead
                ))
                .size(13)
                .font(Font {
                    weight: Weight::Bold,
                    ..Default::default()
                })
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.85, 0.85, 0.85))
                }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            slider(
                0.0..=crate::app::state::RAW_PRELOAD_BEHIND_MAX as f32,
                editor.raw_preload_behind as f32,
                Message::SetRawPreloadBehind
            )
            .step(1.0),
            slider(
                0.0..=crate::app::state::RAW_PRELOAD_AHEAD_MAX as f32,
                editor.raw_preload_ahead as f32,
                Message::SetRawPreloadAhead
            )
            .step(1.0),
            text("How many full RAWs are pre-decoded for faster Develop navigation.")
                .size(11)
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.45, 0.45, 0.45))
                }),
        ]
        .spacing(8),
        button("Reset Preferences to Defaults")
            .on_press(Message::ResetPreferences)
            .padding(8)
            .width(Length::Fill)
            .style(ui::styles::NeutralButton::style),
        button("Close")
            .on_press(Message::CloseModal)
            .padding(8)
            .width(Length::Fill)
            .style(ui::styles::NeutralButton::style)
    ]
    .spacing(12)
    .width(420)
    .into()
}

pub fn view_export_modal<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    let settings = &editor.export_settings;

    let format_section = column![
        text("File Format")
            .size(12)
            .font(Font {
                weight: Weight::Bold,
                ..Default::default()
            })
            .style(|_theme: &Theme| text::Style {
                color: Some(Color::from_rgb(0.5, 0.5, 0.5))
            }),
        row![
            radio(
                "JPEG",
                crate::app::state::ExportFormat::Jpeg,
                Some(settings.format),
                Message::SetExportFormat
            )
            .size(14)
            .spacing(5)
            .text_size(13)
            .style(ui::styles::radio_style),
            radio(
                "PNG",
                crate::app::state::ExportFormat::Png,
                Some(settings.format),
                Message::SetExportFormat
            )
            .size(14)
            .spacing(5)
            .text_size(13)
            .style(ui::styles::radio_style),
        ]
        .spacing(20),
        if settings.format == crate::app::state::ExportFormat::Jpeg {
            Element::from(
                column![
                    row![
                        text("Quality:")
                            .size(13)
                            .style(|_theme: &Theme| text::Style {
                                color: Some(Color::from_rgb(0.7, 0.7, 0.7))
                            }),
                        text(format!("{}", settings.quality))
                            .size(13)
                            .font(Font {
                                weight: Weight::Bold,
                                ..Default::default()
                            })
                            .style(|_theme: &Theme| text::Style {
                                color: Some(Color::from_rgb(0.85, 0.85, 0.85))
                            }),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    slider(10..=100, settings.quality, Message::SetExportQuality).step(1),
                ]
                .spacing(5),
            )
        } else {
            column![].into()
        }
    ]
    .spacing(10);

    let dimensions_section = column![
        text("Dimensions")
            .size(12)
            .font(Font {
                weight: Weight::Bold,
                ..Default::default()
            })
            .style(|_theme: &Theme| text::Style {
                color: Some(Color::from_rgb(0.5, 0.5, 0.5))
            }),
        checkbox("Resize Long Edge", settings.resize)
            .on_toggle(Message::ToggleExportResize)
            .size(16)
            .text_size(13)
            .style(ui::styles::checkbox_style),
        if settings.resize {
            Element::from(
                row![
                    text("Max Width (px):")
                        .size(13)
                        .style(|_theme: &Theme| text::Style {
                            color: Some(Color::from_rgb(0.7, 0.7, 0.7))
                        }),
                    text_input("2048", &settings.max_width.to_string())
                        .on_input(|s| s
                            .parse()
                            .ok()
                            .map(Message::SetExportWidth)
                            .unwrap_or(Message::SetExportWidth(settings.max_width)))
                        .width(Length::Fixed(80.0))
                        .padding(5)
                        .style(ui::styles::text_input_style)
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
        } else {
            row![].into()
        }
    ]
    .spacing(10);

    let destination_section = column![
        text("Destination")
            .size(12)
            .font(Font {
                weight: Weight::Bold,
                ..Default::default()
            })
            .style(|_theme: &Theme| text::Style {
                color: Some(Color::from_rgb(0.5, 0.5, 0.5))
            }),
        row![
            text("Base Folder:")
                .size(13)
                .style(|_theme: &Theme| text::Style {
                    color: Some(Color::from_rgb(0.7, 0.7, 0.7))
                }),
            button(
                row![
                    text(
                        settings
                            .base_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                    )
                    .size(13),
                    text("...").size(13)
                ]
                .spacing(5)
                .align_y(Alignment::Center)
            )
            .on_press(Message::PickExportBasePath)
            .padding(5)
            .style(ui::styles::button_style),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        row![
            text("Subfolder:")
                .size(13)
                .style(|_theme: &Theme| text::Style {
                    color: Some(Color::from_rgb(0.7, 0.7, 0.7))
                }),
            text_input("Export", &settings.subfolder)
                .on_input(Message::SetExportSubfolder)
                .padding(5)
                .style(ui::styles::text_input_style)
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        text(format!(
            "Full Path: {}/{}",
            settings.base_path.display(),
            settings.subfolder
        ))
        .size(11)
        .style(|_theme: &Theme| text::Style {
            color: Some(Color::from_rgb(0.5, 0.5, 0.5))
        })
    ]
    .spacing(10);

    let count = if editor.multi_selection.is_empty() {
        1
    } else {
        editor.multi_selection.len()
    };
    let export_btn = button(
        text(format!("Export {} Images", count)).align_x(iced::alignment::Horizontal::Center),
    )
    .on_press(Message::ExportConfirmed)
    .width(Length::Fill)
    .padding(10)
    .style(|theme, status| button::Style {
        background: Some(Background::Color(Color::from_rgb(0.2, 0.6, 0.3))),
        text_color: Color::WHITE,
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..button::primary(theme, status)
    });

    column![
        text("Export Studio")
            .size(18)
            .font(Font {
                weight: Weight::Bold,
                ..Default::default()
            })
            .style(|_| text::Style {
                color: Some(Color::from_rgb(0.85, 0.85, 0.85))
            }),
        iced::widget::horizontal_rule(1.0).style(|_| iced::widget::rule::Style {
            color: Color::from_rgb(0.3, 0.3, 0.3),
            width: 1,
            radius: 0.0.into(),
            fill_mode: iced::widget::rule::FillMode::Full
        }),
        format_section,
        dimensions_section,
        destination_section,
        iced::widget::vertical_space().height(10),
        row![
            button("Cancel")
                .on_press(Message::CloseModal)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::NeutralButton::style),
            export_btn
        ]
        .spacing(10)
    ]
    .spacing(15)
    .width(400)
    .into()
}

pub fn view_delete_modal<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    let count = editor.pending_delete_ids.len();
    let body = if count == 1 {
        let name = editor
            .images
            .iter()
            .find(|i| i.id == editor.pending_delete_ids[0])
            .map(|i| i.filename.clone())
            .unwrap_or_default();
        format!("\"{}\" will be removed.", name)
    } else {
        format!("{} images will be removed.", count)
    };

    column![
        text("Delete Image").size(18).font(Font {
            weight: Weight::Bold,
            ..Default::default()
        }).style(|_| text::Style {
            color: Some(Color::from_rgb(0.85, 0.85, 0.85))
        }),
        iced::widget::horizontal_rule(1.0).style(|_| iced::widget::rule::Style {
            color: Color::from_rgb(0.3, 0.3, 0.3),
            width: 1,
            radius: 0.0.into(),
            fill_mode: iced::widget::rule::FillMode::Full
        }),
        text(body).size(13).style(|_theme: &Theme| text::Style {
            color: Some(Color::from_rgb(0.75, 0.75, 0.75))
        }),
        text("\"Mark\" hides it from Develop but keeps it (and the file) untouched. \"Delete from Disk\" moves the RAW and cache files to the system trash and removes it from the library. Enter confirms Delete from Disk.")
            .size(11)
            .style(|_theme: &Theme| text::Style {
                color: Some(Color::from_rgb(0.5, 0.5, 0.5))
            }),
        iced::widget::vertical_space().height(10),
        row![
            button(text("Cancel").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::CloseModal)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::NeutralButton::style),
            button(text("Mark for Removal").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::MarkForRemovalConfirmed)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::AccentButton::style),
            button(text("Delete from Disk").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::DeleteFromDiskConfirmed)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::DangerButton::style),
        ]
        .spacing(10)
    ]
    .spacing(15)
    .width(440)
    .into()
}
