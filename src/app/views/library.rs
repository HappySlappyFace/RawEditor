use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use iced::widget::{button, checkbox, column, container, row, scrollable, slider, stack, text, Image, Space};
use iced::widget::image::Handle;
use std::path::PathBuf;
use iced_aw::Wrap;

use crate::ui;
use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::database::models::Image as ImageData;
use crate::ui::icons::ICON_FONT;

/// Build the Library tab view (grid of thumbnails)
fn view_image_card<'a>(img: &'a ImageData, is_selected: bool, size: f32) -> Element<'a, Message> {
    let thumb_width = size;
    let thumb_height = size * 0.75; // 4:3 aspect ratio
    
    let is_deleted = img.file_status == "deleted";
    
    // 1. The Image Content
    let image_widget = if let Some(ref thumb_path) = img.cache_path_thumb {
        let handle = Handle::from_path(PathBuf::from(thumb_path));
        Image::new(handle).content_fit(iced::ContentFit::Contain)
            .width(Length::Fixed(thumb_width))
            .height(Length::Fixed(thumb_height))
    } else {
        Image::new(Handle::from_path(PathBuf::new())) // Placeholder or empty
            .width(Length::Fixed(thumb_width))
            .height(Length::Fixed(thumb_height))
    };
    
    // 2. Rating Overlay (Bottom-Left)
    let rating_overlay = if img.rating > 0 {
        let stars_text = vec![ui::icons::STAR; img.rating as usize].join(" ");
        container(
            text(stars_text).size(12).font(ICON_FONT).style(|_| text::Style { color: Some(Color::from_rgb(1.0, 0.8, 0.2)) })
        )
        .padding([2, 4])
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))),
            border: Border { radius: 3.0.into(), ..Default::default() },
            ..Default::default()
        })
    } else {
        container(text(""))
    };
    
    // 3. Flag Overlay (Full or Corner)
    let flag_overlay = if img.flag == -1 {
        // Reject: Full red X overlay
        container(
            text(ui::icons::TIMES).font(ICON_FONT).size(thumb_width * 0.3).style(|_| text::Style { color: Some(Color::from_rgba(1.0, 0.2, 0.2, 0.4)) })
        )
        .width(Length::Fill).height(Length::Fill)
        .center_x(Length::Fill).center_y(Length::Fill)
        .style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))), ..Default::default() })
    } else if img.flag == 1 {
        // Pick: Green check in top-right
        container(
            text(ui::icons::CHECK).font(ICON_FONT).size(16).style(|_| text::Style { color: Some(Color::from_rgba(0.2, 1.0, 0.2, 0.9)) })
        )
        .padding(4)
        .width(Length::Fill).height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Top)
        .style(|_| container::Style::default())
    } else {
        container(text(""))
    };

    // 4. Deleted Overlay
    let deleted_overlay = if is_deleted {
            container(column![text(ui::icons::TIMES).size(24).font(ICON_FONT), text("Deleted").size(10)].align_x(Alignment::Center).spacing(2))
            .width(Length::Fill).height(Length::Fill)
            .center_x(Length::Fill).center_y(Length::Fill)
            .style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.8))), ..Default::default() })
    } else {
        container(text(""))
    };

    // Stack 'em up
    let content = stack![
        container(image_widget).style(|_| container::Style { background: Some(Background::Color(Color::from_rgb(0.15, 0.15, 0.15))), ..Default::default() }), // Placeholder background
        container(rating_overlay).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Left).align_y(iced::alignment::Vertical::Bottom).padding(4),
        flag_overlay,
        deleted_overlay
    ];

    // Wrapper for Selection Styling
    let wrapper = container(content)
        .width(Length::Fixed(thumb_width + 8.0)) // +8 for padding
        .height(Length::Fixed(thumb_height + 8.0))
        .padding(4) // Selection border thickness
        .style(move |_| {
            if is_selected {
                container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), // Selection Highlight Color
                    border: Border { radius: 4.0.into(), ..Default::default() },
                    ..Default::default()
                }
            } else {
                container::Style::default()
            }
        });

    button(wrapper).on_press(Message::ImageSelected(img.id)).padding(0).style(|theme, status| button::Style { background: None, border: Border::default(), ..button::primary(theme, status) }).into()
}

pub fn view_library(editor: &RawEditor) -> Element<'_, Message> {
    let filtered_images: Vec<&ImageData> = editor.images.iter()
        .filter(|img| editor.min_filter_rating == 0 || img.rating >= editor.min_filter_rating)
        .collect();
    
    let cached_count = filtered_images.iter().filter(|img| img.cache_path_thumb.is_some()).count();
    let deleted_count = filtered_images.iter().filter(|img| img.file_status == "deleted").count();
    let total_count = editor.images.len();
    let filtered_count = filtered_images.len();
    
    let thumbnail_grid = filtered_images.iter().fold(
        Wrap::new().spacing(10.0).line_spacing(10.0),
        |wrap, img| {
            let is_selected = editor.selected_image_id == Some(img.id) || editor.multi_selection.contains(&img.id);
            wrap.push(view_image_card(img, is_selected, editor.thumbnail_size))
        },
    );
    
    column![
        view_library_toolbar(editor),
        container(scrollable(container(thumbnail_grid).width(Length::Fill)).height(Length::Fill).width(Length::Fill)).padding(10).height(Length::Fill),
        view_status_bar(editor, filtered_count, total_count, cached_count, deleted_count)
    ].into()
}

fn view_filter_bar<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    row![
        text("Filter: ").size(14).style(|_| text::Style { color: Some(Color::from_rgb(0.7, 0.7, 0.7)) }),
        button(text("All").size(12)).on_press(Message::SetMinRating(0)).padding(5).style(if editor.min_filter_rating == 0 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { ui::styles::NeutralButton::style }),
        button(row![text(ui::icons::STAR).font(ICON_FONT).size(12), text(" 1+").size(12)]).on_press(Message::SetMinRating(1)).padding(5).style(if editor.min_filter_rating == 1 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { ui::styles::NeutralButton::style }),
        button(row![text(format!("{} {}", ui::icons::STAR, ui::icons::STAR)).font(ICON_FONT).size(12), text(" 2+").size(12)]).on_press(Message::SetMinRating(2)).padding(5).style(if editor.min_filter_rating == 2 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { ui::styles::NeutralButton::style }),
        button(row![text(format!("{} {} {}", ui::icons::STAR, ui::icons::STAR, ui::icons::STAR)).font(ICON_FONT).size(12), text(" 3+").size(12)]).on_press(Message::SetMinRating(3)).padding(5).style(if editor.min_filter_rating == 3 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { ui::styles::NeutralButton::style }),
        button(row![text(format!("{} {} {} {}", ui::icons::STAR, ui::icons::STAR, ui::icons::STAR, ui::icons::STAR)).font(ICON_FONT).size(12), text(" 4+").size(12)]).on_press(Message::SetMinRating(4)).padding(5).style(if editor.min_filter_rating == 4 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { ui::styles::NeutralButton::style }),
        button(row![text(format!("{} {} {} {} {}", ui::icons::STAR, ui::icons::STAR, ui::icons::STAR, ui::icons::STAR, ui::icons::STAR)).font(ICON_FONT).size(12), text(" 5").size(12)]).on_press(Message::SetMinRating(5)).padding(5).style(if editor.min_filter_rating == 5 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { ui::styles::NeutralButton::style }),
        // Phase 83: Auto-Advance Toggle
        iced::widget::checkbox("Auto-Advance", editor.auto_advance)
            .on_toggle(|_| Message::ToggleAutoAdvance)
            .size(16)
            .spacing(5)
            .text_size(12)
            .style(|_theme, _status| checkbox::Style {
                text_color: Some(Color::from_rgb(0.7, 0.7, 0.7)),
                background: Background::Color(Color::from_rgb(0.2, 0.2, 0.2)),
                icon_color: Color::from_rgb(0.85, 0.85, 0.85),
                border: Border { color: Color::from_rgb(0.4, 0.4, 0.4), width: 1.0, radius: 3.0.into() },
            }),
    ].spacing(5).align_y(Alignment::Center).into()
}

fn view_library_toolbar<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    let import_btn = button(
        row![
            text(ui::icons::FOLDER_OPEN).font(ICON_FONT),
            text("Import Folder")
        ].spacing(8)
    )
    .on_press(Message::ImportFolder)
    .padding(10)
    .style(|theme, status| button::Style {
        background: Some(Background::Color(Color::from_rgb(0.2, 0.2, 0.2))),
        text_color: Color::WHITE,
        border: Border { radius: 4.0.into(), ..Default::default() },
        ..button::primary(theme, status)
    });

    let export_btn = button(
        row![
            text(ui::icons::SAVE).font(ICON_FONT),
            text("Export")
        ].spacing(8)
    )
    .on_press(Message::OpenExportModal)
    .padding(10)
    .style(|theme, status| button::Style {
        background: Some(Background::Color(Color::from_rgb(0.2, 0.2, 0.2))),
        text_color: Color::WHITE,
        border: Border { radius: 4.0.into(), ..Default::default() },
        ..button::primary(theme, status)
    });

    let size_slider = row![
        text(ui::icons::TH_LARGE).font(ICON_FONT).size(16),
        slider(100.0..=400.0, editor.thumbnail_size, Message::SetThumbnailSize).width(Length::Fixed(150.0)),
        text(ui::icons::TH).font(ICON_FONT).size(20),
    ].spacing(10).align_y(Alignment::Center);

    container(
        row![
            import_btn,
            export_btn,
            Space::with_width(Length::Fill),
            size_slider,
            Space::with_width(20.0),
            view_filter_bar(editor)
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .width(Length::Fill)
    )
    .padding(10)
    .style(|_| container::Style { background: Some(Background::Color(Color::from_rgb(0.12, 0.12, 0.12))), ..Default::default() })
    .into()
}

fn view_status_bar<'a>(editor: &'a RawEditor, filtered_count: usize, total_count: usize, cached_count: usize, deleted_count: usize) -> Element<'a, Message> {
    container(
        row![
            text(format!("Showing: {}/{}", filtered_count, total_count)).size(12).style(|_| text::Style { color: Some(Color::from_rgb(0.5, 0.5, 0.5)) }),
            text(" | ").size(12).style(|_| text::Style { color: Some(Color::from_rgb(0.3, 0.3, 0.3)) }),
            text(format!("Thumbnails: {}/{}", cached_count, filtered_count)).size(12).style(|_| text::Style { color: Some(Color::from_rgb(0.5, 0.5, 0.5)) }),
            text(" | ").size(12).style(|_| text::Style { color: Some(Color::from_rgb(0.3, 0.3, 0.3)) }),
            text(format!("Deleted: {}", deleted_count)).size(12).style(|_| text::Style { color: Some(Color::from_rgb(0.5, 0.5, 0.5)) }),
            iced::widget::horizontal_space(),
            text(&editor.status).size(12).style(|_| text::Style { color: Some(Color::from_rgb(0.6, 0.6, 0.6)) }),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
    )
    .height(Length::Fixed(30.0))
    .padding([0, 10])
    .align_y(iced::alignment::Vertical::Center)
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
        border: Border {
            width: 1.0,
            color: Color::from_rgb(0.15, 0.15, 0.15),
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}
