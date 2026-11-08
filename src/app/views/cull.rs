use iced::{Background, Color, Element, Length};
use iced::widget::{column, container, stack, text, Image, Container};
use iced::widget::image::Handle;

use crate::ui;
use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::database::models::Image as ImageData;
use crate::ui::theme;

// Phase 74: The Cull Interface (Fast review mode)
pub fn view_cull(editor: &RawEditor) -> Element<'_, Message> {
    let main_content: Element<Message> = if let Some(id) = editor.selected_image_id {
        view_preview(editor, id)
    } else {
        view_empty_state()
    };

    let filtered_images: Vec<&ImageData> = editor.displayed_images();
    let filmstrip = ui::filmstrip::view(&filtered_images, editor);

    // Cull was the one tab with no export entry point, despite the export
    // handler explicitly optimising for the cull-then-export-the-selects flow
    // by reusing the already-decoded RAW cache.
    let selected = editor.multi_selection.len().max(1);
    let export_bar = container(
        iced::widget::row![
            iced::widget::Space::with_width(Length::Fill),
            crate::ui::widgets::button(
                iced::widget::row![
                    text(ui::icons::SAVE).font(crate::ui::icons::ICON_FONT).size(theme::size::SMALL),
                    text(format!("Export {selected}")).size(theme::size::SMALL)
                ]
                .spacing(theme::space::S)
                .align_y(iced::Alignment::Center)
            )
            .on_press(Message::OpenExportModal)
            .padding([6, 12])
            .style(crate::ui::styles::NeutralButton::style),
        ]
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .padding([6, 10]);
    
    column![
        main_content,
        export_bar,
        Container::new(filmstrip).width(Length::Fill).height(Length::Fixed(115.0)).clip(true)
    ].width(Length::Fill).height(Length::Fill).into()
}

fn view_preview(editor: &RawEditor, id: i64) -> Element<'_, Message> {
    // Use working preview (1280px) if available (from cache or disk)
    let image_handle = editor.working_preview.clone()
        .or_else(|| editor.images.iter().find(|i| i.id == id).and_then(|i| i.cache_path_working.as_ref()).map(|p| Handle::from_path(p.clone())));
        
    if let Some(handle) = image_handle {
        let image_widget = Image::new(handle).width(Length::Fill).height(Length::Fill).content_fit(iced::ContentFit::Contain);
        
        let overlay = view_overlay(editor, id);
        let filename_overlay = view_filename_overlay(editor, id);

        let stacked_image = stack![
            image_widget,
            container(overlay).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Left).align_y(iced::alignment::Vertical::Top).padding(10),
            container(filename_overlay).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Left).align_y(iced::alignment::Vertical::Bottom).padding(10),
        ];
        
        container(stacked_image).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill).style(|_| container::Style { background: Some(Background::Color(Color::BLACK)), ..Default::default() }).into()
    } else {
        container(ui::text::tinted("Loading preview…", theme::size::LABEL, theme::colours().text_muted)).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill).into()
    }
}

fn view_overlay(editor: &RawEditor, id: i64) -> Element<'_, Message> {
    match editor.info_overlay {
        crate::app::state::InfoOverlayState::Hidden => container(column![]).into(),
        crate::app::state::InfoOverlayState::Metadata => {
            let lines = super::develop::metadata_lines(editor)
                .into_iter()
                .fold(column![].spacing(theme::space::XXS), |col, line| {
                    col.push(text(line).size(theme::size::SMALL).style(|_| text::Style { color: Some(theme::colours().on_photo) }))
                });
            container(lines).padding(10).style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))), border: iced::border::Border { radius: theme::radius::CONTROL.into(), ..Default::default() }, ..Default::default() }).into()
        }
        crate::app::state::InfoOverlayState::CacheDebug => {
            let (before, after) = count_cached_neighbors(editor, id);
            container(
                text(format!("-{} | +{}", before, after))
                    .size(theme::size::HEADING)
                    .style(|_| text::Style { color: Some(theme::colours().on_photo) })
            ).padding(10).style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))), border: iced::border::Border { radius: theme::radius::CONTROL.into(), ..Default::default() }, ..Default::default() }).into()
        }
    }
}

fn view_filename_overlay(editor: &RawEditor, id: i64) -> Element<'_, Message> {
    container(text(editor.images.iter().find(|img| img.id == id).map(|img| img.filename.clone()).unwrap_or_default()).size(theme::size::SMALL).style(|_| text::Style { color: Some(theme::colours().on_photo) })).padding(5).style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.4))), border: iced::border::Border { radius: theme::radius::CONTROL.into(), ..Default::default() }, ..Default::default() }).into()
}

fn view_empty_state() -> Element<'static, Message> {
    container(ui::text::empty_state("No image selected", ui::text::body("Pick a photo in Library to start culling."))).width(Length::Fill).height(Length::Fill).center_x(Length::Fill).center_y(Length::Fill).into()
}

/// Cached previews within the preload window, behind and ahead — in display
/// order, the same order the preloader and the arrow keys use.
fn count_cached_neighbors(editor: &RawEditor, id: i64) -> (usize, usize) {
    let ids = editor.displayed_ids();
    let Some(current) = ids.iter().position(|&i| i == id) else {
        return (0, 0);
    };
    let n = ids.len();
    let cached = |offset: usize, back: bool| {
        let i = if back { (current + n - offset % n) % n } else { (current + offset) % n };
        editor.preview_cache.contains(&ids[i])
    };
    let before = (1..=editor.preview_preload_behind.min(n - 1)).filter(|&o| cached(o, true)).count();
    let after = (1..=editor.preview_preload_ahead.min(n - 1)).filter(|&o| cached(o, false)).count();
    (before, after)
}
