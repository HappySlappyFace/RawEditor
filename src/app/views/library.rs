use iced::widget::image::Handle;
use iced::widget::{
    button, container, column, row, scrollable, stack, text, Image, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use iced_aw::Wrap;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::database::models::Image as ImageData;
use crate::ui;
use crate::ui::icons::ICON_FONT;

const THUMB_PRESET_SMALL: f32 = 150.0;
const THUMB_PRESET_MEDIUM: f32 = 190.0;
const THUMB_PRESET_LARGE: f32 = 230.0;
const THUMB_PRESET_XL: f32 = 280.0;

fn metric_pill<'a>(label: &'a str, value: impl ToString, color: Color) -> Element<'a, Message> {
    container(
        row![
            text(label).size(11).style(|_| text::Style {
                color: Some(Color::from_rgb(0.68, 0.68, 0.68))
            }),
            text(value.to_string())
                .size(11)
                .style(move |_| text::Style { color: Some(color) }),
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

fn style_sidebar_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        if active {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.20, 0.36, 0.54))),
                text_color: Color::WHITE,
                border: Border {
                    radius: 6.0.into(),
                    width: 1.0,
                    color: Color::from_rgb(0.38, 0.57, 0.76),
                },
                ..Default::default()
            }
        } else {
            match status {
                button::Status::Hovered => button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.20, 0.20, 0.20))),
                    text_color: Color::from_rgb(0.92, 0.92, 0.92),
                    border: Border {
                        radius: 6.0.into(),
                        width: 1.0,
                        color: Color::from_rgb(0.28, 0.28, 0.28),
                    },
                    ..Default::default()
                },
                _ => button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.14, 0.14, 0.14))),
                    text_color: Color::from_rgb(0.74, 0.74, 0.74),
                    border: Border {
                        radius: 6.0.into(),
                        width: 1.0,
                        color: Color::from_rgb(0.20, 0.20, 0.20),
                    },
                    ..Default::default()
                },
            }
        }
    }
}

fn style_sidebar_action_button(_theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered => button::Style {
            background: Some(Background::Color(Color::from_rgb(0.24, 0.24, 0.24))),
            text_color: Color::from_rgb(0.94, 0.94, 0.94),
            border: Border {
                radius: 6.0.into(),
                width: 0.0,
                color: Color::TRANSPARENT,
            },
            ..Default::default()
        },
        button::Status::Pressed => button::Style {
            background: Some(Background::Color(Color::from_rgb(0.16, 0.16, 0.16))),
            text_color: Color::from_rgb(0.90, 0.90, 0.90),
            border: Border {
                radius: 6.0.into(),
                width: 0.0,
                color: Color::TRANSPARENT,
            },
            ..Default::default()
        },
        _ => button::Style {
            background: Some(Background::Color(Color::from_rgb(0.19, 0.19, 0.19))),
            text_color: Color::from_rgb(0.84, 0.84, 0.84),
            border: Border {
                radius: 6.0.into(),
                width: 0.0,
                color: Color::TRANSPARENT,
            },
            ..Default::default()
        },
    }
}

fn compact_path(path: &str) -> String {
    let path = std::path::Path::new(path);
    let parts: Vec<String> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect();

    if parts.len() >= 2 {
        let last = &parts[parts.len() - 1];
        let before_last = &parts[parts.len() - 2];
        let compact = format!(".../{}/{}", before_last, last);
        if compact.len() <= 34 {
            return compact;
        }
    }

    let as_str = path.to_string_lossy();
    if as_str.len() <= 36 {
        return as_str.to_string();
    }
    let tail: String = as_str
        .chars()
        .rev()
        .take(33)
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{}", tail)
}

fn collect_folder_stats(editor: &RawEditor) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for image in &editor.images {
        if let Some(parent) = std::path::Path::new(&image.path).parent() {
            let key = parent.to_string_lossy().to_string();
            *counts.entry(key).or_insert(0) += 1;
        }
    }

    let mut folders: Vec<(String, usize)> = counts.into_iter().collect();
    folders.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    folders.truncate(16);
    folders
}

fn is_thumb_preset_active(current: f32, preset: f32) -> bool {
    (current - preset).abs() < 1.0
}

fn view_thumbnail_size_presets<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    row![
        text("Card size")
            .size(12)
            .style(|_| text::Style { color: Some(Color::from_rgb(0.62, 0.62, 0.62)) }),
        button(text("S").size(11))
            .on_press(Message::SetThumbnailSize(THUMB_PRESET_SMALL))
            .padding([4, 8])
            .style(style_chip(is_thumb_preset_active(
                editor.thumbnail_size,
                THUMB_PRESET_SMALL
            ))),
        button(text("M").size(11))
            .on_press(Message::SetThumbnailSize(THUMB_PRESET_MEDIUM))
            .padding([4, 8])
            .style(style_chip(is_thumb_preset_active(
                editor.thumbnail_size,
                THUMB_PRESET_MEDIUM
            ))),
        button(text("L").size(11))
            .on_press(Message::SetThumbnailSize(THUMB_PRESET_LARGE))
            .padding([4, 8])
            .style(style_chip(is_thumb_preset_active(
                editor.thumbnail_size,
                THUMB_PRESET_LARGE
            ))),
        button(text("XL").size(11))
            .on_press(Message::SetThumbnailSize(THUMB_PRESET_XL))
            .padding([4, 8])
            .style(style_chip(is_thumb_preset_active(
                editor.thumbnail_size,
                THUMB_PRESET_XL
            ))),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn view_image_card<'a>(img: &'a ImageData, is_selected: bool, size: f32) -> Element<'a, Message> {
    let thumb_width = size;
    let thumb_height = size * 0.75;
    let is_deleted = img.file_status == "deleted";
    let thumb_missing = img.cache_path_thumb.is_none();
    let has_loaded_thumb = !is_deleted && img.cache_path_thumb.is_some();

    let image_content: Element<'a, Message> = if has_loaded_thumb {
        if let Some(thumb_path) = img.cache_path_thumb.as_ref() {
            Image::new(Handle::from_path(PathBuf::from(thumb_path)))
                .content_fit(iced::ContentFit::Cover)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            Space::with_width(Length::Fill).into()
        }
    } else {
        let placeholder_label = if is_deleted {
            "Missing..."
        } else if thumb_missing {
            "Queued..."
        } else {
            "Generating..."
        };
        container(
            column![
                text(ui::icons::TH_LARGE).font(ICON_FONT).size(24),
                text(placeholder_label).size(11).style(|_| text::Style {
                    color: Some(Color::from_rgb(0.88, 0.88, 0.88))
                })
            ]
            .spacing(6)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.12, 0.12, 0.12))),
            border: Border {
                radius: 0.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    };

    let top_left_badge: Element<'a, Message> = if has_loaded_thumb && img.flag == 1 {
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
    } else if has_loaded_thumb && img.flag == -1 {
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

    let top_right_badge: Element<'a, Message> = if has_loaded_thumb && img.rating > 0 {
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

    let image_stack = stack![
        container(image_content)
            .width(Length::Fixed(thumb_width))
            .height(Length::Fixed(thumb_height))
            .clip(true)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.10, 0.10, 0.10))),
                border: Border {
                    radius: 0.0.into(),
                    ..Default::default()
                },
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
    ];

    let card = container(image_stack)
        .width(Length::Fixed(thumb_width + 14.0))
        .height(Length::Fixed(thumb_height + 14.0))
        .padding(7)
        .clip(true)
        .style(move |_| {
            if is_selected {
                container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.18, 0.27, 0.36))),
                    border: Border {
                        radius: 0.0.into(),
                        width: 1.0,
                        color: Color::from_rgb(0.39, 0.61, 0.82),
                    },
                    ..Default::default()
                }
            } else {
                container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.09, 0.09, 0.09))),
                    border: Border {
                        radius: 0.0.into(),
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

fn view_library_sidebar<'a>(
    editor: &'a RawEditor,
    folder_stats: &[(String, usize)],
    total_count: usize,
) -> Element<'a, Message> {
    let selected_filter = editor.library_folder_filter.clone();
    let all_active = selected_filter.is_none();

    let mut folders_column = column![
        button(
            row![
                text("All Photos").size(12),
                Space::with_width(Length::Fill),
                text(total_count.to_string()).size(11)
            ]
            .align_y(Alignment::Center)
        )
        .on_press(Message::SetLibraryFolderFilter(None))
        .padding([7, 9])
        .width(Length::Fill)
        .style(style_sidebar_button(all_active)),
    ]
    .spacing(6);

    for (folder, count) in folder_stats {
        let is_active = selected_filter
            .as_ref()
            .map(|active| active == folder)
            .unwrap_or(false);
        folders_column = folders_column.push(
            button(
                row![
                    text(compact_path(folder)).size(11),
                    Space::with_width(Length::Fill),
                    text(count.to_string()).size(10)
                ]
                .align_y(Alignment::Center),
            )
            .on_press(Message::SetLibraryFolderFilter(Some(folder.clone())))
            .padding([6, 8])
            .width(Length::Fill)
            .style(style_sidebar_button(is_active)),
        );
    }

    container(
        column![
            text("Library Sources").size(14),
            text("Projects and locations")
                .size(11)
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.58, 0.58, 0.58))
                }),
            Space::with_height(Length::Fixed(8.0)),
            button(
                row![
                    text(ui::icons::FOLDER_OPEN).font(ICON_FONT),
                    text("Import Folder")
                ]
                .spacing(8)
                .align_y(Alignment::Center)
            )
            .on_press(Message::ImportFolder)
            .padding([8, 10])
            .width(Length::Fill)
            .style(style_sidebar_action_button),
            button(
                row![
                    text(ui::icons::FOLDER).font(ICON_FONT),
                    text("Import from Camera")
                ]
                .spacing(8)
                .align_y(Alignment::Center)
            )
            .on_press(Message::ImportFromCamera)
            .padding([8, 10])
            .width(Length::Fill)
            .style(style_sidebar_action_button),
            Space::with_height(Length::Fixed(10.0)),
            text("Folders").size(12).style(|_| text::Style {
                color: Some(Color::from_rgb(0.68, 0.68, 0.68))
            }),
            scrollable(folders_column).height(Length::Fill),
        ]
        .spacing(6)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .padding(10)
    .width(Length::Fixed(250.0))
    .height(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.09, 0.09, 0.09))),
        border: Border {
            width: 1.0,
            color: Color::from_rgb(0.16, 0.16, 0.16),
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn view_library_toolbar<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    let export_btn = button(
        row![text(ui::icons::SAVE).font(ICON_FONT), text("Export")]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .on_press(Message::OpenExportModal)
    .padding([8, 12])
    .style(style_toolbar_button);

    container(
        row![
            text("Library").size(22),
            view_filter_bar(editor),
            Space::with_width(Length::Fill),
            export_btn,
        ]
        .spacing(10)
        .align_y(Alignment::Center)
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
    let folder_label = editor
        .library_folder_filter
        .as_ref()
        .map(|folder| compact_path(folder))
        .unwrap_or_else(|| "All folders".to_string());

    container(
        row![
            metric_pill(
                "Items",
                format!("{}/{}", filtered_count, total_count),
                Color::from_rgb(0.82, 0.82, 0.82),
            ),
            text("|").size(12).style(|_| text::Style {
                color: Some(Color::from_rgb(0.28, 0.28, 0.28))
            }),
            metric_pill("Selected", selected_count, Color::from_rgb(0.58, 0.78, 1.0)),
            text("|").size(12).style(|_| text::Style {
                color: Some(Color::from_rgb(0.28, 0.28, 0.28))
            }),
            metric_pill(
                "Cached",
                format!("{}/{}", cached_count, filtered_count),
                Color::from_rgb(0.54, 0.78, 0.58),
            ),
            text("|").size(12).style(|_| text::Style {
                color: Some(Color::from_rgb(0.28, 0.28, 0.28))
            }),
            metric_pill("Deleted", deleted_count, Color::from_rgb(0.94, 0.45, 0.45)),
            text("|").size(12).style(|_| text::Style {
                color: Some(Color::from_rgb(0.28, 0.28, 0.28))
            }),
            metric_pill("Folder", folder_label, Color::from_rgb(0.82, 0.82, 0.82)),
            iced::widget::horizontal_space(),
            view_thumbnail_size_presets(editor),
            Space::with_width(Length::Fixed(8.0)),
            text(&editor.status).size(12).style(|_| text::Style {
                color: Some(Color::from_rgb(0.62, 0.62, 0.62))
            }),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(42.0))
    .padding([0, 10])
    .clip(true)
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

pub fn view_library(editor: &RawEditor) -> Element<'_, Message> {
    let folder_stats = collect_folder_stats(editor);
    let filtered_images: Vec<&ImageData> = editor
        .images
        .iter()
        .filter(|img| editor.min_filter_rating == 0 || img.rating >= editor.min_filter_rating)
        .filter(|img| {
            editor
                .library_folder_filter
                .as_ref()
                .map(|folder| img.path.starts_with(folder))
                .unwrap_or(true)
        })
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

    let grid_content: Element<'_, Message> = if filtered_images.is_empty() {
        container(
            column![
                text("No images match this filter").size(24),
                text("Try another folder/rating or import a new directory.")
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

        let centered_grid = container(thumbnail_grid)
            .width(Length::Shrink)
            .align_x(iced::alignment::Horizontal::Center);

        container(
            container(centered_grid)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .padding([12, 12]),
        )
        .width(Length::Fill)
        .into()
    };

    let body = row![
        view_library_sidebar(editor, &folder_stats, total_count),
        container(
            scrollable(grid_content)
                .height(Length::Fill)
                .width(Length::Fill)
        )
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
                ..Default::default()
            })
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .clip(true);

    column![
        container(view_library_toolbar(editor))
            .width(Length::Fill)
            .clip(true),
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true),
        view_status_bar(
            editor,
            filtered_count,
            total_count,
            selected_count,
            cached_count,
            deleted_count
        )
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
