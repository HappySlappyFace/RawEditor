use iced::widget::{button, container, image, row, scrollable};
use iced::{Background, Border, Color, Element, Length, Theme};
use std::path::PathBuf;

use crate::state::data::Image;
use crate::Message;

/// Render the filmstrip timeline at the bottom of the Develop tab
pub fn view<'a>(images: &'a [Image], selected_id: Option<i64>) -> Element<'a, Message> {
    // Build row with thumbnails
    let mut thumbnails = Vec::new();

    for img in images {
        // Skip images without thumbnail
        let thumb_path_str = match &img.cache_path_thumb {
            Some(p) => p,
            None => continue, // Skip this image
        };
        
        let is_selected = Some(img.id) == selected_id;
        
        // Get thumbnail path (256px cache)
        let thumb_path = PathBuf::from(thumb_path_str);
        
        // Create image widget
        let img_widget = image(thumb_path)
            .width(Length::Fixed(120.0))
            .height(Length::Fixed(80.0));
        
        // Wrap in container for border styling
        let thumbnail_container = container(img_widget)
            .padding(6)  // Padding so border is visible
            .style(move |theme: &Theme| {
                if is_selected {
                    // Selected: thick bright border
                    container::Style {
                        border: Border {
                            color: Color::from_rgb(0.3, 0.6, 1.0), // Blue
                            width: 6.0,
                            radius: 4.0.into(),
                        },
                        background: Some(Background::Color(Color::from_rgb(0.15, 0.15, 0.2))),
                        ..Default::default()
                    }
                } else {
                    // Unselected: subtle border
                    container::Style {
                        border: Border {
                            color: Color::from_rgb(0.3, 0.3, 0.3),
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        // background: Some(Background::Color(Color::from_rgb(0.1, 0.1, 0.1))),
                        ..Default::default()
                    }
                }
            });
        
        // Wrap in button for clicking
        let thumbnail_button = button(thumbnail_container)
            .on_press(Message::ImageSelected(img.id))
            .style(|theme: &Theme, status| {
                button::Style {
                    background: None,
                    border: Border::default(),
                    ..button::primary(theme, status)
                }
            })
            .into(); // Convert to Element
        
        thumbnails.push(thumbnail_button);
    }

    // Create row from thumbnails
    let film_row = row(thumbnails).spacing(0).padding(5);
    // Make it scrollable horizontally
    let scrollable_film = scrollable(film_row)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new()
                .width(8)
                .scroller_width(8)
        ));
    
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
