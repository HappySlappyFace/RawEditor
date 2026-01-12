use iced::font::{Font, Weight};
use iced::widget::{
    button, checkbox, column, container, row, scrollable, slider, stack, text, Container, Image,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::message::Message;
use crate::app::state::{EditorReadiness, RawEditor};
use crate::database::models::Image as ImageData;
use crate::ui;
use crate::ui::icons::ICON_FONT;

/// Build the Develop tab view (full-screen editor with preview)
pub fn view_develop(editor: &RawEditor) -> Element<'_, Message> {
    let sidebar = view_sidebar(editor);
    let sidebar_container = container(
        scrollable(sidebar)
            .width(Length::Fixed(300.0))
            .height(Length::Fill),
    )
    .style(|_theme| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.15, 0.15, 0.15))),
        ..Default::default()
    });

    let (image_handle, overlay_content) = prepare_preview(editor);
    let main_content = view_main_content(editor, image_handle, overlay_content);

    if matches!(editor.editor_readiness, EditorReadiness::NoSelection) {
        return main_content;
    }

    let editor_content = column![row![main_content, sidebar_container]
        .spacing(0)
        .height(Length::Fill)]
    .width(Length::Fill)
    .height(Length::Fill);
    let filtered_images: Vec<&ImageData> = editor
        .images
        .iter()
        .filter(|img| editor.min_filter_rating == 0 || img.rating >= editor.min_filter_rating)
        .collect();
    let filmstrip = ui::filmstrip::view(&filtered_images, &editor.multi_selection);
    column![
        editor_content,
        Container::new(filmstrip)
            .width(Length::Fill)
            .height(Length::Fixed(115.0))
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn view_sidebar(editor: &RawEditor) -> iced::widget::Column<'_, Message> {
    let histogram_toggle =
        checkbox("Show Histogram", editor.histogram_enabled).on_toggle(Message::HistogramToggled);

    let histogram_section = if editor.histogram_enabled {
        if let Some(data) = &editor.histogram_data {
            let histogram_widget =
                iced::widget::canvas::Canvas::new(crate::ui::histogram::Histogram {
                    data: data.clone(),
                })
                .width(iced::Length::Fill)
                .height(iced::Length::Fixed(120.0));
            Some(container(histogram_widget).padding(5).style(|_theme| {
                iced::widget::container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgb(
                        0.1, 0.1, 0.1,
                    ))),
                    border: iced::Border {
                        color: iced::Color::from_rgb(0.3, 0.3, 0.3),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }
            }))
        } else {
            None
        }
    } else {
        None
    };

    let mut sidebar = column![text("Edit Controls").size(16), histogram_toggle];
    if let Some(hist) = histogram_section {
        sidebar = sidebar.push(hist);
    }

    sidebar = sidebar
        .push(text("Tone").size(14))
        .push(slider_row(
            "Exposure",
            editor.current_edit_params.exposure,
            -5.0..=5.0,
            0.1,
            Message::ExposureChanged,
        ))
        .push(slider_row(
            "Contrast",
            editor.current_edit_params.contrast,
            -10.0..=10.0,
            0.005,
            Message::ContrastChanged,
        ))
        .push(slider_row(
            "Highlights",
            editor.current_edit_params.highlights,
            -1.0..=1.0,
            0.01,
            Message::HighlightsChanged,
        ))
        .push(slider_row(
            "Shadows",
            editor.current_edit_params.shadows,
            -1.0..=1.0,
            0.01,
            Message::ShadowsChanged,
        ))
        .push(slider_row(
            "Whites",
            editor.current_edit_params.whites,
            0.8..=1.2,
            0.01,
            Message::WhitesChanged,
        ))
        .push(slider_row(
            "Blacks",
            editor.current_edit_params.blacks,
            0.0..=0.2,
            0.005,
            Message::BlacksChanged,
        ))
        .push(text("Color").size(14))
        .push(slider_row(
            "Temp",
            editor.current_edit_params.temperature,
            -1.0..=1.0,
            0.01,
            Message::TemperatureChanged,
        ))
        .push(slider_row(
            "Tint",
            editor.current_edit_params.tint,
            -1.0..=1.0,
            0.01,
            Message::TintChanged,
        ))
        .push(slider_row(
            "Vibrance",
            editor.current_edit_params.vibrance,
            -1.0..=1.0,
            0.01,
            Message::VibranceChanged,
        ))
        .push(slider_row(
            "Saturation",
            editor.current_edit_params.saturation,
            -100.0..=100.0,
            1.0,
            Message::SaturationChanged,
        ))
        .push(text("Detail").size(14))
        .push(slider_row(
            "Denoise",
            editor.current_edit_params.noise_reduction,
            0.0..=2.0,
            0.01,
            Message::NoiseReductionChanged,
        ))
        .push(slider_row(
            "Sharpen",
            editor.current_edit_params.sharpening,
            0.0..=2.0,
            0.01,
            Message::SharpeningChanged,
        ))
        .push(slider_row(
            "Masking",
            editor.current_edit_params.sharpen_masking,
            0.0..=1.0,
            0.01,
            Message::SharpenMaskingChanged,
        ))
        .push(text("Geometry").size(14))
        .push(slider_row(
            "Rotate",
            editor.current_edit_params.rotation,
            -45.0..=45.0,
            0.1,
            Message::RotationChanged,
        ))
        .push(
            row![
                button(text(ui::icons::COPY).font(ICON_FONT).size(16))
                    .style(ui::styles::NeutralButton::style)
                    .on_press(Message::CopySettings)
                    .padding(8),
                button(text(ui::icons::PASTE).font(ICON_FONT).size(16))
                    .style(ui::styles::NeutralButton::style)
                    .on_press_maybe(
                        editor
                            .edit_clipboard
                            .as_ref()
                            .map(|_| Message::PasteSettings)
                    )
                    .padding(8),
                iced::widget::Space::with_width(Length::Fill),
                button(
                    row![
                        text(ui::icons::RESET).font(ICON_FONT).size(14),
                        text("Reset").size(14)
                    ]
                    .spacing(5)
                )
                .style(ui::styles::NeutralButton::style)
                .on_press(Message::ResetEdits)
                .padding([8, 12]),
            ]
            .spacing(10)
            .width(Length::Fill),
        )
        .push(text("Crop").size(14).font(Font {
            weight: Weight::Bold,
            ..Default::default()
        }))
        .push(
            button(
                row![
                    text(if editor.is_cropping {
                        "Done"
                    } else {
                        "Crop Tool"
                    })
                    .size(14),
                    text(ui::icons::CROP).font(ICON_FONT).size(14)
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .style(if editor.is_cropping {
                ui::styles::AccentButton::style
            } else {
                ui::styles::NeutralButton::style
            })
            .on_press(Message::ToggleCrop)
            .width(Length::Fill),
        )
        .push(
            row![
                button(text("Reset").size(12))
                    .style(ui::styles::NeutralButton::style)
                    .on_press(Message::SetCrop([0.0, 0.0, 1.0, 1.0])),
                button(text("1:1").size(12))
                    .style(ui::styles::NeutralButton::style)
                    .on_press_maybe(if let Some(res) = &editor.image_resources {
                        Some(Message::SetCrop(RawEditor::calculate_center_crop(
                            1.0, res.width, res.height,
                        )))
                    } else {
                        None
                    }),
                button(text("16:9").size(12))
                    .style(ui::styles::NeutralButton::style)
                    .on_press_maybe(if let Some(res) = &editor.image_resources {
                        Some(Message::SetCrop(RawEditor::calculate_center_crop(
                            16.0 / 9.0,
                            res.width,
                            res.height,
                        )))
                    } else {
                        None
                    }),
                button(text("2:3").size(12))
                    .style(ui::styles::NeutralButton::style)
                    .on_press_maybe(if let Some(res) = &editor.image_resources {
                        Some(Message::SetCrop(RawEditor::calculate_center_crop(
                            2.0 / 3.0,
                            res.width,
                            res.height,
                        )))
                    } else {
                        None
                    }),
            ]
            .spacing(5),
        );

    if crate::debug::SHOW_SENSOR_CORRECTION {
        sidebar = sidebar
            .push(text("Sensor Correction").size(14))
            .push(
                checkbox(
                    "Shift Grid X",
                    editor.current_edit_params.black_phase_x != 0,
                )
                .on_toggle(|checked| {
                    Message::BlackPhaseChanged(false, if checked { 1 } else { 0 })
                }),
            )
            .push(
                checkbox(
                    "Shift Grid Y",
                    editor.current_edit_params.black_phase_y != 0,
                )
                .on_toggle(|checked| Message::BlackPhaseChanged(true, if checked { 1 } else { 0 })),
            )
            .push(
                text(format!(
                    "Black TL (Red): {:.1}",
                    editor.current_edit_params.black_offsets[0]
                ))
                .size(12),
            )
            .push(
                slider(
                    -50.0..=50.0,
                    editor.current_edit_params.black_offsets[0],
                    |v| Message::BlackOffsetChanged(0, v),
                )
                .step(0.1),
            )
            .push(
                text(format!(
                    "Black TR (Green): {:.1}",
                    editor.current_edit_params.black_offsets[1]
                ))
                .size(12),
            )
            .push(
                slider(
                    -50.0..=50.0,
                    editor.current_edit_params.black_offsets[1],
                    |v| Message::BlackOffsetChanged(1, v),
                )
                .step(0.1),
            )
            .push(
                text(format!(
                    "Black BL (Green): {:.1}",
                    editor.current_edit_params.black_offsets[2]
                ))
                .size(12),
            )
            .push(
                slider(
                    -50.0..=50.0,
                    editor.current_edit_params.black_offsets[2],
                    |v| Message::BlackOffsetChanged(2, v),
                )
                .step(0.1),
            )
            .push(
                text(format!(
                    "Black BR (Blue): {:.1}",
                    editor.current_edit_params.black_offsets[3]
                ))
                .size(12),
            )
            .push(
                slider(
                    -50.0..=50.0,
                    editor.current_edit_params.black_offsets[3],
                    |v| Message::BlackOffsetChanged(3, v),
                )
                .step(0.1),
            );
    }

    sidebar
        .push(
            button(
                row![
                    text(ui::icons::SAVE).font(ICON_FONT).size(14),
                    text(" Export Image").size(14)
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .style(ui::styles::AccentButton::style)
            .on_press(Message::OpenExportModal)
            .padding(12)
            .width(Length::Fill),
        )
        .spacing(10)
        .padding(15)
}

fn prepare_preview(
    editor: &RawEditor,
) -> (
    Option<iced::widget::image::Handle>,
    Option<Element<'_, Message>>,
) {
    match &editor.editor_readiness {
        EditorReadiness::NoSelection => (None, Option::<Element<Message>>::None),
        EditorReadiness::Loading(_) => {
            let handle = editor.working_preview.clone();
            let overlay = container(
                column![row![
                    text(ui::icons::HOURGLASS)
                        .font(ICON_FONT)
                        .size(14)
                        .style(|_| text::Style {
                            color: Some(Color::WHITE)
                        }),
                    text(" Loading RAW...").size(14).style(|_| text::Style {
                        color: Some(Color::WHITE)
                    })
                ]]
                .padding(8),
            )
            .style(|_theme| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
                border: Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .padding(20)
            .align_x(iced::Alignment::End)
            .align_y(iced::Alignment::End);
            (handle, Some(overlay.into()))
        }
        EditorReadiness::Ready(_) => {
            if let (Some(ctx), Some(resources)) = (&editor.gpu_context, &editor.image_resources) {
                let mut params_to_render = if editor.show_before {
                    crate::core::types::EditParams::default()
                } else {
                    editor.current_edit_params.clone()
                };
                if editor.is_cropping {
                    params_to_render.crop = [0.0, 0.0, 1.0, 1.0];
                    resources.update_uniforms_with_zoom(ctx, &params_to_render, 1.0, 0.0, 0.0);
                } else {
                    resources.update_uniforms_with_zoom(
                        ctx,
                        &params_to_render,
                        editor.zoom,
                        editor.pan_offset.x,
                        editor.pan_offset.y,
                    );
                }

                // Phase 103: Use rendered preview if available (async updated)
                // Fallback to working_preview (source) if not yet rendered
                let handle = editor
                    .rendered_preview
                    .clone()
                    .or(editor.working_preview.clone());
                (handle, None)
            } else {
                (None, None)
            }
        }
        EditorReadiness::Failed(_, error) => {
            let overlay = container(
                column![
                    row![
                        text(ui::icons::TIMES).font(ICON_FONT).size(24),
                        text(" Preview Failed").size(24)
                    ],
                    text("").size(20),
                    text(error.clone())
                        .size(14)
                        .style(|theme: &Theme| text::Style {
                            color: Some(theme.palette().danger)
                        })
                ]
                .padding(40)
                .align_x(Alignment::Center),
            );
            (None, Some(overlay.into()))
        }
    }
}

fn view_main_content<'a>(
    editor: &'a RawEditor,
    image_handle: Option<iced::widget::image::Handle>,
    overlay_content: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    if matches!(editor.editor_readiness, EditorReadiness::NoSelection) {
        return container(
            column![
                text("No Image Selected").size(32),
                text("").size(20),
                text("← Switch to Library tab to select an image")
                    .size(18)
                    .style(|theme: &Theme| text::Style {
                        color: Some(theme.palette().text.scale_alpha(0.6))
                    })
            ]
            .padding(40)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into();
    }

    let image_widget: Element<Message> = if let Some(handle) = image_handle {
        match &editor.editor_readiness {
            EditorReadiness::Loading(_) => {
                use crate::ui::preview_renderer::PreviewRenderer;
                use iced::widget::canvas::Canvas;
                Canvas::new(PreviewRenderer {
                    handle,
                    zoom: editor.zoom,
                    offset: editor.pan_offset,
                    is_cropping: false,
                    crop: [0.0, 0.0, 1.0, 1.0],
                    image_width: 3,
                    image_height: 2,
                    draw_image: true,
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
            }
            EditorReadiness::Ready(_) => {
                if let Some(resources) = &editor.image_resources {
                    if editor.is_cropping {
                        use crate::ui::preview_renderer::PreviewRenderer;
                        use iced::widget::canvas::Canvas;
                        stack![
                            Canvas::new(PreviewRenderer {
                                handle: handle.clone(),
                                zoom: editor.zoom,
                                offset: editor.pan_offset,
                                is_cropping: false,
                                crop: editor.current_edit_params.crop,
                                image_width: resources.width,
                                image_height: resources.height,
                                draw_image: true
                            })
                            .width(Length::Fill)
                            .height(Length::Fill),
                            Canvas::new(PreviewRenderer {
                                handle: handle.clone(),
                                zoom: editor.zoom,
                                offset: editor.pan_offset,
                                is_cropping: true,
                                crop: editor.current_edit_params.crop,
                                image_width: resources.width,
                                image_height: resources.height,
                                draw_image: false
                            })
                            .width(Length::Fill)
                            .height(Length::Fill)
                        ]
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                    } else {
                        Image::new(handle)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .content_fit(iced::ContentFit::Contain)
                            .into()
                    }
                } else {
                    Image::new(handle)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .content_fit(iced::ContentFit::Contain)
                        .into()
                }
            }
            _ => Image::new(handle)
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(iced::ContentFit::Contain)
                .into(),
        }
    } else {
        iced::widget::Space::new(Length::Fill, Length::Fill).into()
    };

    let overlay = match editor.info_overlay {
        crate::app::state::InfoOverlayState::Hidden => container(column![]),
        crate::app::state::InfoOverlayState::Metadata
        | crate::app::state::InfoOverlayState::CacheDebug => container(
            column![
                text(format!(
                    "{} {}",
                    editor
                        .current_metadata
                        .as_ref()
                        .map(|m| m.make.clone())
                        .unwrap_or_default(),
                    editor
                        .current_metadata
                        .as_ref()
                        .map(|m| m.model.clone())
                        .unwrap_or_default()
                ))
                .size(12)
                .style(|_| text::Style {
                    color: Some(Color::WHITE)
                }),
                text(
                    editor
                        .current_metadata
                        .as_ref()
                        .map(|m| m.lens.clone())
                        .unwrap_or_default()
                )
                .size(12)
                .style(|_| text::Style {
                    color: Some(Color::WHITE)
                }),
                text(format!(
                    "ISO {}  {}  f/{}",
                    editor
                        .current_metadata
                        .as_ref()
                        .map(|m| m.iso.to_string())
                        .unwrap_or("---".to_string()),
                    editor
                        .current_metadata
                        .as_ref()
                        .map(|m| m.shutter_speed.clone())
                        .unwrap_or("---".to_string()),
                    editor
                        .current_metadata
                        .as_ref()
                        .map(|m| m.aperture.to_string())
                        .unwrap_or("---".to_string())
                ))
                .size(12)
                .style(|_| text::Style {
                    color: Some(Color::WHITE)
                }),
            ]
            .spacing(2),
        )
        .padding(10)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))),
            border: iced::border::Border {
                radius: 5.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }),
    };

    let filename_overlay = container(
        text(
            editor
                .selected_image_id
                .and_then(|id| editor.images.iter().find(|img| img.id == id))
                .map(|img| img.filename.clone())
                .unwrap_or_default(),
        )
        .size(12)
        .style(|_| text::Style {
            color: Some(Color::WHITE),
        }),
    )
    .padding(5)
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.4))),
        border: iced::border::Border {
            radius: 3.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    let stacked_image = stack![
        image_widget,
        container(overlay)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Top)
            .padding(10),
        container(filename_overlay)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Bottom)
            .padding(10),
    ];

    use iced::mouse::ScrollDelta;
    use iced::widget::mouse_area;
    let interactive_image = mouse_area(stacked_image)
        .on_scroll(|delta| {
            let zoom_delta = match delta {
                ScrollDelta::Lines { y, .. } => y * 0.1,
                ScrollDelta::Pixels { y, .. } => y * 0.01,
            };
            Message::Zoom(zoom_delta, iced::Point::new(-1.0, -1.0))
        })
        .on_press(Message::MousePressed)
        .on_release(Message::MouseReleased)
        .on_move(|position| Message::MouseMoved(position));

    let content_stack = if let Some(overlay) = overlay_content {
        stack![interactive_image, overlay]
    } else {
        stack![interactive_image]
    };
    container(content_stack)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .clip(true)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::BLACK)),
            ..Default::default()
        })
        .into()
}

fn slider_row<'a, F>(
    label: &'a str,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    on_change: F,
) -> Element<'a, Message>
where
    F: Fn(f32) -> Message + 'a,
{
    row![
        text(label)
            .width(Length::Fixed(90.0))
            .size(13)
            .style(|_theme| text::Style {
                color: Some(Color::from_rgb(0.7, 0.7, 0.7))
            }),
        slider(range, value, on_change)
            .step(step)
            .width(Length::Fill)
            .style(crate::ui::styles::ProSlider::style)
            .on_release(Message::CommitEdit),
        text(format!("{:.2}", value))
            .width(Length::Fixed(40.0))
            .size(13)
            .align_x(iced::alignment::Horizontal::Right)
            .style(|_theme| text::Style {
                color: Some(Color::from_rgb(0.7, 0.7, 0.7))
            }),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center)
    .into()
}
