use iced::widget::{button, container, image, row, scrollable, text, stack};
use iced::{Background, Border, Color, Element, Length, Theme};
use std::path::PathBuf;
use std::collections::HashSet;  // Phase 55: Multi-selection

use crate::state::data::Image;
use crate::Message;
use crate::ui::icons;  // Phase 58: Icon constants

/// Render the filmstrip timeline at the bottom of the Develop tab
/// Phase 55: Now accepts HashSet for multi-selection
pub fn view<'a>(images: &'a [Image], selection: &HashSet<i64>) -> Element<'a, Message> {
    // Build row with thumbnails
    let mut thumbnails = Vec::new();

    for img in images {
        // Skip images without thumbnail
        let thumb_path_str = match &img.cache_path_thumb {
            Some(p) => p,
            None => continue, // Skip this image
        };
        
        // Phase 55: Check if image is in selection set
        let is_selected = selection.contains(&img.id);
        
        // Get thumbnail path (256px cache)
        let thumb_path = PathBuf::from(thumb_path_str);
        
        // Create image widget
        let img_widget = image(thumb_path)
            .width(Length::Fixed(140.0))
            .height(Length::Fixed(105.0));
        
        // Phase 56: Overlay star rating on bottom-left corner
        let rating_overlay = if img.rating > 0 {
            let stars_text = vec![icons::STAR; img.rating as usize].join(" ");
            container(
                text(stars_text)  // Space between stars
                    .size(14)
                    .color(Color::from_rgb(1.0, 0.8, 0.2))  // Gold
                    .font(crate::ICON_FONT)  // Phase 57: Use embedded font
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
            .height(Length::Fill)
        } else {
            container(text("")).width(Length::Fill).height(Length::Fill)
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
                if is_selected {
                    // Selected: light grey background only (no border)
                    container::Style {
                        background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))),
                        ..Default::default()
                    }
                } else {
                    // Unselected: no styling
                    container::Style::default()
                }
            });
        
        // Wrap in button for clicking
        let thumbnail_button = button(thumbnail_container)
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
    container(scrollable_film)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(0)
        .style(|_theme: &Theme| {
            container::Style {
                background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
                ..Default::default()
            }
        })
        .into()
}
