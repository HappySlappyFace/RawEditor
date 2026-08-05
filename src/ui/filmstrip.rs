use iced::widget::{button, container, image, mouse_area, row, scrollable, text, stack, Space};
use iced::{Background, Border, Color, Element, Length, Theme};
use std::path::PathBuf;

use crate::database::models::{Image, MARKED_FOR_REMOVAL_RATING};
use crate::app::message::Message;
use crate::ui::icons;  // Phase 58: Icon constants

/// Id shared by the Cull and Develop filmstrips — only one is ever mounted at
/// a time, so a single static id is enough to target it with `scroll_by`.
pub fn scroll_id() -> scrollable::Id {
    scrollable::Id::new("filmstrip")
}

/// Render the filmstrip timeline at the bottom of the Develop and Cull tabs.
///
/// Takes the whole editor rather than a selection set so it can use
/// `RawEditor::is_selected` — the same predicate the library grid uses. It
/// previously read `multi_selection` alone, which disagreed with the grid
/// whenever `selected_image_id` was not also in the set.
pub fn view<'a>(
    images: &[&'a Image],
    editor: &crate::app::state::RawEditor,
) -> Element<'a, Message> {
    // Build row with thumbnails
    let mut thumbnails = Vec::new();

    for img in images {
        let img = *img;
        // Skip images without thumbnail
        let thumb_path_str = match &img.cache_path_thumb {
            Some(p) => p,
            None => continue, // Skip this image
        };
        
        let is_selected = editor.is_selected(img.id);
        
        // Get thumbnail path (256px cache)
        let thumb_path = PathBuf::from(thumb_path_str);
        
        // Create image widget
        let img_widget = image(iced::widget::image::Handle::from_path(thumb_path))
            .width(Length::Fixed(140.0))
            .height(Length::Fixed(105.0));
        
        // Phase 56: Overlay star rating (or the "marked for removal" badge,
        // which reuses this same field via a reserved sentinel — must be
        // checked first or `img.rating as usize` below would try to build a
        // 255-element star vec) on the bottom-left corner.
        let rating_overlay = if img.rating == MARKED_FOR_REMOVAL_RATING {
            Element::from(container(
                container(
                    row![
                        text(icons::TIMES).size(12).font(crate::app::view::ICON_FONT),
                        text("Marked").size(12),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center)
                )
                .padding(iced::Padding { top: 2.0, right: 4.0, bottom: 2.0, left: 4.0 })
                .style(|_theme: &Theme| {
                    container::Style {
                        background: Some(Background::Color(Color::from_rgba(0.85, 0.55, 0.05, 0.9))),
                        border: Border { radius: 3.0.into(), ..Default::default() },
                        ..Default::default()
                    }
                })
            )
            .padding(iced::Padding { top: 0.0, right: 0.0, bottom: 2.0, left: 2.0 })
            .align_y(iced::alignment::Vertical::Bottom)
            .align_x(iced::alignment::Horizontal::Left)
            .width(Length::Fill)
            .height(Length::Fill))
        } else if img.rating > 0 {
            let stars_text = vec![icons::STAR; img.rating as usize].join(" ");
            Element::from(container(
                container(
                    text(stars_text)  // Space between stars
                        .size(14)
                        .color(Color::from_rgb(1.0, 0.8, 0.2))  // Gold
                        .font(crate::app::view::ICON_FONT)  // Phase 57: Use embedded font
                )
                .padding(iced::Padding {
                    top: 2.0,
                    right: 4.0,
                    bottom: 2.0,
                    left: 4.0,
                })
                .style(|_theme: &Theme| {
                    container::Style {
                        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))),  // Dark semi-transparent
                        border: Border {
                            radius: 3.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
            )
            .padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 2.0,  // 2px from bottom
                left: 2.0,    // 2px from left
            })
            .align_y(iced::alignment::Vertical::Bottom)
            .align_x(iced::alignment::Horizontal::Left)
            .width(Length::Fill)
            .height(Length::Fill))
        } else {
            Element::from(container(text("")).width(Length::Fill).height(Length::Fill))
        };
        
        // Stack image with rating overlay
        let thumbnail_content = stack![
            img_widget,
            rating_overlay
        ];
        
        // Wrap in container for selection styling
        let thumbnail_container = container(thumbnail_content)
            .padding(iced::Padding {
                top: 4.0,      // Top padding
                right: 16.0,    // Creates 4px gap between images (2+2)
                bottom: 4.0,   // Bottom padding
                left: 16.0,     // Creates 4px gap between images (2+2)
            })
            .style(move |_theme: &Theme| {
                let mut style = if is_selected {
                    // Selected: light grey background only (no border)
                    container::Style {
                        background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))),
                        ..Default::default()
                    }
                } else {
                    // Unselected: no styling
                    container::Style::default()
                };
                
                // Phase 83: Flag Feedback
                if img.flag == -1 {
                    style.text_color = Some(Color::from_rgba(1.0, 1.0, 1.0, 0.5)); // Dim text if any
                }
                style
            });
            
        // Phase 83: Apply opacity/overlay
        let mut final_content = Element::from(thumbnail_container);
        
        if img.flag == -1 {
            // Rejected: Overlay a semi-transparent black box with "X"
            let reject_overlay = container(
                text(icons::TIMES).font(crate::app::view::ICON_FONT).size(24).color(Color::from_rgba(1.0, 0.2, 0.2, 0.8))
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))), ..Default::default() });
            
            final_content = Element::from(stack![
                final_content,
                reject_overlay
            ]);
        } else if img.flag == 1 {
             // Picked: Overlay a small green flag/check
            let pick_overlay = container(
                text(icons::CHECK).font(crate::app::view::ICON_FONT).size(16).color(Color::from_rgba(0.2, 1.0, 0.2, 0.9))
            )
            .padding(4)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top)
            .width(Length::Fill)
            .height(Length::Fill);
            
            final_content = Element::from(stack![
                final_content,
                pick_overlay
            ]);
        }

        // Wrap in button for clicking
        let thumbnail_button = button(final_content)
            .padding(0)  // Remove button's default padding
            .on_press(Message::ImageSelected(img.id))
            .style(|_theme: &Theme, status| {
                button::Style {
                    background: None,
                    border: Border::default(),
                    ..button::primary(_theme, status)
                }
            })
            .into(); // Convert to Element
        
        thumbnails.push(thumbnail_button);
    }

    // Create row from thumbnails - minimal spacing
    let film_row = row(thumbnails)
        .spacing(0)
        .padding(iced::Padding {
            top: 2.0,
            right: 0.0,
            bottom: 10.0,  // Extra space for scrollbar
            left: 0.0,
        });
    // Make it scrollable horizontally
    let scrollable_film = scrollable(film_row)
        .id(scroll_id())
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new()
                .width(8)
                .scroller_width(8)
        ))
        .style(|_theme: &Theme, _status| {
            scrollable::Style {
                container: container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
                    ..Default::default()
                },
                vertical_rail: scrollable::Rail {
                    background: None,
                    border: Border::default(),
                    scroller: scrollable::Scroller {
                        color: Color::from_rgb(0.3, 0.3, 0.3),
                        border: Border::default(),
                    },
                },
                horizontal_rail: scrollable::Rail {
                    background: Some(Background::Color(Color::from_rgb(0.1, 0.1, 0.1))), // Track color
                    border: Border::default(),
                    scroller: scrollable::Scroller {
                        color: Color::from_rgb(0.4, 0.4, 0.4), // Draggable scroller color
                        border: Border::default(),
                    },
                },
                gap: None,
            }
        });
    
    // Dark background container (no padding to avoid wasted space)
    let scroll_surface = container(scrollable_film)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(0)
        .clip(true)
        .style(|_theme: &Theme| {
            container::Style {
                background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
                ..Default::default()
            }
        });

    // Wheel interceptor, stacked ABOVE the scrollable (later stack children
    // get first crack at events in iced). Capturing the wheel event here
    // short-circuits iced's Stack dispatch before the scrollable underneath
    // ever sees it, so its hardcoded ~60px/line native scroll never fires —
    // handlers::scroll::handle_filmstrip_wheel drives 100% of the motion,
    // giving a single continuous physics model with no jump/seam.
    let wheel_capture = mouse_area(Space::new(Length::Fill, Length::Fill))
        .on_scroll(Message::FilmstripWheel);

    stack![scroll_surface, wheel_capture]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
