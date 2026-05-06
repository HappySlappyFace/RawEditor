use iced::widget::image::Handle;
use iced::widget::{button, checkbox, column, container, row, scrollable, slider, stack, text, Image, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use iced_aw::Wrap;
use std::path::PathBuf;

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::database::models::Image as ImageData;
use crate::ui;
use crate::ui::icons::ICON_FONT;

fn metric_pill<'a>(label: &'a str, value: impl ToString, color: Color) -> Element<'a, Message> {
    container(
        row![
            text(label).size(11).style(|_| text::Style {
                color: Some(Color::from_rgb(0.68, 0.68, 0.68))
            }),
            text(value.to_string()).size(11).style(move |_| text::Style { color: Some(color) }),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([4, 8])
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.13, 0.13, 0.13))),
        border: Border {
            radius: 12.0.into(),
            width: 1.0,
            color: Color::from_rgb(0.20, 0.20, 0.20),
        },
        ..Default::default()
    })
    .into()
}

fn style_chip(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        if active {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.24, 0.44, 0.66))),
                text_color: Color::WHITE,
                border: Border {
                    radius: 14.0.into(),
                    width: 1.0,
                    color: Color::from_rgb(0.45, 0.65, 0.85),
                },
                ..Default::default()
            }
        } else {
            match status {
                button::Status::Hovered => button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.22, 0.22, 0.22))),
                    text_color: Color::from_rgb(0.90, 0.90, 0.90),
                    border: Border {
                        radius: 14.0.into(),
                        width: 1.0,
                        color: Color::from_rgb(0.30, 0.30, 0.30),
                    },
                    ..Default::default()
                },
                _ => button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.16, 0.16, 0.16))),
                    text_color: Color::from_rgb(0.72, 0.72, 0.72),
                    border: Border {
                        radius: 14.0.into(),
                        width: 1.0,
                        color: Color::from_rgb(0.24, 0.24, 0.24),
                    },
                    ..Default::default()
                },
            }
        }
    }
}

fn style_toolbar_button(_theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered => button::Style {
            background: Some(Background::Color(Color::from_rgb(0.30, 0.30, 0.30))),
            text_color: Color::WHITE,
            border: Border {
                radius: 8.0.into(),
                width: 1.0,
                color: Color::from_rgb(0.45, 0.45, 0.45),
            },
            ..Default::default()
        },
        button::Status::Pressed => button::Style {
            background: Some(Background::Color(Color::from_rgb(0.17, 0.17, 0.17))),
            text_color: Color::WHITE,
            border: Border {
                radius: 8.0.into(),
                width: 1.0,
                color: Color::from_rgb(0.35, 0.35, 0.35),
            },
            ..Default::default()
        },
        _ => button::Style {
            background: Some(Background::Color(Color::from_rgb(0.20, 0.20, 0.20))),
            text_color: Color::from_rgb(0.88, 0.88, 0.88),
            border: Border {
                radius: 8.0.into(),
                width: 1.0,
                color: Color::from_rgb(0.30, 0.30, 0.30),
            },
            ..Default::default()
        },
    }
}

/// Build the Library tab view (grid of thumbnails)
fn view_image_card<'a>(img: &'a ImageData, is_selected: bool, size: f32) -> Element<'a, Message> {
    let thumb_width = size;
    let thumb_height = size * 0.75;
    let is_deleted = img.file_status == "deleted";
    let thumb_missing = img.cache_path_thumb.is_none();

    let image_content: Element<'a, Message> = if let Some(ref thumb_path) = img.cache_path_thumb {
        Image::new(Handle::from_path(PathBuf::from(thumb_path)))
            .content_fit(iced::ContentFit::Cover)
            .width(Length::Fixed(thumb_width))
            .height(Length::Fixed(thumb_height))
            .into()
    } else {
        container(
            column![
                text(ui::icons::TH_LARGE).font(ICON_FONT).size(24),
                text("No preview").size(11).style(|_| text::Style {
                    color: Some(Color::from_rgb(0.65, 0.65, 0.65))
                })
            ]
            .spacing(6)
            .align_x(Alignment::Center),
        )
        .width(Length::Fixed(thumb_width))
        .height(Length::Fixed(thumb_height))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.12, 0.12, 0.12))),
            ..Default::default()
        })
        .into()
    };

    let top_left_badge: Element<'a, Message> = if is_deleted {
        container(
            row![
                text(ui::icons::TIMES).font(ICON_FONT).size(11),
                text("Missing").size(11)
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([3, 7])
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.85, 0.18, 0.18, 0.95))),
            border: Border {
                radius: 10.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    } else if img.flag == 1 {
        container(
            row![
                text(ui::icons::CHECK).font(ICON_FONT).size(11),
                text("Pick").size(11)
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([3, 7])
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.15, 0.62, 0.25, 0.95))),
            border: Border {
                radius: 10.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    } else if img.flag == -1 {
        container(
            row![
                text(ui::icons::TIMES).font(ICON_FONT).size(11),
                text("Reject").size(11)
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([3, 7])
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.85, 0.20, 0.20, 0.95))),
            border: Border {
                radius: 10.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    } else {
        container(Space::with_width(0)).into()
    };

    let top_right_badge: Element<'a, Message> = if img.rating > 0 {
        let stars = vec![ui::icons::STAR; img.rating as usize].join(" ");
        container(text(stars).font(ICON_FONT).size(10))
            .padding([3, 7])
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.65))),
                border: Border {
                    radius: 10.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    } else {
        container(Space::with_width(0)).into()
    };

    let filename_overlay = container(
        row![
            text(&img.filename).size(12).style(|_| text::Style {
                color: Some(Color::from_rgb(0.94, 0.94, 0.94))
            }),
            Space::with_width(Length::Fill),
            text(if thumb_missing { "Queued" } else { "Ready" })
                .size(10)
                .style(move |_| text::Style {
                    color: Some(if thumb_missing {
                        Color::from_rgb(0.88, 0.71, 0.31)
                    } else {
                        Color::from_rgb(0.52, 0.78, 0.60)
                    })
                })
        ]
        .align_y(Alignment::Center),
    )
    .padding([4, 8])
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgba(0.02, 0.02, 0.02, 0.82))),
        ..Default::default()
    });

    let image_stack = stack![
        container(image_content)
            .width(Length::Fixed(thumb_width))
            .height(Length::Fixed(thumb_height))
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.10, 0.10, 0.10))),
                ..Default::default()
            }),
        container(top_left_badge)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Top)
            .padding(8),
        container(top_right_badge)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top)
            .padding(8),
        container(filename_overlay)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Bottom),
    ];

    let card = container(image_stack)
        .width(Length::Fixed(thumb_width + 14.0))
        .height(Length::Fixed(thumb_height + 14.0))
        .padding(7)
        .style(move |_| {
            if is_selected {
                container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.18, 0.27, 0.36))),
                    border: Border {
                        radius: 10.0.into(),
                        width: 1.0,
                        color: Color::from_rgb(0.39, 0.61, 0.82),
                    },
                    ..Default::default()
                }
            } else {
                container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.09, 0.09, 0.09))),
                    border: Border {
                        radius: 10.0.into(),
                        width: 1.0,
                        color: Color::from_rgb(0.17, 0.17, 0.17),
                    },
                    ..Default::default()
                }
            }
        });

    button(card)
        .on_press(Message::ImageSelected(img.id))
        .padding(0)
        .style(|_theme, _status| button::Style {
            background: None,
            border: Border::default(),
            ..Default::default()
        })
        .into()
}

pub fn view_library(editor: &RawEditor) -> Element<'_, Message> {
    let filtered_images: Vec<&ImageData> = editor
        .images
        .iter()
        .filter(|img| editor.min_filter_rating == 0 || img.rating >= editor.min_filter_rating)
        .collect();

    let cached_count = filtered_images
        .iter()
        .filter(|img| img.cache_path_thumb.is_some())
        .count();
    let deleted_count = filtered_images
        .iter()
        .filter(|img| img.file_status == "deleted")
        .count();
    let total_count = editor.images.len();
    let filtered_count = filtered_images.len();
    let selected_count = editor.multi_selection.len();

    let content: Element<'_, Message> = if filtered_images.is_empty() {
        container(
            column![
                text("No images match this filter").size(24),
                text("Try lowering the rating filter or import a new folder.")
                    .size(14)
                    .style(|_| text::Style {
                        color: Some(Color::from_rgb(0.55, 0.55, 0.55))
                    }),
                button(
                    row![
                        text(ui::icons::FOLDER_OPEN).font(ICON_FONT),
                        text("Import Folder")
                    ]
                    .spacing(8)
                )
                .on_press(Message::ImportFolder)
                .style(style_toolbar_button)
                .padding([10, 14]),
            ]
            .spacing(16)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    } else {
        let thumbnail_grid = filtered_images.iter().fold(
            Wrap::new().spacing(12.0).line_spacing(12.0),
            |wrap, img| {
                let is_selected =
                    editor.selected_image_id == Some(img.id) || editor.multi_selection.contains(&img.id);
                wrap.push(view_image_card(img, is_selected, editor.thumbnail_size))
            },
        );

        container(
            scrollable(container(thumbnail_grid).width(Length::Fill))
                .height(Length::Fill)
                .width(Length::Fill),
        )
        .padding([10, 12])
        .height(Length::Fill)
        .into()
    };

    column![
        view_library_toolbar(editor, filtered_count, total_count, selected_count, deleted_count),
        content,
        view_status_bar(
            editor,
            filtered_count,
            total_count,
            selected_count,
            cached_count,
            deleted_count
        )
    ]
    .into()
}

fn view_filter_bar<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    row![
        button(text("All").size(12))
            .on_press(Message::SetMinRating(0))
            .padding([5, 10])
            .style(style_chip(editor.min_filter_rating == 0)),
        button(row![text(ui::icons::STAR).font(ICON_FONT).size(11), text("1+")].spacing(4))
            .on_press(Message::SetMinRating(1))
            .padding([5, 10])
            .style(style_chip(editor.min_filter_rating == 1)),
        button(
            row![
                text(format!("{} {}", ui::icons::STAR, ui::icons::STAR))
                    .font(ICON_FONT)
                    .size(11),
                text("2+")
            ]
            .spacing(4)
        )
        .on_press(Message::SetMinRating(2))
        .padding([5, 10])
        .style(style_chip(editor.min_filter_rating == 2)),
        button(
            row![
                text(format!("{} {} {}", ui::icons::STAR, ui::icons::STAR, ui::icons::STAR))
                    .font(ICON_FONT)
                    .size(11),
                text("3+")
            ]
            .spacing(4)
        )
        .on_press(Message::SetMinRating(3))
        .padding([5, 10])
        .style(style_chip(editor.min_filter_rating == 3)),
        button(
            row![
                text(
                    format!(
                        "{} {} {} {}",
                        ui::icons::STAR,
                        ui::icons::STAR,
                        ui::icons::STAR,
                        ui::icons::STAR
                    )
                )
                .font(ICON_FONT)
                .size(11),
                text("4+")
            ]
            .spacing(4)
        )
        .on_press(Message::SetMinRating(4))
        .padding([5, 10])
        .style(style_chip(editor.min_filter_rating == 4)),
        button(
            row![
                text(
                    format!(
                        "{} {} {} {} {}",
                        ui::icons::STAR,
                        ui::icons::STAR,
                        ui::icons::STAR,
                        ui::icons::STAR,
                        ui::icons::STAR
                    )
                )
                .font(ICON_FONT)
                .size(11),
                text("5")
            ]
            .spacing(4)
        )
        .on_press(Message::SetMinRating(5))
        .padding([5, 10])
        .style(style_chip(editor.min_filter_rating == 5)),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn view_quick_actions<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    let has_selection = editor.selected_image_id.is_some() || !editor.multi_selection.is_empty();
    row![
        text("Quick actions").size(12).style(|_| text::Style {
            color: Some(Color::from_rgb(0.62, 0.62, 0.62))
        }),
        button(text("☆ 0").size(11))
            .on_press_maybe(has_selection.then_some(Message::SetRating(0)))
            .padding([4, 8])
            .style(ui::styles::NeutralButton::style),
        button(text("★ 3").size(11))
            .on_press_maybe(has_selection.then_some(Message::SetRating(3)))
            .padding([4, 8])
            .style(ui::styles::NeutralButton::style),
        button(text("★ 5").size(11))
            .on_press_maybe(has_selection.then_some(Message::SetRating(5)))
            .padding([4, 8])
            .style(ui::styles::NeutralButton::style),
        button(row![text(ui::icons::CHECK).font(ICON_FONT).size(11), text("Pick").size(11)].spacing(4))
            .on_press_maybe(has_selection.then_some(Message::SetFlag(1)))
            .padding([4, 8])
            .style(ui::styles::NeutralButton::style),
        button(row![text(ui::icons::TIMES).font(ICON_FONT).size(11), text("Reject").size(11)].spacing(4))
            .on_press_maybe(has_selection.then_some(Message::SetFlag(-1)))
            .padding([4, 8])
            .style(ui::styles::NeutralButton::style),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn view_library_toolbar<'a>(
    editor: &'a RawEditor,
    filtered_count: usize,
    total_count: usize,
    selected_count: usize,
    deleted_count: usize,
) -> Element<'a, Message> {
    let import_btn = button(
        row![text(ui::icons::FOLDER_OPEN).font(ICON_FONT), text("Import")]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .on_press(Message::ImportFolder)
    .padding([8, 12])
    .style(style_toolbar_button);

    let export_btn = button(
        row![text(ui::icons::SAVE).font(ICON_FONT), text("Export")]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .on_press(Message::OpenExportModal)
    .padding([8, 12])
    .style(style_toolbar_button);

    let size_slider = row![
        text(ui::icons::TH_LARGE).font(ICON_FONT).size(13),
        slider(100.0..=400.0, editor.thumbnail_size, Message::SetThumbnailSize)
            .width(Length::Fixed(140.0)),
        text(ui::icons::TH).font(ICON_FONT).size(18),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(
        column![
            row![
                column![
                    text("Library").size(22),
                    text("Browse, cull, and prepare your selection")
                        .size(12)
                        .style(|_| text::Style {
                            color: Some(Color::from_rgb(0.58, 0.58, 0.58))
                        }),
                ]
                .spacing(2),
                Space::with_width(Length::Fill),
                metric_pill("Shown", format!("{}/{}", filtered_count, total_count), Color::from_rgb(0.82, 0.82, 0.82)),
                metric_pill("Selected", selected_count, Color::from_rgb(0.58, 0.78, 1.0)),
                metric_pill("Missing", deleted_count, Color::from_rgb(0.94, 0.45, 0.45)),
                Space::with_width(Length::Fixed(8.0)),
                import_btn,
                export_btn,
                Space::with_width(Length::Fixed(8.0)),
                size_slider,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            row![
                view_filter_bar(editor),
                Space::with_width(Length::Fill),
                view_quick_actions(editor),
                Space::with_width(Length::Fixed(12.0)),
                checkbox("Auto-Advance", editor.auto_advance)
                    .on_toggle(|_| Message::ToggleAutoAdvance)
                    .size(16)
                    .spacing(6)
                    .text_size(12)
                    .style(|_theme, _status| checkbox::Style {
                        text_color: Some(Color::from_rgb(0.72, 0.72, 0.72)),
                        background: Background::Color(Color::from_rgb(0.20, 0.20, 0.20)),
                        icon_color: Color::from_rgb(0.88, 0.88, 0.88),
                        border: Border {
                            color: Color::from_rgb(0.36, 0.36, 0.36),
                            width: 1.0,
                            radius: 3.0.into(),
                        },
                    }),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(10)
        .width(Length::Fill),
    )
    .padding(12)
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.10, 0.10, 0.10))),
        border: Border {
            width: 1.0,
            color: Color::from_rgb(0.16, 0.16, 0.16),
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn view_status_bar<'a>(
    editor: &'a RawEditor,
    filtered_count: usize,
    total_count: usize,
    selected_count: usize,
    cached_count: usize,
    deleted_count: usize,
) -> Element<'a, Message> {
    container(
        row![
            metric_pill("Items", format!("{}/{}", filtered_count, total_count), Color::from_rgb(0.82, 0.82, 0.82)),
            metric_pill("Selected", selected_count, Color::from_rgb(0.58, 0.78, 1.0)),
            metric_pill("Cached", format!("{}/{}", cached_count, filtered_count), Color::from_rgb(0.54, 0.78, 0.58)),
            metric_pill("Deleted", deleted_count, Color::from_rgb(0.94, 0.45, 0.45)),
            iced::widget::horizontal_space(),
            text(&editor.status).size(12).style(|_| text::Style {
                color: Some(Color::from_rgb(0.62, 0.62, 0.62))
            }),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .height(Length::Fixed(42.0))
    .padding([0, 10])
    .align_y(iced::alignment::Vertical::Center)
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.07, 0.07, 0.07))),
        border: Border {
            width: 1.0,
            color: Color::from_rgb(0.14, 0.14, 0.14),
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}
