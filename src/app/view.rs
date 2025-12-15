use iced::{Alignment, Background, Border, Color, Element, Font, Length, Point, Theme};
use iced::widget::{button, checkbox, column, container, row, scrollable, slider, stack, text, Image};
use iced::widget::image::Handle;
use iced::font::Weight;
use iced_aw::Wrap;

use crate::ui;
use crate::app::message::{Message, AppTab};
use crate::app::state::{RawEditor, EditorStatus};
use crate::state::data::Image as ImageData;

// Phase 69: Brand Identity
const LOGO_BYTES: &[u8] = include_bytes!("../../assets/logo.png");

// Phase 57: Embedded font for icons and typography
pub const ICON_FONT: Font = Font::with_name("JetBrainsMono Nerd Font");
const ICON_FONT_BYTES: &[u8] = include_bytes!("../../assets/fonts/icons.ttf");

/// Build the user interface
pub fn view(editor: &RawEditor) -> Element<'_, Message> {
    // Phase 23: Show splash screen if database is still loading
    let content = match &editor.library {
        None => view_splash(editor),
        Some(_) => view_main(editor),
    };
    
    // Phase 84: Generic Modal System
    let modal = match editor.active_modal {
        crate::app::state::Modal::None => text("").height(0).width(0).into(),
        crate::app::state::Modal::Help => modal_overlay(view_help_modal()),
        crate::app::state::Modal::Preferences => text("Preferences").into(), // Placeholder
    };
    
    stack![
        content,
        modal
    ].into()
}

/// Phase 23: Splash screen shown during database loading
fn view_splash(editor: &RawEditor) -> Element<'_, Message> {
    use iced::widget::Space;
    
    let left_content = column![
        Space::with_height(Length::Fill),
        iced::widget::image("assets/splash.png")
            .width(Length::Fill)
            .content_fit(iced::ContentFit::Cover),
        Space::with_height(Length::Fill),
    ]
    .align_x(iced::Alignment::Center);
    
    let left_panel = container(left_content)
    .width(Length::FillPortion(7))
    .height(Length::Fill)
    .style(|_theme| {
        container::Style {
            background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.10))),
            ..Default::default()
        }
    });
    
    let right_panel = container(
        column![
            Space::with_height(Length::Fill),
            text("RAW Editor").size(56).center().style(|_theme| text::Style { color: Some(Color::from_rgb(0.9, 0.9, 0.9)) }),
            Space::with_height(10.0),
            text("Professional RAW Photo Editor").size(14).center().style(|_theme| text::Style { color: Some(Color::from_rgb(0.6, 0.6, 0.6)) }),
            Space::with_height(40.0),
            text(&editor.status).size(16).center().style(|_theme| text::Style { color: Some(Color::from_rgb(0.8, 0.8, 0.8)) }),
            Space::with_height(15.0),
            text(ui::icons::HOURGLASS).size(32).font(ICON_FONT).center().style(|_theme| text::Style { color: Some(Color::from_rgb(0.5, 0.7, 1.0)) }),
            Space::with_height(Length::Fill),
            text("Version 0.4").size(11).center().style(|_theme| text::Style { color: Some(Color::from_rgb(0.4, 0.4, 0.4)) }),
            Space::with_height(10.0),
        ]
        .align_x(iced::Alignment::Center)
    )
    .width(Length::FillPortion(3))
    .height(Length::Fill)
    .style(|_theme| {
        container::Style {
            background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.10))),
            ..Default::default()
        }
    });
    
    row![left_panel, right_panel].width(Length::Fill).height(Length::Fill).into()
}

/// Phase 23: Main application UI (shown after database loads)
fn view_main(editor: &RawEditor) -> Element<'_, Message> {
    let title_bar = view_title_bar(editor);

    let content = match editor.current_tab {
        AppTab::Library => view_library(editor),
        AppTab::Cull => view_cull(editor),
        AppTab::Develop => view_develop(editor),
    };
    
    column![title_bar, content].into()
}

// Phase 74: The Cull Interface (Fast review mode)
fn view_cull(editor: &RawEditor) -> Element<'_, Message> {
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

/// Build the custom window title bar
fn view_title_bar(editor: &RawEditor) -> Element<'_, Message> {
    let menus = row![
        button(container(text("File").size(13)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).style(ui::styles::WindowControlButton::style).height(Length::Fill).padding([0, 10]),
        button(container(text("Edit").size(13)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).style(ui::styles::WindowControlButton::style).height(Length::Fill).padding([0, 10]),
        button(container(text("Window").size(13)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).style(ui::styles::WindowControlButton::style).height(Length::Fill).padding([0, 10]),
        button(container(text("Help").size(13)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).style(ui::styles::WindowControlButton::style).height(Length::Fill).padding([0, 10]).on_press(Message::OpenModal(crate::app::state::Modal::Help)),
    ].spacing(0).align_y(Alignment::Center);

    let navigation = container(
        row![
            button(container(row![text(ui::icons::FOLDER).font(ICON_FONT).size(14), text(" Library").size(14)].spacing(5).align_y(Alignment::Center)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .height(Length::Fill).padding([0, 15])
                .style(|t, s| ui::styles::TabButton { is_active: editor.current_tab == AppTab::Library }.style(t, s))
                .on_press(Message::TabChanged(AppTab::Library)),
            
            button(container(row![text(ui::icons::CHECK).font(ICON_FONT).size(14), text(" Cull").size(14)].spacing(5).align_y(Alignment::Center)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .height(Length::Fill).padding([0, 15])
                .style(|t, s| ui::styles::TabButton { is_active: editor.current_tab == AppTab::Cull }.style(t, s))
                .on_press(Message::TabChanged(AppTab::Cull)),

            button(container(row![text(ui::icons::PAINTBRUSH).font(ICON_FONT).size(14), text(" Develop").size(14)].spacing(5).align_y(Alignment::Center)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .height(Length::Fill).padding([0, 15])
                .style(|t, s| ui::styles::TabButton { is_active: editor.current_tab == AppTab::Develop }.style(t, s))
                .on_press(Message::TabChanged(AppTab::Develop)),
        ].spacing(0).height(Length::Fill)
    ).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center);

    let controls = row![
        container(Image::new(Handle::from_bytes(LOGO_BYTES.to_vec())).height(Length::Fixed(24.0)).content_fit(iced::ContentFit::Contain)).height(Length::Fill).align_y(iced::alignment::Vertical::Center).padding([8,15]),
        iced::widget::Space::with_width(Length::Fixed(5.0)),
        button(container(text(ui::icons::MINIMIZE).font(ICON_FONT).size(14)).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).on_press(Message::MinimizeWindow).style(ui::styles::WindowControlButton::style).width(Length::Fixed(45.0)).height(Length::Fill),
        button(container(text(ui::icons::MAXIMIZE).font(ICON_FONT).size(14)).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).on_press(Message::MaximizeWindow).style(ui::styles::WindowControlButton::style).width(Length::Fixed(45.0)).height(Length::Fill),
        button(container(text(ui::icons::CLOSE).font(ICON_FONT).size(14)).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).on_press(Message::CloseWindow)
            .style(|_theme, status| if status == button::Status::Hovered { button::Style { background: Some(Background::Color(Color::from_rgb(0.9, 0.2, 0.2))), text_color: Color::WHITE, ..button::Style::default() } } else { button::Style { text_color: Color::from_rgb(0.7, 0.7, 0.7), ..button::text(_theme, status) } })
            .width(Length::Fixed(45.0)).height(Length::Fill),
    ].spacing(0).height(Length::Fill).align_y(Alignment::Center);

    container(
        stack![
            row![menus, iced::widget::mouse_area(container(iced::widget::Space::with_width(Length::Fill))).on_press(Message::DragWindow), controls].height(Length::Fill).align_y(Alignment::Center).padding(0),
            navigation,
        ]
    ).height(Length::Fixed(35.0)).style(|_theme| container::Style { background: Some(Background::Color(Color::from_rgb(0.05, 0.05, 0.05))), ..Default::default() }).into()
}

/// Build the Library tab view (grid of thumbnails)
fn view_library(editor: &RawEditor) -> Element<'_, Message> {
    let filtered_images: Vec<&ImageData> = editor.images.iter()
        .filter(|img| editor.min_filter_rating == 0 || img.rating >= editor.min_filter_rating)
        .collect();
    
    let cached_count = filtered_images.iter().filter(|img| img.cache_path_thumb.is_some()).count();
    let deleted_count = filtered_images.iter().filter(|img| img.file_status == "deleted").count();
    let total_count = editor.images.len();
    let filtered_count = filtered_images.len();
    
    let filter_bar = row![
        text("Filter: ").size(14),
        button(text("All").size(12)).on_press(Message::SetMinRating(0)).padding(5).style(if editor.min_filter_rating == 0 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { |_theme: &Theme, _status| button::Style { text_color: Color::WHITE, ..Default::default() } }),
        button(row![text(ui::icons::STAR).font(ICON_FONT).size(12), text(" 1+").size(12)]).on_press(Message::SetMinRating(1)).padding(5).style(if editor.min_filter_rating == 1 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { |_theme: &Theme, _status| button::Style { text_color: Color::WHITE, ..Default::default() } }),
        button(row![text(format!("{} {}", ui::icons::STAR, ui::icons::STAR)).font(ICON_FONT).size(12), text(" 2+").size(12)]).on_press(Message::SetMinRating(2)).padding(5).style(if editor.min_filter_rating == 2 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { |_theme: &Theme, _status| button::Style { text_color: Color::WHITE, ..Default::default() } }),
        button(row![text(format!("{} {} {}", ui::icons::STAR, ui::icons::STAR, ui::icons::STAR)).font(ICON_FONT).size(12), text(" 3+").size(12)]).on_press(Message::SetMinRating(3)).padding(5).style(if editor.min_filter_rating == 3 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { |_theme: &Theme, _status| button::Style { text_color: Color::WHITE, ..Default::default() } }),
        button(row![text(format!("{} {} {} {}", ui::icons::STAR, ui::icons::STAR, ui::icons::STAR, ui::icons::STAR)).font(ICON_FONT).size(12), text(" 4+").size(12)]).on_press(Message::SetMinRating(4)).padding(5).style(if editor.min_filter_rating == 4 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { |_theme: &Theme, _status| button::Style { text_color: Color::WHITE, ..Default::default() } }),
        button(row![text(format!("{} {} {} {} {}", ui::icons::STAR, ui::icons::STAR, ui::icons::STAR, ui::icons::STAR, ui::icons::STAR)).font(ICON_FONT).size(12), text(" 5").size(12)]).on_press(Message::SetMinRating(5)).padding(5).style(if editor.min_filter_rating == 5 { |_theme: &Theme, _status| button::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))), text_color: Color::WHITE, ..Default::default() } } else { |_theme: &Theme, _status| button::Style { text_color: Color::WHITE, ..Default::default() } }),
        // Phase 83: Auto-Advance Toggle
        iced::widget::checkbox("Auto-Advance", editor.auto_advance).on_toggle(|_| Message::ToggleAutoAdvance).size(16).spacing(5),
    ].spacing(5).padding(5);
    
    let grid_header = column![
        text("RAW Editor v0.3 - Culling features").size(24),
        button("Import Folder").on_press(Message::ImportFolder).padding(8),
        text(&editor.status).size(12),
        text(format!("Showing: {}/{}  |  Thumbnails: {}/{}  |  Deleted: {}", filtered_count, total_count, cached_count, filtered_count, deleted_count)).size(11),
        filter_bar,
    ].spacing(10).padding(10);
    
    const THUMB_SIZE: u16 = 200;
    
    let thumbnail_grid = filtered_images.iter().fold(
        Wrap::new().spacing(8.0).line_spacing(8.0),
        |wrap, img| {
            let is_deleted = img.file_status == "deleted";
            let thumbnail_content = if is_deleted {
                container(column![text(ui::icons::TIMES).size(24).font(ICON_FONT), text(&img.filename).size(8), text("(deleted)").size(7)].align_x(Alignment::Center).spacing(4))
                    .width(THUMB_SIZE).height(THUMB_SIZE).center_x(iced::Length::Fixed(200.0)).center_y(iced::Length::Fixed(150.0))
                    .style(|_theme| container::Style { background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))), border: Border { color: Color::from_rgb(0.5, 0.2, 0.2), width: 2.0, radius: 4.0.into() }, ..Default::default() })
            } else if let Some(ref thumb_path) = img.cache_path_thumb {
                let handle = Handle::from_path(thumb_path.clone());
                container(Image::new(handle).content_fit(iced::ContentFit::Contain))
                    .width(THUMB_SIZE).height(THUMB_SIZE).center_x(iced::Length::Fixed(200.0)).center_y(iced::Length::Fixed(150.0))
                    .style(|_theme| container::Style { background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.25))), border: Border { color: Color::from_rgb(0.4, 0.4, 0.4), width: 1.0, radius: 4.0.into() }, ..Default::default() })
            } else {
                container(text(ui::icons::HOURGLASS).size(48).font(ICON_FONT))
                    .width(THUMB_SIZE).height(THUMB_SIZE).center_x(iced::Length::Fixed(200.0)).center_y(iced::Length::Fixed(150.0))
                    .style(|_theme| container::Style { background: Some(Background::Color(Color::from_rgb(0.2, 0.2, 0.2))), border: Border { color: Color::from_rgb(0.3, 0.3, 0.3), width: 1.0, radius: 4.0.into() }, ..Default::default() })
            };
            
            wrap.push(button(thumbnail_content).on_press(Message::ImageSelected(img.id)).padding(0).style(|theme, status| button::Style { background: None, border: Border::default(), ..button::primary(theme, status) }))
        },
    );
    
    container(column![grid_header, scrollable(thumbnail_grid).height(Length::Fill).width(Length::Fill)]).width(Length::Fill).height(Length::Fill).into()
}

/// Build the Develop tab view (full-screen editor with preview)
fn view_develop(editor: &RawEditor) -> Element<'_, Message> {
    let histogram_toggle = checkbox("Show Histogram", editor.histogram_enabled).on_toggle(Message::HistogramToggled);
    
    let histogram_section = if editor.histogram_enabled {
        let histogram_widget = iced::widget::canvas::Canvas::new(crate::ui::histogram::Histogram { data: editor.histogram_data.borrow().clone() }).width(iced::Length::Fill).height(iced::Length::Fixed(120.0));
        Some(container(histogram_widget).padding(5).style(|_theme| iced::widget::container::Style { background: Some(iced::Background::Color(iced::Color::from_rgb(0.1, 0.1, 0.1))), border: iced::Border { color: iced::Color::from_rgb(0.3, 0.3, 0.3), width: 1.0, radius: 4.0.into() }, ..Default::default() }))
    } else { None };
    
    let mut sidebar = column![text("Edit Controls").size(16), histogram_toggle];
    if let Some(hist) = histogram_section { sidebar = sidebar.push(hist); }
    
    let sidebar = sidebar
        .push(text("Tone").size(14))
        .push(slider_row("Exposure", editor.current_edit_params.exposure, -5.0..=5.0, 0.1, Message::ExposureChanged))
        .push(slider_row("Contrast", editor.current_edit_params.contrast, -10.0..=10.0, 0.005, Message::ContrastChanged))
        .push(slider_row("Highlights", editor.current_edit_params.highlights, -1.0..=1.0, 0.01, Message::HighlightsChanged))
        .push(slider_row("Shadows", editor.current_edit_params.shadows, -1.0..=1.0, 0.01, Message::ShadowsChanged))
        .push(slider_row("Whites", editor.current_edit_params.whites, 0.8..=1.2, 0.01, Message::WhitesChanged))
        .push(slider_row("Blacks", editor.current_edit_params.blacks, 0.0..=0.2, 0.005, Message::BlacksChanged))
        .push(text("Color").size(14))
        .push(slider_row("Temp", editor.current_edit_params.temperature, -1.0..=1.0, 0.01, Message::TemperatureChanged))
        .push(slider_row("Tint", editor.current_edit_params.tint, -1.0..=1.0, 0.01, Message::TintChanged))
        .push(slider_row("Vibrance", editor.current_edit_params.vibrance, -1.0..=1.0, 0.01, Message::VibranceChanged))
        .push(slider_row("Saturation", editor.current_edit_params.saturation, -100.0..=100.0, 1.0, Message::SaturationChanged))
        .push(text("Detail").size(14))
        .push(slider_row("Denoise", editor.current_edit_params.noise_reduction, 0.0..=2.0, 0.01, Message::NoiseReductionChanged))
        .push(slider_row("Sharpen", editor.current_edit_params.sharpening, 0.0..=2.0, 0.01, Message::SharpeningChanged))
        .push(slider_row("Masking", editor.current_edit_params.sharpen_masking, 0.0..=1.0, 0.01, Message::SharpenMaskingChanged))
        .push(text("Geometry").size(14))
        .push(slider_row("Rotate", editor.current_edit_params.rotation, -45.0..=45.0, 0.1, Message::RotationChanged))
        .push(row![
            button(text(ui::icons::COPY).font(ICON_FONT).size(16)).style(ui::styles::NeutralButton::style).on_press(Message::CopySettings).padding(8),
            button(text(ui::icons::PASTE).font(ICON_FONT).size(16)).style(ui::styles::NeutralButton::style).on_press_maybe(editor.edit_clipboard.as_ref().map(|_| Message::PasteSettings)).padding(8),
            iced::widget::Space::with_width(Length::Fill),
            button(row![text(ui::icons::RESET).font(ICON_FONT).size(14), text("Reset").size(14)].spacing(5)).style(ui::styles::NeutralButton::style).on_press(Message::ResetEdits).padding([8, 12]),
        ].spacing(10).width(Length::Fill))
        .push(text("Crop").size(14).font(Font { weight: Weight::Bold, ..Default::default() }))
        .push(button(row![text(if editor.is_cropping { "Done" } else { "Crop Tool" }).size(14), text(ui::icons::CROP).font(ICON_FONT).size(14)].spacing(5).align_y(Alignment::Center)).style(if editor.is_cropping { ui::styles::AccentButton::style } else { ui::styles::NeutralButton::style }).on_press(Message::ToggleCrop).width(Length::Fill))
        .push(row![
            button(text("Reset").size(12)).style(ui::styles::NeutralButton::style).on_press(Message::SetCrop([0.0, 0.0, 1.0, 1.0])),
            button(text("1:1").size(12)).style(ui::styles::NeutralButton::style).on_press_maybe(if let EditorStatus::Ready(pipeline) = &editor.editor_status { Some(Message::SetCrop(RawEditor::calculate_center_crop(1.0, pipeline.width, pipeline.height))) } else { None }),
            button(text("16:9").size(12)).style(ui::styles::NeutralButton::style).on_press_maybe(if let EditorStatus::Ready(pipeline) = &editor.editor_status { Some(Message::SetCrop(RawEditor::calculate_center_crop(16.0/9.0, pipeline.width, pipeline.height))) } else { None }),
            button(text("2:3").size(12)).style(ui::styles::NeutralButton::style).on_press_maybe(if let EditorStatus::Ready(pipeline) = &editor.editor_status { Some(Message::SetCrop(RawEditor::calculate_center_crop(2.0/3.0, pipeline.width, pipeline.height))) } else { None }),
        ].spacing(5));

    let mut sidebar = sidebar;
    if crate::debug::SHOW_SENSOR_CORRECTION {
        sidebar = sidebar.push(text("Sensor Correction").size(14))
            .push(checkbox("Shift Grid X", editor.current_edit_params.black_phase_x != 0).on_toggle(|checked| Message::BlackPhaseChanged(false, if checked { 1 } else { 0 })))
            .push(checkbox("Shift Grid Y", editor.current_edit_params.black_phase_y != 0).on_toggle(|checked| Message::BlackPhaseChanged(true, if checked { 1 } else { 0 })))
            .push(text(format!("Black TL (Red): {:.1}", editor.current_edit_params.black_offsets[0])).size(12)).push(slider(-50.0..=50.0, editor.current_edit_params.black_offsets[0], |v| Message::BlackOffsetChanged(0, v)).step(0.1))
            .push(text(format!("Black TR (Green): {:.1}", editor.current_edit_params.black_offsets[1])).size(12)).push(slider(-50.0..=50.0, editor.current_edit_params.black_offsets[1], |v| Message::BlackOffsetChanged(1, v)).step(0.1))
            .push(text(format!("Black BL (Green): {:.1}", editor.current_edit_params.black_offsets[2])).size(12)).push(slider(-50.0..=50.0, editor.current_edit_params.black_offsets[2], |v| Message::BlackOffsetChanged(2, v)).step(0.1))
            .push(text(format!("Black BR (Blue): {:.1}", editor.current_edit_params.black_offsets[3])).size(12)).push(slider(-50.0..=50.0, editor.current_edit_params.black_offsets[3], |v| Message::BlackOffsetChanged(3, v)).step(0.1));
    }

    let sidebar = sidebar.push(button(row![text(ui::icons::SAVE).font(ICON_FONT).size(14), text(" Export Image").size(14)].spacing(5).align_y(Alignment::Center)).style(ui::styles::AccentButton::style).on_press(Message::ExportImage).padding(12).width(Length::Fill)).spacing(10).padding(15);
    let sidebar_container = container(scrollable(sidebar).width(Length::Fixed(300.0)).height(Length::Fill)).style(|_theme| container::Style { background: Some(Background::Color(Color::from_rgb(0.15, 0.15, 0.15))), ..Default::default() });

    let (image_handle, overlay_content) = match &editor.editor_status {
        EditorStatus::NoSelection => (None, Option::<Element<Message>>::None),
        EditorStatus::Loading(_) => {
            let handle = editor.working_preview.clone();
            let overlay = container(column![row![text(ui::icons::HOURGLASS).font(ICON_FONT).size(14).style(|_| text::Style { color: Some(Color::WHITE) }), text(" Loading RAW...").size(14).style(|_| text::Style { color: Some(Color::WHITE) })]].padding(8)).style(|_theme| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))), border: Border { radius: 4.0.into(), ..Default::default() }, ..Default::default() }).padding(20).align_x(iced::Alignment::End).align_y(iced::Alignment::End);
            (handle, Some(overlay.into()))
        }
        EditorStatus::Ready(pipeline) => {
            let mut params_to_render = if editor.show_before { crate::state::edit::EditParams::default() } else { editor.current_edit_params.clone() };
            if editor.is_cropping { params_to_render.crop = [0.0, 0.0, 1.0, 1.0]; pipeline.update_uniforms_with_zoom(&params_to_render, 1.0, 0.0, 0.0); } else { pipeline.update_uniforms_with_zoom(&params_to_render, editor.zoom, editor.pan_offset.x, editor.pan_offset.y); }
            
            let crop = params_to_render.crop;
            let original_aspect = pipeline.width as f32 / pipeline.height as f32;
            let crop_aspect = original_aspect * (crop[2] / crop[3]);
            const MAX_PREVIEW_SIZE: u32 = 1280;
            let (target_w, target_h) = if crop_aspect > 1.0 { let w = MAX_PREVIEW_SIZE; (w, (w as f32 / crop_aspect) as u32) } else { let h = MAX_PREVIEW_SIZE; ((h as f32 * crop_aspect) as u32, h) };
            
            let rgba_bytes = pipeline.render_to_bytes(target_w, target_h);
            if editor.histogram_enabled {
                let histogram_bytes = pipeline.render_to_histogram_bytes();
                *editor.histogram_data.borrow_mut() = pipeline.calculate_histogram(&histogram_bytes);
                // editor.histogram_cache.clear(); // Already cleared in update
            }
            (Some(iced::widget::image::Handle::from_rgba(target_w, target_h, rgba_bytes)), None)
        }
        EditorStatus::Failed(_, error) => {
            let overlay = container(column![row![text(ui::icons::TIMES).font(ICON_FONT).size(24), text(" Preview Failed").size(24)], text("").size(20), text(error.clone()).size(14).style(|theme: &Theme| text::Style { color: Some(theme.palette().danger) })].padding(40).align_x(Alignment::Center));
            (None, Some(overlay.into()))
        }
    };

    let main_content: Element<Message> = if let EditorStatus::NoSelection = editor.editor_status {
        container(column![text("No Image Selected").size(32), text("").size(20), text("← Switch to Library tab to select an image").size(18).style(|theme: &Theme| text::Style { color: Some(theme.palette().text.scale_alpha(0.6)) })].padding(40).align_x(Alignment::Center)).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill).into()
    } else {
        let image_widget: Element<Message> = if let Some(handle) = image_handle {
            match &editor.editor_status {
                EditorStatus::Loading(_) => {
                    use iced::widget::canvas::Canvas;
                    use crate::ui::preview_renderer::PreviewRenderer;
                    Canvas::new(PreviewRenderer { handle, zoom: editor.zoom, offset: editor.pan_offset, is_cropping: false, crop: [0.0, 0.0, 1.0, 1.0], image_width: 3, image_height: 2, draw_image: true }).width(Length::Fill).height(Length::Fill).into()
                }
                EditorStatus::Ready(pipeline) => {
                    if editor.is_cropping {
                        use iced::widget::canvas::Canvas;
                        use crate::ui::preview_renderer::PreviewRenderer;
                        stack![
                            Canvas::new(PreviewRenderer { handle: handle.clone(), zoom: editor.zoom, offset: editor.pan_offset, is_cropping: false, crop: editor.current_edit_params.crop, image_width: pipeline.width, image_height: pipeline.height, draw_image: true }).width(Length::Fill).height(Length::Fill),
                            Canvas::new(PreviewRenderer { handle: handle.clone(), zoom: editor.zoom, offset: editor.pan_offset, is_cropping: true, crop: editor.current_edit_params.crop, image_width: pipeline.width, image_height: pipeline.height, draw_image: false }).width(Length::Fill).height(Length::Fill)
                        ].width(Length::Fill).height(Length::Fill).into()
                    } else {
                        Image::new(handle).width(Length::Fill).height(Length::Fill).content_fit(iced::ContentFit::Contain).into()
                    }
                }
                _ => Image::new(handle).width(Length::Fill).height(Length::Fill).content_fit(iced::ContentFit::Contain).into()
            }
        } else { iced::widget::Space::new(Length::Fill, Length::Fill).into() };

        let overlay = match editor.info_overlay {
            crate::app::state::InfoOverlayState::Hidden => container(column![]),
            crate::app::state::InfoOverlayState::Metadata | crate::app::state::InfoOverlayState::CacheDebug => {
                container(column![
                    text(format!("{} {}", editor.current_metadata.as_ref().map(|m| m.make.clone()).unwrap_or_default(), editor.current_metadata.as_ref().map(|m| m.model.clone()).unwrap_or_default())).size(12).style(|_| text::Style { color: Some(Color::WHITE) }),
                    text(editor.current_metadata.as_ref().map(|m| m.lens.clone()).unwrap_or_default()).size(12).style(|_| text::Style { color: Some(Color::WHITE) }),
                    text(format!("ISO {}  {}  f/{}", editor.current_metadata.as_ref().map(|m| m.iso.to_string()).unwrap_or("---".to_string()), editor.current_metadata.as_ref().map(|m| m.shutter_speed.clone()).unwrap_or("---".to_string()), editor.current_metadata.as_ref().map(|m| m.aperture.to_string()).unwrap_or("---".to_string()))).size(12).style(|_| text::Style { color: Some(Color::WHITE) }),
                ].spacing(2)).padding(10).style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))), border: iced::border::Border { radius: 5.0.into(), ..Default::default() }, ..Default::default() })
            }
        };

        let filename_overlay = container(text(editor.selected_image_id.and_then(|id| editor.images.iter().find(|img| img.id == id)).map(|img| img.filename.clone()).unwrap_or_default()).size(12).style(|_| text::Style { color: Some(Color::WHITE) })).padding(5).style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.4))), border: iced::border::Border { radius: 3.0.into(), ..Default::default() }, ..Default::default() });

        let stacked_image = stack![
            image_widget,
            container(overlay).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Left).align_y(iced::alignment::Vertical::Top).padding(10),
            container(filename_overlay).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Left).align_y(iced::alignment::Vertical::Bottom).padding(10),
        ];

        use iced::widget::mouse_area;
        use iced::mouse::ScrollDelta;
        let interactive_image = mouse_area(stacked_image)
            .on_scroll(|delta| { let zoom_delta = match delta { ScrollDelta::Lines { y, .. } => y * 0.1, ScrollDelta::Pixels { y, .. } => y * 0.01 }; Message::Zoom(zoom_delta, Point::new(-1.0, -1.0)) })
            .on_press(Message::MousePressed).on_release(Message::MouseReleased).on_move(|position| Message::MouseMoved(position));
            
        let content_stack = if let Some(overlay) = overlay_content { stack![interactive_image, overlay] } else { stack![interactive_image] };
        container(content_stack).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill).clip(true).style(|_| container::Style { background: Some(Background::Color(Color::BLACK)), ..Default::default() }).into()
    };

    if matches!(editor.editor_status, EditorStatus::NoSelection) { return main_content; }
    
    let editor_content = column![row![main_content, sidebar_container].spacing(0).height(Length::Fill)].width(Length::Fill).height(Length::Fill);
    let filtered_images: Vec<&ImageData> = editor.images.iter().filter(|img| editor.min_filter_rating == 0 || img.rating >= editor.min_filter_rating).collect();
    let filmstrip = ui::filmstrip::view(&filtered_images, &editor.multi_selection);
    column![editor_content, Container::new(filmstrip).width(Length::Fill).height(Length::Fixed(115.0))].width(Length::Fill).height(Length::Fill).into()
}

fn slider_row<'a, F>(label: &'a str, value: f32, range: std::ops::RangeInclusive<f32>, step: f32, on_change: F) -> Element<'a, Message> where F: Fn(f32) -> Message + 'a {
    row![
        text(label).width(Length::Fixed(90.0)).size(13).style(|_theme| text::Style { color: Some(Color::from_rgb(0.7, 0.7, 0.7)) }),
        slider(range, value, on_change).step(step).width(Length::Fill).style(crate::ui::styles::ProSlider::style).on_release(Message::CommitEdit),
        text(format!("{:.2}", value)).width(Length::Fixed(40.0)).size(13).align_x(iced::alignment::Horizontal::Right).style(|_theme| text::Style { color: Some(Color::from_rgb(0.7, 0.7, 0.7)) }),
    ].spacing(10).align_y(iced::Alignment::Center).into()
}

use iced::widget::Container;

/// Set the application theme
pub fn theme(_: &RawEditor) -> Theme {
    Theme::Dark
}

fn modal_overlay<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    use iced::widget::{container, mouse_area};
    
    // The backdrop: semi-transparent black, fills screen, closes on click
    let backdrop = mouse_area(
        container(text(" "))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
                ..Default::default()
            })
    )
    .on_press(Message::CloseModal);

    // The card: centered content
    let card = container(content)
        .padding(20)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.15, 0.15, 0.17))),
            border: Border {
                radius: 10.0.into(),
                width: 1.0,
                color: Color::from_rgb(0.3, 0.3, 0.3),
            },
            ..Default::default()
        });
    
    stack![
        backdrop,
        container(
            // Swallow clicks on the card so they don't reach the backdrop
            mouse_area(card).on_press(Message::ModalNoOp)
        ).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill)
    ].into()
}

fn view_help_modal<'a>() -> Element<'a, Message> {
    let shortcut = |key: &str, desc: &str| {
        row![
            text(key.to_string()).font(Font { weight: Weight::Bold, ..Default::default() }).width(Length::Fixed(120.0)).style(|_| text::Style { color: Some(Color::from_rgb(0.8, 0.8, 1.0)) }),
            text(desc.to_string()).size(14)
        ].spacing(10)
    };
    
    column![
        text("Keyboard Shortcuts").size(24).font(Font { weight: Weight::Bold, ..Default::default() }),
        iced::widget::horizontal_rule(1.0),
        column![
            text("Navigation").size(16).style(|_| text::Style{color: Some(Color::from_rgb(0.6, 0.6, 0.6))}),
            shortcut("Arrow Keys", "Previous / Next Image"),
            shortcut("Space", "Toggle Before/After View"),
            
            text("Rating & Culling").size(16).style(|_| text::Style{color: Some(Color::from_rgb(0.6, 0.6, 0.6))}),
            shortcut("0 - 5", "Set Star Rating"),
            shortcut("P", "Pick (Flag)"),
            shortcut("X", "Reject (X)"),
            shortcut("U", "Unflag"),
            
            text("Editing").size(16).style(|_| text::Style{color: Some(Color::from_rgb(0.6, 0.6, 0.6))}),
            shortcut("Ctrl + Z", "Undo"),
            shortcut("Ctrl + Shift + Z", "Redo"),
            shortcut("Ctrl + C / V", "Copy / Paste Settings"),
        ].spacing(10),
        
        button("Close").on_press(Message::CloseModal).padding(10).width(Length::Fill).style(ui::styles::NeutralButton::style)
    ]
    .spacing(20)
    .width(400)
    .into()
}
