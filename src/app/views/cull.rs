use iced::{Alignment, Background, Color, Element, Length, Theme};
use iced::widget::{column, container, stack, text, Image, Container};
use iced::widget::image::Handle;

use crate::ui;
use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::database::models::Image as ImageData;

// Phase 74: The Cull Interface (Fast review mode)
pub fn view_cull(editor: &RawEditor) -> Element<'_, Message> {
    let main_content: Element<Message> = if let Some(id) = editor.selected_image_id {
        // Use working preview (1280px) if available (from cache or disk)
        // We prioritize speed over quality here.
        let image_handle = editor.working_preview.clone()
            .or_else(|| editor.images.iter().find(|i| i.id == id).and_then(|i| i.cache_path_working.as_ref()).map(|p| Handle::from_path(p.clone())));
            
        if let Some(handle) = image_handle {
            let image_widget = Image::new(handle).width(Length::Fill).height(Length::Fill).content_fit(iced::ContentFit::Contain);
            
            // HUD Overlay (ISO, Shutter, etc.)
            let overlay = match editor.info_overlay {
                crate::app::state::InfoOverlayState::Hidden => container(column![]),
                crate::app::state::InfoOverlayState::Metadata => {
                    container(column![
                        text(format!("{} {}", editor.current_metadata.as_ref().map(|m| m.make.clone()).unwrap_or_default(), editor.current_metadata.as_ref().map(|m| m.model.clone()).unwrap_or_default())).size(12).style(|_| text::Style { color: Some(Color::WHITE) }),
                        text(editor.current_metadata.as_ref().map(|m| m.lens.clone()).unwrap_or_default()).size(12).style(|_| text::Style { color: Some(Color::WHITE) }),
                        text(format!("ISO {}  {}  f/{}", editor.current_metadata.as_ref().map(|m| m.iso.to_string()).unwrap_or("---".to_string()), editor.current_metadata.as_ref().map(|m| m.shutter_speed.clone()).unwrap_or("---".to_string()), editor.current_metadata.as_ref().map(|m| m.aperture.to_string()).unwrap_or("---".to_string()))).size(12).style(|_| text::Style { color: Some(Color::WHITE) }),
                    ].spacing(2)).padding(10).style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))), border: iced::border::Border { radius: 5.0.into(), ..Default::default() }, ..Default::default() })
                }
                crate::app::state::InfoOverlayState::CacheDebug => {
                    // Count cached images before and after current
                    let mut before = 0;
                    let mut after = 0;
                    if let Some(current_idx) = editor.images.iter().position(|i| i.id == id) {
                        let total = editor.images.len() as isize;
                        
                        // Phase 80: Configurable Cache Size
                        let behind_limit = crate::app::state::PRELOAD_BEHIND;
                        let ahead_limit = crate::app::state::PRELOAD_AHEAD;

                        // Check -BEHIND to -1
                        for i in 1..=behind_limit {
                            let mut idx = current_idx as isize - i as isize;
                            if idx < 0 { idx += total; }
                            if let Some(img) = editor.images.get(idx as usize) {
                                if editor.preview_cache.contains(&img.id) { before += 1; }
                            }
                        }
                        // Check +1 to +AHEAD
                        for i in 1..=ahead_limit {
                            let mut idx = current_idx as isize + i as isize;
                            if idx >= total { idx -= total; }
                            if let Some(img) = editor.images.get(idx as usize) {
                                if editor.preview_cache.contains(&img.id) { after += 1; }
                            }
                        }
                    }
                    
                    container(
                        text(format!("-{} | +{}", before, after))
                            .size(16)
                            .style(|_| text::Style { color: Some(Color::WHITE) })
                    ).padding(10).style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))), border: iced::border::Border { radius: 5.0.into(), ..Default::default() }, ..Default::default() })
                }
            };

            let filename_overlay = container(text(editor.images.iter().find(|img| img.id == id).map(|img| img.filename.clone()).unwrap_or_default()).size(12).style(|_| text::Style { color: Some(Color::WHITE) })).padding(5).style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.4))), border: iced::border::Border { radius: 3.0.into(), ..Default::default() }, ..Default::default() });

            let stacked_image = stack![
                image_widget,
                container(overlay).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Left).align_y(iced::alignment::Vertical::Top).padding(10),
                container(filename_overlay).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Left).align_y(iced::alignment::Vertical::Bottom).padding(10),
            ];
            
            container(stacked_image).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill).style(|_| container::Style { background: Some(Background::Color(Color::BLACK)), ..Default::default() }).into()
        } else {
            container(text("Loading preview...").size(20).style(|theme: &Theme| text::Style { color: Some(theme.palette().text.scale_alpha(0.6)) })).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill).into()
        }
    } else {
        container(column![text("No Image Selected").size(32), text("").size(20), text("← Switch to Library tab to select an image").size(18).style(|theme: &Theme| text::Style { color: Some(theme.palette().text.scale_alpha(0.6)) })].padding(40).align_x(Alignment::Center)).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill).into()
    };

    let filtered_images: Vec<&ImageData> = editor.images.iter().filter(|img| editor.min_filter_rating == 0 || img.rating >= editor.min_filter_rating).collect();
    let filmstrip = ui::filmstrip::view(&filtered_images, &editor.multi_selection);
    
    column![main_content, Container::new(filmstrip).width(Length::Fill).height(Length::Fixed(115.0))].width(Length::Fill).height(Length::Fill).into()
}
