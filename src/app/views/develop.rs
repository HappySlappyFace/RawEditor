use iced::font::{Font, Weight};
use iced::widget::{
    checkbox, column, container, mouse_area, row, scrollable, slider, stack, text, text_input,
    Container,
};
use crate::ui::widgets::button;
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::message::{DefringeParam, LensParam, LensToggle, Message, SidebarSection};
use crate::app::state::{EditorReadiness, RawEditor};
use crate::database::models::Image as ImageData;
use crate::ui;
use crate::ui::icons::ICON_FONT;
use crate::ui::widgets::transition::entrance;

/// The edit sidebar's background — also what folding sections fade from.
const SIDEBAR_BACKGROUND: Color = Color::from_rgb(0.15, 0.15, 0.15);

/// Build the Develop tab view (full-screen editor with preview)
pub fn view_develop(editor: &RawEditor) -> Element<'_, Message> {
    let sidebar = view_sidebar(editor);
    let sidebar_container = container(
        scrollable(sidebar)
            .width(Length::Fixed(300.0))
            .height(Length::Fill),
    )
    .style(|_theme| container::Style {
        background: Some(Background::Color(SIDEBAR_BACKGROUND)),
        ..Default::default()
    });

    let (has_pixels, overlay_content) = prepare_preview(editor);
    let main_content = view_main_content(editor, has_pixels, overlay_content);

    if matches!(editor.editor_readiness, EditorReadiness::NoSelection) {
        return main_content;
    }

    let editor_content = column![row![main_content, sidebar_container]
        .spacing(0)
        .height(Length::Fill)]
    .width(Length::Fill)
    .height(Length::Fill);
    let filtered_images: Vec<&ImageData> = editor.displayed_images();
    let filmstrip = ui::filmstrip::view(&filtered_images, editor);
    let content: Element<_> = column![
        editor_content,
        Container::new(filmstrip)
            .width(Length::Fill)
            .height(Length::Fixed(115.0))
            .clip(true)
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into();

    // The profiler overlay is NOT stacked here any more — it lives in
    // `views::layout::view_main` so the Window > Performance HUD toggle works
    // in every tab, not just this one.
    content
}

fn view_sidebar(editor: &RawEditor) -> iced::widget::Column<'_, Message> {
    // Slider reset targets (Ctrl/Cmd+click).
    let d = crate::core::types::EditParams::default();
    let histogram_toggle =
        checkbox("Show Histogram", editor.histogram_enabled).on_toggle(Message::HistogramToggled);

    let histogram_section = if editor.histogram_enabled {
        let histogram_widget = iced::widget::canvas::Canvas::new(crate::ui::histogram::Histogram {
            data: *editor.histogram_data.borrow(),
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
    };

    let mut sidebar = column![text("Edit Controls").size(16), camera_profile_block(editor), histogram_toggle];
    if let Some(hist) = histogram_section {
        sidebar = sidebar.push(hist);
    }

    sidebar = sidebar
        .push(text("Tone").size(14))
        .push(slider_row(editor, 
            "Exposure",
            editor.current_edit_params.exposure, d.exposure,
            -5.0..=5.0,
            0.1,
            Message::ExposureChanged,
        ))
        .push(slider_row(editor, 
            "Contrast",
            editor.current_edit_params.contrast, d.contrast,
            -10.0..=10.0,
            0.005,
            Message::ContrastChanged,
        ))
        .push(slider_row(editor, 
            "Highlights",
            editor.current_edit_params.highlights, d.highlights,
            -1.0..=1.0,
            0.01,
            Message::HighlightsChanged,
        ))
        .push(slider_row(editor, 
            "Shadows",
            editor.current_edit_params.shadows, d.shadows,
            -1.0..=1.0,
            0.01,
            Message::ShadowsChanged,
        ))
        .push(slider_row(editor, 
            "Whites",
            editor.current_edit_params.whites, d.whites,
            0.8..=1.2,
            0.01,
            Message::WhitesChanged,
        ))
        .push(slider_row(editor, 
            "Blacks",
            editor.current_edit_params.blacks, d.blacks,
            0.0..=0.2,
            0.005,
            Message::BlacksChanged,
        ))
        .push(collapsible_section(editor, "Tone Curve", SidebarSection::ToneCurve, curve_section(editor)))
        .push(text("Tone Mapping").size(14))
        .push(tone_mapper_selector(editor))
        .push(tone_mapper_hint(editor))
        .push(tone_shape_controls(editor))
        .push(text("Color").size(14))
        .push({
            // Real WB readout: sliders pivot around the camera's as-shot values.
            let (as_k, as_t) = editor.as_shot_wb.unwrap_or((5000.0, 0.0));
            row![
                text(format!("As Shot  {:.0}K · tint {:+.0}", as_k, as_t))
                    .size(11)
                    .width(Length::Fill)
                    .style(|_theme| text::Style {
                        color: Some(Color::from_rgb(0.45, 0.45, 0.45)),
                    }),
                button(
                    text(if editor.is_wb_picking { "Picking…" } else { "WB Picker" }).size(11),
                )
                .style(if editor.is_wb_picking {
                    ui::styles::AccentButton::style
                } else {
                    ui::styles::NeutralButton::style
                })
                .on_press(Message::ToggleWbPicker),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
        })
        .push({
            let (as_k, _) = editor.as_shot_wb.unwrap_or((5000.0, 0.0));
            let kelvin = editor.current_edit_params.kelvin_from_anchor(as_k);
            slider_row_display(editor, 
                "Temp",
                editor.current_edit_params.temperature, d.temperature,
                -1.0..=1.0,
                0.01,
                format!("{:.0}K", kelvin),
                // Inverse of `kelvin_from_anchor`, as the WB picker solves it.
                move |k| (1.0e6 / as_k.clamp(1500.0, 50000.0) - 1.0e6 / k.max(1.0)) / 120.0,
                Message::TemperatureChanged,
            )
        })
        .push({
            let (_, as_t) = editor.as_shot_wb.unwrap_or((5000.0, 0.0));
            let tint = editor.current_edit_params.tint_from_anchor(as_t);
            slider_row_display(editor, 
                "Tint",
                editor.current_edit_params.tint, d.tint,
                -1.0..=1.0,
                0.01,
                format!("{:+.0}", tint),
                move |t| (t - as_t) / 50.0,
                Message::TintChanged,
            )
        })
        .push(slider_row(editor, 
            "Vibrance",
            editor.current_edit_params.vibrance, d.vibrance,
            -1.0..=1.0,
            0.01,
            Message::VibranceChanged,
        ))
        .push(slider_row(editor, 
            "Saturation",
            editor.current_edit_params.saturation, d.saturation,
            -100.0..=100.0,
            1.0,
            Message::SaturationChanged,
        ))
        .push(text("Presence").size(14))
        .push(slider_row(editor, 
            "Texture",
            editor.current_edit_params.texture, d.texture,
            -1.0..=1.0,
            0.01,
            Message::TextureChanged,
        ))
        .push(slider_row(editor, 
            "Clarity",
            editor.current_edit_params.clarity, d.clarity,
            -1.0..=1.0,
            0.01,
            Message::ClarityChanged,
        ))
        .push(slider_row(editor, 
            "Dehaze",
            editor.current_edit_params.dehaze, d.dehaze,
            -1.0..=1.0,
            0.01,
            Message::DehazeChanged,
        ))
        .push(collapsible_section(
            editor,
            "Detail Preview",
            SidebarSection::DetailPreview,
            loupe_section(editor),
        ))
        .push(text("Noise Reduction").size(14))
        .push(slider_row(editor, 
            "Luminance",
            editor.current_edit_params.luma_noise, d.luma_noise,
            0.0..=1.0,
            0.01,
            Message::LumaNoiseChanged,
        ))
        .push(slider_row(editor, 
            "Detail",
            editor.current_edit_params.luma_detail, d.luma_detail,
            0.0..=1.0,
            0.01,
            Message::LumaDetailChanged,
        ))
        .push(slider_row(editor, 
            "Color",
            editor.current_edit_params.color_noise, d.color_noise,
            0.0..=1.0,
            0.01,
            Message::ColorNoiseChanged,
        ))
        .push(text("Detail").size(14))
        .push(slider_row(editor, 
            "Sharpen",
            editor.current_edit_params.sharpening, d.sharpening,
            0.0..=2.0,
            0.01,
            Message::SharpeningChanged,
        ))
        .push(slider_row(editor, 
            "Masking",
            editor.current_edit_params.sharpen_masking, d.sharpen_masking,
            0.0..=1.0,
            0.01,
            Message::SharpenMaskingChanged,
        ))
        .push(text("Geometry").size(14))
        .push(geometry_tools(editor))
        .push(slider_row_display(
            editor,
            "Straighten",
            editor.current_edit_params.rotation, d.rotation,
            -45.0..=45.0,
            0.05,
            format!("{:.1}°", editor.current_edit_params.rotation),
            |v| v,
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
                aspect_crop_button(editor, "1:1", 1.0),
                aspect_crop_button(editor, "16:9", 16.0 / 9.0),
                aspect_crop_button(editor, "2:3", 2.0 / 3.0),
            ]
            .spacing(5),
        );

    sidebar = sidebar
        .push(collapsible_section(editor, "Lens Corrections", SidebarSection::Lens, lens_section(editor)))
        .push(collapsible_section(editor, "Defringe", SidebarSection::Defringe, defringe_section(editor)));

    sidebar = push_mask_section(sidebar, editor);

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
                .step(0.1_f32),
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
                .step(0.1_f32),
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
                .step(0.1_f32),
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
                .step(0.1_f32),
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

/// Quarter turns and the level tool, above the Straighten slider. Both need
/// the open image's GPU resources (a turn reads its shape).
fn geometry_tools(editor: &RawEditor) -> Element<'_, Message> {
    let ready = editor
        .image_resources
        .as_ref()
        .is_some_and(|r| Some(r.image_id) == editor.selected_image_id);
    let turn = |icon: &'static str, clockwise: bool| {
        button(text(icon).font(ICON_FONT).size(15))
            .style(ui::styles::NeutralButton::style)
            .on_press_maybe(ready.then_some(Message::TurnImage { clockwise }))
            .padding([5, 10])
    };
    let level = button(
        row![
            text(ui::icons::LEVEL).font(ICON_FONT).size(13),
            // Armed shows as the accent style; the status line says what to do.
            text("Level").size(12),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .style(if editor.is_leveling {
        ui::styles::AccentButton::style
    } else {
        ui::styles::NeutralButton::style
    })
    .on_press_maybe(ready.then_some(Message::ToggleLevelTool))
    .padding([6, 10]);
    row![
        text("Rotate")
            .width(Length::Fixed(90.0))
            .size(13)
            .style(|_| text::Style { color: Some(Color::from_rgb(0.7, 0.7, 0.7)) }),
        turn(ui::icons::ROTATE_LEFT, false),
        turn(ui::icons::ROTATE_RIGHT, true),
        iced::widget::Space::with_width(Length::Fill),
        level,
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// A centred crop preset. The crop rectangle lives in display (EXIF-oriented)
/// space, so the ratio must be solved against the oriented dimensions — the
/// raw sensor dims are landscape even for a portrait shot, which turned
/// "16:9" into a tall sliver on every vertical frame.
fn aspect_crop_button<'a>(editor: &RawEditor, label: &'a str, ratio: f32) -> Element<'a, Message> {
    button(text(label).size(12))
        .style(ui::styles::NeutralButton::style)
        .on_press_maybe(editor.image_resources.as_ref().map(|res| {
            let (w, h) = res.oriented_dims(editor.current_edit_params.quarter_turns);
            Message::SetCrop(RawEditor::calculate_center_crop(ratio, w, h))
        }))
        .into()
}

/// `(has_pixels, overlay)`. The viewport is a wgpu shader fed from
/// `working_preview_bytes` / `rendered_preview_bytes`, so all the caller needs
/// is whether there is anything to draw — this used to return an
/// `Option<Handle>` that was only ever pattern-matched for presence, which cost
/// a full extra copy of every rendered frame to construct.
fn prepare_preview(editor: &RawEditor) -> (bool, Option<Element<'_, Message>>) {
    match &editor.editor_readiness {
        EditorReadiness::NoSelection => (false, Option::<Element<Message>>::None),
        EditorReadiness::Loading(_) => {
            let has_pixels = editor.working_preview_bytes.is_some();
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
            (has_pixels, Some(entrance(overlay).starting_scale(0.94).starting_offset(0.0, 10.0).into()))
        }
        EditorReadiness::Ready(_) => {
            if let (Some(_ctx), Some(_resources)) = (&editor.gpu_context, &editor.image_resources) {
                // Phase 103/104: Use async rendered preview, falling back to
                // the working preview until the first GPU render lands. Mirrors
                // the byte-buffer priority in `view_main_content`.
                let has_pixels = editor.rendered_preview_bytes.is_some()
                    || editor.working_preview_bytes.is_some();
                (has_pixels, None)
            } else {
                (false, None)
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
            (false, Some(overlay.into()))
        }
    }
}

fn view_main_content<'a>(
    editor: &'a RawEditor,
    has_pixels: bool,
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

    let image_widget: Element<Message> = if has_pixels {
        use crate::ui::preview_renderer::{ViewportProgram, CropOverlay};
        use iced::widget::shader::Shader;
        use iced::widget::canvas::Canvas;

        // Pick the pixel source AND the window that describes it together —
        // they must never come from different renders. The working preview is
        // a whole-image JPEG, so it is always FULL_VIEW_RECT; only the GPU
        // render produces a windowed buffer.
        let (active_bytes, active_view_rect) = match editor.rendered_preview_bytes.clone() {
            Some(b) => (Some(b), editor.rendered_view_rect),
            None => (
                editor.working_preview_bytes.clone(),
                crate::gpu::params::FULL_VIEW_RECT,
            ),
        };


        // Two DIFFERENT dims are needed here and conflating them is the classic
        // bug in this file:
        //   buffer_dims  — size of the pixel buffer, i.e. the render target.
        //                  Since the windowed render this is viewport-shaped,
        //                  not image-shaped. Only the texture cares.
        //   image_dims   — the whole image's display shape. Everything that
        //                  positions pixels on screen (letterbox fit, zoom,
        //                  pan, crop/mask handles) must use this.
        let (buf_w, buf_h) = editor.buffer_dims();
        let (img_w, img_h) = editor.image_display_dims();

        match &editor.editor_readiness {
            EditorReadiness::Loading(_) | EditorReadiness::Ready(_) => {
                // Phase 115: Bottom layer – native GPU shader viewport (respects scissor rects!)
                let viewport: Element<'_, Message> = Shader::new(ViewportProgram {
                    pixels: active_bytes,
                    buffer_width: buf_w,
                    buffer_height: buf_h,
                    image_width: img_w,
                    image_height: img_h,
                    view_rect: active_view_rect,
                    zoom: editor.zoom,
                    offset: editor.pan_offset,
                    generation: editor.preview_generation,
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .into();

                // Always include the CropOverlay canvas so its update() fires
                // ViewportResized whenever bounds change, keeping viewport_size accurate
                // even when not cropping. Its draw() returns nothing when is_cropping=false.
                // While peeking at the original, hide the editing furniture —
                // crop and mask handles describe adjustments that aren't being
                // applied to what's on screen. The Canvas itself must STAY in
                // the stack regardless: its update() is what emits
                // ViewportResized, which keeps viewport_size accurate.
                let peeking = editor.before_peek;
                let overlay: Element<'_, Message> = Canvas::new(CropOverlay {
                    zoom: editor.zoom,
                    offset: editor.pan_offset,
                    image_width: img_w,
                    image_height: img_h,
                    is_cropping: editor.is_cropping && !peeking,
                    is_wb_picking: editor.is_wb_picking && !peeking,
                    crop: editor.current_edit_params.crop,
                    is_mask_placing: !peeking
                        && editor.mask_tool != crate::app::state::MaskTool::Inactive,
                    selected_mask: if peeking {
                        None
                    } else {
                        editor.selected_mask.and_then(|i| {
                            (i < editor.current_edit_params.mask_count as usize)
                                .then(|| editor.current_edit_params.masks[i])
                        })
                    },
                    brush: (!peeking && editor.brush_armed && editor.selected_mask.is_some())
                        .then_some((editor.brush_settings.size, editor.brush_settings.feather)),
                    is_color_picking: !peeking && editor.color_picking_mask.is_some(),
                    is_loupe_targeting: !peeking && editor.loupe.targeting,
                    is_leveling: !peeking && editor.is_leveling,
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .into();

                // Without a badge there is nothing distinguishing "the original"
                // from "an edit that did nothing" — which is a fair part of why
                // the old before/after went unnoticed after it broke.
                // Deliberately not animated: a before/after comparison is
                // only as good as its switch is instant.
                let badge: Element<'_, Message> = if peeking {
                    container(
                        container(
                            text("BEFORE")
                                .size(11)
                                .font(Font { weight: Weight::Bold, ..Default::default() })
                                .style(|_theme| text::Style { color: Some(Color::WHITE) }),
                        )
                        .padding([4, 10])
                        .style(|_theme| container::Style {
                            background: Some(Background::Color(Color::from_rgb(
                                0.98, 0.45, 0.09,
                            ))),
                            border: Border { radius: 3.0.into(), ..Default::default() },
                            ..Default::default()
                        }),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Top)
                    .padding(16)
                    .into()
                } else {
                    iced::widget::Space::new(Length::Shrink, Length::Shrink).into()
                };

                container(
                    stack![viewport, overlay, badge]
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .clip(true)
                .into()
            }
            // Unreachable in practice: prepare_preview only reports pixels for
            // Loading/Ready, so has_pixels gates this whole block. The other
            // states render their own overlay instead.
            _ => iced::widget::Space::new(Length::Fill, Length::Fill).into(),
        }

    } else {
        iced::widget::Space::new(Length::Fill, Length::Fill).into()
    };

    // Instant, like the BEFORE badge: an info toggle is a glance.
    let overlay: Element<'_, Message> = match editor.info_overlay {
        crate::app::state::InfoOverlayState::Hidden => container(column![]).into(),
        // Cache debug: what the loader is actually holding. Previously this
        // shared an arm with Metadata and rendered identically, so cycling with
        // `I` appeared to show the same panel twice before hiding.
        crate::app::state::InfoOverlayState::CacheDebug => {
            let budget = editor.raw_cache_budget_bytes().max(1);
            let used_pct = (editor.raw_cache_bytes as f64 / budget as f64 * 100.0).min(999.0);
            let selected_cached = editor
                .selected_image_id
                .map(|id| {
                    let raw = editor.raw_cache.contains(&id);
                    let prev = editor.preview_cache.contains(&id);
                    match (raw, prev) {
                        (true, true) => "raw + preview",
                        (true, false) => "raw only",
                        (false, true) => "preview only",
                        (false, false) => "not cached",
                    }
                })
                .unwrap_or("no selection");

            info_panel(
                column![
                    text("CACHE").size(12).style(|_| text::Style {
                        color: Some(Color::from_rgb(0.98, 0.45, 0.09))
                    }),
                    info_line(format!(
                        "RAW  {} files  {:.0} / {} MB  ({:.0}%)",
                        editor.raw_cache.len(),
                        editor.raw_cache_bytes as f64 / (1024.0 * 1024.0),
                        editor.raw_preload_budget_mb,
                        used_pct,
                    )),
                    info_line(format!(
                        "Preview  {} / {} entries",
                        editor.preview_cache.len(),
                        editor.cache_capacity,
                    )),
                    info_line(format!(
                        "In flight  {} preview  {} raw",
                        editor.pending_loads.len(),
                        editor.pending_raw_loads.len(),
                    )),
                    info_line(format!(
                        "Queued  {} preview  {} raw",
                        editor.queued_loads.len(),
                        editor.queued_raw_loads.len(),
                    )),
                    info_line(format!("This image  {}", selected_cached)),
                ]
                .spacing(2)
                .into(),
            )
            .into()
        }
        crate::app::state::InfoOverlayState::Metadata => {
            let mut lines = column![
                // 100% = one sensor pixel per screen pixel, not "fitted".
                text(format!("Zoom {:.0}%", editor.zoom_percent()))
                    .size(12)
                    .style(|_| text::Style {
                        color: Some(Color::from_rgb(0.98, 0.45, 0.09))
                    }),
            ]
            .spacing(2);
            for line in metadata_lines(editor) {
                lines = lines.push(info_line(line));
            }
            info_panel(lines.into()).into()
        }
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
        .on_move(Message::MouseMoved);

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
        .style(|_| container::Style {
            // The shader quad now covers exactly the on-screen image rect (no
            // more, no less), so this container background IS the letterbox
            // color on every side, including portrait side bars.
            background: Some(Background::Color(Color::from_rgb(0.12, 0.12, 0.12))),
            ..Default::default()
        })
        .into()
}

/// "Local Masks" sidebar section: add-mask buttons, the mask list, and the
/// per-mask adjustment sliders for the selected mask.
fn push_mask_section<'a>(
    mut sidebar: iced::widget::Column<'a, Message>,
    editor: &'a RawEditor,
) -> iced::widget::Column<'a, Message> {
    use crate::app::message::{BrushSetting, MaskAdjustment};
    use crate::app::state::MaskTool;
    use crate::core::types::{MASK_BRUSH, MASK_LINEAR, MASK_RADIAL, MASK_RANGE, MAX_MASKS};

    let params = &editor.current_edit_params;
    let count = params.mask_count as usize;

    sidebar = sidebar.push(text("Local Masks").size(14).font(Font {
        weight: Weight::Bold,
        ..Default::default()
    }));

    // Add-mask buttons. Linear/radial toggle placement mode (the next
    // click-drag on the preview places the mask); brush and range masks have
    // no geometry and are created straight away. Disabled at the mask limit,
    // except to cancel a mode.
    let can_add = count < MAX_MASKS;
    let linear_active = editor.mask_tool == MaskTool::PlacingLinear;
    let radial_active = editor.mask_tool == MaskTool::PlacingRadial;
    let add_button = |label: &'static str, active: bool, kind: u32| {
        button(text(if active { "Click image…" } else { label }).size(12))
            .style(if active {
                ui::styles::AccentButton::style
            } else {
                ui::styles::NeutralButton::style
            })
            .on_press_maybe((can_add || active).then_some(Message::ToggleMaskPlacement(kind)))
            .width(Length::Fill)
    };
    sidebar = sidebar
        .push(row![add_button("+ Linear", linear_active, 0), add_button("+ Radial", radial_active, 1)].spacing(5))
        .push(row![add_button("+ Brush", false, MASK_BRUSH), add_button("+ Range", false, MASK_RANGE)].spacing(5));

    // Mask list: select / enable / delete
    for i in 0..count {
        let m = params.masks[i];
        let selected = editor.selected_mask == Some(i);
        let kind = match m.mask_type {
            MASK_LINEAR => "Linear",
            MASK_RADIAL => "Radial",
            MASK_BRUSH => "Brush",
            _ => "Range",
        };
        let mut extras = String::new();
        if m.mask_type != MASK_BRUSH && params.brush.has_strokes_for(i) {
            extras.push_str(" +brush");
        }
        if m.luma_range != 0 {
            extras.push_str(" ·L");
        }
        if m.color_range != 0 {
            extras.push_str(" ·C");
        }
        let label = format!("{kind} {}{extras}", i + 1);
        sidebar = sidebar.push(
            row![
                button(text(label).size(12))
                    .style(if selected {
                        ui::styles::AccentButton::style
                    } else {
                        ui::styles::NeutralButton::style
                    })
                    .on_press(Message::SelectMask(if selected { None } else { Some(i) }))
                    .width(Length::Fill),
                checkbox("", m.enabled != 0)
                    .on_toggle(move |checked| Message::ToggleMaskEnabled(i, checked))
                    .size(16),
                button(text(ui::icons::TIMES).font(ICON_FONT).size(12))
                    .style(ui::styles::NeutralButton::style)
                    .on_press(Message::DeleteMask(i))
                    .padding(6),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        );
    }

    // Per-mask controls for the selected mask
    let Some(i) = editor.selected_mask.filter(|&i| i < count) else {
        return sidebar;
    };
    let m = params.masks[i];
    let small = |label: &'a str, on: bool| {
        checkbox(label, on).size(16).text_size(13)
    };
    sidebar = sidebar.push(
        row![
            small("Invert", m.invert != 0).on_toggle(Message::ToggleMaskInvert),
            small("Show overlay", editor.show_mask_overlay).on_toggle(Message::ToggleMaskOverlay),
        ]
        .spacing(12),
    );

    // Brush: painting is the whole of a brush mask, and a refinement
    // (add / erase) for any other.
    let b = editor.brush_settings;
    let is_brush = m.mask_type == MASK_BRUSH;
    let mut brush_row = row![].spacing(4).align_y(Alignment::Center);
    if !is_brush {
        brush_row = brush_row.push(
            button(text(if editor.brush_armed { "Brushing" } else { "Brush" }).size(11))
                .style(if editor.brush_armed {
                    ui::styles::AccentButton::style
                } else {
                    ui::styles::NeutralButton::style
                })
                .on_press(Message::ToggleBrushArmed),
        );
    }
    if editor.brush_armed {
        for (label, erase) in [("Paint", false), ("Erase", true)] {
            brush_row = brush_row.push(
                button(text(label).size(11))
                    .style(if b.erase == erase {
                        ui::styles::AccentButton::style
                    } else {
                        ui::styles::NeutralButton::style
                    })
                    .on_press(Message::SetBrushErase(erase)),
            );
        }
    }
    brush_row = brush_row.push(iced::widget::Space::with_width(Length::Fill));
    if params.brush.has_strokes_for(i) {
        brush_row = brush_row.push(
            button(text("Clear brush").size(11))
                .style(ui::styles::NeutralButton::style)
                .on_press(Message::ClearMaskBrush),
        );
    }
    sidebar = sidebar.push(brush_row);
    if editor.brush_armed {
        sidebar = sidebar
            .push(slider_row_plain(editor, "Size", b.size, 0.002..=0.3, 0.001, format!("{:.1}%", b.size * 100.0), |p| p / 100.0, |v| {
                Message::BrushSettingChanged(BrushSetting::Size, v)
            }))
            .push(slider_row_plain(editor, "Feather", b.feather, 0.0..=1.0, 0.01, format!("{:.2}", b.feather), |v| v, |v| {
                Message::BrushSettingChanged(BrushSetting::Feather, v)
            }))
            .push(slider_row_plain(editor, "Flow", b.flow, 0.01..=1.0, 0.01, format!("{:.2}", b.flow), |v| v, |v| {
                Message::BrushSettingChanged(BrushSetting::Flow, v)
            }))
            .push(hint("Alt while painting swaps paint / erase".to_string()));
    }

    // Range refinements.
    sidebar = sidebar.push(small("Luminance range", m.luma_range != 0).on_toggle(Message::ToggleMaskLumaRange));
    if m.luma_range != 0 {
        sidebar = sidebar
            .push(slider_row(editor, "Low", m.luma_lo, 0.0, 0.0..=1.0, 0.01, |v| {
                Message::MaskAdjustmentChanged(MaskAdjustment::LumaLo, v)
            }))
            .push(slider_row(editor, "High", m.luma_hi, 1.0, 0.0..=1.0, 0.01, |v| {
                Message::MaskAdjustmentChanged(MaskAdjustment::LumaHi, v)
            }))
            .push(slider_row(editor, "Smooth", m.luma_smooth, 0.1, 0.0..=0.5, 0.01, |v| {
                Message::MaskAdjustmentChanged(MaskAdjustment::LumaSmooth, v)
            }));
    }
    let picking = editor.color_picking_mask == Some(i);
    sidebar = sidebar.push(
        row![
            small("Color range", m.color_range != 0 || picking).on_toggle(Message::ToggleMaskColorRange),
            iced::widget::Space::with_width(Length::Fill),
            button(text(if picking { "Picking…" } else { "Pick color" }).size(11))
                .style(if picking { ui::styles::AccentButton::style } else { ui::styles::NeutralButton::style })
                .on_press(Message::PickMaskColor),
        ]
        .align_y(Alignment::Center),
    );
    if m.color_range != 0 {
        sidebar = sidebar.push(slider_row(editor, "Tolerance", m.color_tolerance, 0.3, 0.0..=1.0, 0.01, |v| {
            Message::MaskAdjustmentChanged(MaskAdjustment::ColorTolerance, v)
        }));
    }

    // Adjustments.
    sidebar = sidebar
        .push(text("Adjustments").size(12))
        .push(slider_row(editor, "Exposure", m.exposure, 0.0, -4.0..=4.0, 0.05, |v| {
            Message::MaskAdjustmentChanged(MaskAdjustment::Exposure, v)
        }))
        .push(slider_row(editor, "Contrast", m.contrast, 0.0, -1.0..=1.0, 0.01, |v| {
            Message::MaskAdjustmentChanged(MaskAdjustment::Contrast, v)
        }))
        .push(slider_row(editor, "Highlights", m.highlights, 0.0, -1.0..=1.0, 0.01, |v| {
            Message::MaskAdjustmentChanged(MaskAdjustment::Highlights, v)
        }))
        .push(slider_row(editor, "Shadows", m.shadows, 0.0, -1.0..=1.0, 0.01, |v| {
            Message::MaskAdjustmentChanged(MaskAdjustment::Shadows, v)
        }))
        .push(slider_row(editor, "Saturation", m.saturation, 0.0, -1.0..=1.0, 0.01, |v| {
            Message::MaskAdjustmentChanged(MaskAdjustment::Saturation, v)
        }))
        .push(slider_row_display(editor, 
            "Hue",
            m.hue, 0.0,
            -1.0..=1.0,
            0.01,
            format!("{:+.0}°", m.hue * crate::gpu::params::MASK_HUE_RANGE_DEG),
            |deg| deg / crate::gpu::params::MASK_HUE_RANGE_DEG,
            |v| Message::MaskAdjustmentChanged(MaskAdjustment::Hue, v),
        ))
        .push(slider_row(editor, "Warmth", m.warmth, 0.0, -1.0..=1.0, 0.01, |v| {
            Message::MaskAdjustmentChanged(MaskAdjustment::Warmth, v)
        }))
        .push(slider_row(editor, "Tint", m.tint, 0.0, -1.0..=1.0, 0.01, |v| {
            Message::MaskAdjustmentChanged(MaskAdjustment::Tint, v)
        }));
    if m.mask_type == MASK_RADIAL {
        sidebar = sidebar
            .push(slider_row(editor, "Feather", m.feather, 0.5, 0.0..=1.0, 0.01, |v| {
                Message::MaskAdjustmentChanged(MaskAdjustment::Feather, v)
            }))
            .push(slider_row_display(editor, 
                "Rotation",
                m.rotation, 0.0,
                -180.0..=180.0,
                1.0,
                format!("{:.0}°", m.rotation),
                |v| v,
                |v| Message::MaskAdjustmentChanged(MaskAdjustment::Rotation, v),
            ));
    }

    sidebar
}

/// A section title that folds its body away (`ui::widgets::collapse`).
///
/// Header and body are a single sidebar item, with the gap between them
/// inside the body: a folded section then adds no spacing of its own, and
/// the gap folds away with the body instead of snapping.
fn collapsible_section<'a>(
    editor: &RawEditor,
    title: &'a str,
    section: SidebarSection,
    body: Element<'a, Message>,
) -> Element<'a, Message> {
    let body = container(body).padding(iced::Padding { top: 10.0, ..iced::Padding::ZERO });
    column![
        section_header(editor, title, section),
        crate::ui::widgets::collapse::collapse(expanded(editor, section), body)
            .veil(SIDEBAR_BACKGROUND),
    ]
    .into()
}

fn expanded(editor: &RawEditor, section: SidebarSection) -> bool {
    !editor.collapsed_sections.contains(&section)
}

/// A clickable section title with a disclosure arrow.
fn section_header<'a>(editor: &RawEditor, title: &'a str, section: SidebarSection) -> Element<'a, Message> {
    let arrow = if expanded(editor, section) { "▾" } else { "▸" };
    button(
        row![text(arrow).size(12), text(title).size(14)]
            .spacing(6)
            .align_y(Alignment::Center),
    )
    .padding([2, 0])
    .width(Length::Fill)
    .style(|_theme, _status| button::Style {
        background: None,
        text_color: Color::WHITE,
        ..Default::default()
    })
    .on_press(Message::ToggleSection(section))
    .into()
}

/// The 1:1 detail loupe (`handlers::loupe`). Noise reduction is held back on
/// the main view while its sliders move; this is where it shows live, at the
/// sensor's own resolution — the only scale grain can be judged at.
fn loupe_section(editor: &RawEditor) -> Element<'_, Message> {
    use crate::app::handlers::loupe::LOUPE_SIZE;
    let frame = editor.loupe.frame.as_ref().filter(|f| Some(f.image_id) == editor.selected_image_id);
    let picture: Element<'_, Message> = match frame {
        Some(f) => stack![
            iced::widget::image(f.handle.clone())
                .width(Length::Fixed(LOUPE_SIZE))
                .height(Length::Fixed(LOUPE_SIZE)),
            iced::widget::canvas::Canvas::new(ui::loupe::LoupeDrag)
                .width(Length::Fixed(LOUPE_SIZE))
                .height(Length::Fixed(LOUPE_SIZE)),
        ]
        .into(),
        None => {
            let failed = editor.loupe.failed.as_ref().filter(|(id, _)| Some(*id) == editor.selected_image_id);
            let message = match failed {
                Some((_, e)) => format!("Unavailable: {e}"),
                None => "Preparing 1:1 preview…".to_string(),
            };
            container(hint(message))
                .width(Length::Fixed(LOUPE_SIZE))
                .height(Length::Fixed(LOUPE_SIZE))
                .center_x(Length::Fixed(LOUPE_SIZE))
                .center_y(Length::Fixed(LOUPE_SIZE))
                .into()
        }
    };
    let framed = container(picture).padding(1).style(|_theme| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.1, 0.1, 0.1))),
        border: Border { color: Color::from_rgb(0.3, 0.3, 0.3), width: 1.0, radius: 0.0.into() },
        ..Default::default()
    });

    let targeting = editor.loupe.targeting;
    let pick = button(
        row![
            text("\u{f05b}").font(ICON_FONT).size(12), // crosshairs
            text(if targeting { "Click the image…" } else { "Pick Point" }).size(11),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .style(if targeting { ui::styles::AccentButton::style } else { ui::styles::NeutralButton::style })
    .on_press(Message::ToggleLoupeTarget)
    .padding([4, 8]);

    column![
        container(framed).center_x(Length::Fill),
        pick,
        hint("Shown at 1:1 — drag to move it. Noise reduction reaches the main image when its slider pauses.".to_string()),
    ]
    .spacing(6)
    .into()
}

/// One entry of the camera-profile dropdown.
#[derive(Debug, Clone, PartialEq)]
struct ProfileOption {
    choice: crate::core::camera_profiles::ProfileChoice,
    label: String,
}

impl std::fmt::Display for ProfileOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// Top of the sidebar: which camera profile (DCP) this image uses — a
/// dropdown when the camera has any — or, when it has none, the offer to
/// download an open-source one (`handlers::camera_profiles`).
fn camera_profile_block(editor: &RawEditor) -> Element<'_, Message> {
    use crate::core::camera_profiles::{camera_name, Catalog, ProfileChoice};
    let Some(meta) = editor.current_metadata.as_ref() else { return column![].into() };
    let Some(catalog) = Catalog::loaded() else {
        return hint("Reading camera profiles…".to_string());
    };
    let camera = camera_name(&meta.make, &meta.model);
    let installed = catalog.for_camera(&meta.make, &meta.model);
    if installed.is_empty() {
        return missing_profile_prompt(editor, camera);
    }

    // Two files that call themselves the same are told apart by file name.
    let label = |p: &crate::core::camera_profiles::ProfileInfo| {
        if installed.iter().filter(|q| q.name == p.name).count() > 1 {
            p.file_name.trim_end_matches(".dcp").to_string()
        } else {
            p.name.clone()
        }
    };
    // What "Default" means for this camera right now.
    let default_label = match catalog.default_for(&editor.camera_profile_defaults, &meta.make, &meta.model).map(String::as_str) {
        Some("builtin") => "Default (none)".to_string(),
        Some(file) => match catalog.find(file) {
            Some(p) => format!("Default ({})", label(p)),
            None => format!("Default ({})", label(installed[0])),
        },
        None => format!("Default ({})", label(installed[0])),
    };
    let mut options = vec![ProfileOption { choice: ProfileChoice::AUTO, label: default_label }];
    options.extend(installed.iter().filter_map(|p| {
        ProfileChoice::named(&p.file_name).map(|choice| ProfileOption { choice, label: label(p) })
    }));
    options.push(ProfileOption { choice: ProfileChoice::BUILTIN, label: "None (built-in colour)".to_string() });
    let current = editor.current_edit_params.camera_profile;
    // A named profile that has since been removed resolves like Default.
    let selected = options.iter().find(|o| o.choice == current).or(options.first()).cloned();

    row![
        text("Profile").size(13).width(Length::Fixed(60.0)),
        iced::widget::pick_list(options, selected, |o: ProfileOption| Message::CameraProfileSelected(o.choice))
            .text_size(12)
            .padding([4, 8])
            .width(Length::Fill)
            .style(ui::styles::pick_list_style)
            .menu_style(ui::styles::pick_list_menu_style),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// No camera profile for this camera: say what is being used instead, and
/// offer RawTherapee's open-source profile or an import.
fn missing_profile_prompt(editor: &RawEditor, camera: String) -> Element<'_, Message> {
    use crate::app::state::ProfileJob;
    let state = &editor.camera_profiles;
    let unavailable = state.unavailable.as_deref() == Some(camera.as_str());
    let warn = Color::from_rgb(0.9, 0.55, 0.3);

    let download = button(
        row![
            text("\u{f019}").font(ICON_FONT).size(12), // download
            text(if state.busy == Some(ProfileJob::Downloading) { "Downloading…" } else { "Download" }).size(12),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .style(ui::styles::AccentButton::style)
    .on_press_maybe((state.busy.is_none() && !unavailable).then_some(Message::DownloadCameraProfile))
    .padding([5, 10]);
    let import = button(text("Import…").size(12))
        .style(ui::styles::NeutralButton::style)
        .on_press_maybe(state.busy.is_none().then_some(Message::ImportCameraProfiles))
        .padding([5, 10]);

    let mut col = column![
        text("No DCP profile found").size(13),
        hint(format!(
            "{camera} is using the default colour pipeline. Would you like to download an open-source profile for it?"
        )),
        row![download, import].spacing(6),
    ]
    .spacing(6);
    if unavailable {
        col = col.push(
            text(format!("The RawTherapee project has no profile for {camera}. You can import one of your own instead."))
                .size(10)
                .style(move |_theme| text::Style { color: Some(warn) }),
        );
    } else if let Some(Err(e)) = &state.notice {
        col = col.push(text(e.clone()).size(10).style(move |_theme| text::Style { color: Some(warn) }));
    }
    container(col)
        .padding(10)
        .width(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.12, 0.12, 0.12))),
            border: Border { color: ui::palette::accent_selection_border(), width: 1.0, radius: 4.0.into() },
            ..Default::default()
        })
        .into()
}

/// No lensfun database at all: offer the one-click download, and say how to
/// install it by hand instead (`handlers::lensfun`).
fn missing_database_prompt(editor: &RawEditor) -> Element<'_, Message> {
    use crate::app::state::LensfunJob;
    let downloading = editor.lensfun.busy == Some(LensfunJob::Downloading);
    let download = button(
        row![
            text("\u{f019}").font(ICON_FONT).size(13), // download
            text(if downloading { "Downloading…" } else { "Download Lens Profiles" }).size(12),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .style(ui::styles::AccentButton::style)
    .on_press_maybe(editor.lensfun.busy.is_none().then_some(Message::LensfunDownload))
    .padding([5, 10]);
    let mut col = column![
        hint("No lens profiles are installed.".to_string()),
        download,
        hint(format!(
            "About 0.4 MB from the lensfun project. Or install the `lensfun` package, or put its XML files in {}",
            crate::core::paths::lensfun_dir().display()
        )),
    ]
    .spacing(6);
    if let Some(Err(e)) = &editor.lensfun.installed {
        col = col.push(
            text(format!("Download failed: {e}"))
                .size(10)
                .style(|_theme| text::Style { color: Some(Color::from_rgb(0.9, 0.55, 0.3)) }),
        );
    }
    col.into()
}

fn hint<'a>(s: String) -> Element<'a, Message> {
    text(s)
        .size(10)
        .style(|_theme| text::Style { color: Some(Color::from_rgb(0.45, 0.45, 0.45)) })
        .into()
}

/// Profile toggles (with what was matched, or why nothing was) and the
/// manual sliders.
fn lens_section(editor: &RawEditor) -> Element<'_, Message> {
    let l = editor.current_edit_params.lens;
    let d = crate::core::lens::LensParams::default();
    let ctx = editor.current_metadata.as_ref().map(|m| m.lens_context.clone());
    let lens_name = ctx
        .as_ref()
        .and_then(|c| c.lens_name.clone())
        .unwrap_or_else(|| "Unknown lens".to_string());
    let has_profile = ctx.as_ref().is_some_and(|c| c.profile.is_some());
    let no_database = crate::core::lens::LensDb::loaded_count() == Some(0);
    let status: Element<'_, Message> = match (has_profile, no_database) {
        (true, _) => hint(format!(
            "Profile: {}",
            ctx.as_ref().and_then(|c| c.profile.as_ref()).map(|p| p.lens_name.as_str()).unwrap_or("")
        )),
        (false, true) => missing_database_prompt(editor),
        (false, false) => hint(
            "No profile for this lens in the lensfun database — Preferences › Lens Profiles can check for a newer one."
                .to_string(),
        ),
    };

    let mut col = column![
        text(lens_name).size(12),
        status,
        checkbox("Enable profile corrections", l.profile)
            .on_toggle_maybe(has_profile.then_some(|on| Message::LensToggled(LensToggle::Profile, on)))
            .size(16)
            .text_size(13),
        checkbox("Remove chromatic aberration", l.profile_ca)
            .on_toggle_maybe(has_profile.then_some(|on| Message::LensToggled(LensToggle::ProfileCa, on)))
            .size(16)
            .text_size(13),
    ]
    .spacing(6);
    if l.profile && has_profile {
        col = col
            .push(slider_row_display(editor, 
                "Distortion",
                l.profile_distortion, d.profile_distortion,
                0.0..=2.0,
                0.01,
                format!("{:.0}%", l.profile_distortion * 100.0),
                |p| p / 100.0,
                |v| Message::LensParamChanged(LensParam::ProfileDistortion, v),
            ))
            .push(slider_row_display(editor, 
                "Vignetting",
                l.profile_vignetting, d.profile_vignetting,
                0.0..=2.0,
                0.01,
                format!("{:.0}%", l.profile_vignetting * 100.0),
                |p| p / 100.0,
                |v| Message::LensParamChanged(LensParam::ProfileVignetting, v),
            ));
    }
    col.push(text("Manual").size(12))
        .push(slider_row(editor, "Distortion", l.distortion, d.distortion, -1.0..=1.0, 0.01, |v| {
            Message::LensParamChanged(LensParam::Distortion, v)
        }))
        .push(slider_row(editor, "Vignetting", l.vignetting, d.vignetting, -1.0..=1.0, 0.01, |v| {
            Message::LensParamChanged(LensParam::Vignetting, v)
        }))
        .push(slider_row(editor, "Red / Cyan", l.ca_red, d.ca_red, -1.0..=1.0, 0.01, |v| {
            Message::LensParamChanged(LensParam::CaRed, v)
        }))
        .push(slider_row(editor, "Blue / Yellow", l.ca_blue, d.ca_blue, -1.0..=1.0, 0.01, |v| {
            Message::LensParamChanged(LensParam::CaBlue, v)
        }))
        .push(
            checkbox("Constrain (no empty borders)", l.constrain)
                .on_toggle(|on| Message::LensToggled(LensToggle::Constrain, on))
                .size(16)
                .text_size(13),
        )
        .into()
}

fn defringe_section(editor: &RawEditor) -> Element<'_, Message> {
    let f = editor.current_edit_params.defringe;
    let d = crate::core::lens::DefringeParams::default();
    column![
        slider_row(editor, "Purple", f.purple_amount, d.purple_amount, 0.0..=1.0, 0.01, |v| {
            Message::DefringeChanged(DefringeParam::PurpleAmount, v)
        }),
        slider_row_display(editor, "Purple Hue", f.purple_hue, d.purple_hue, 200.0..=360.0, 1.0, format!("{:.0}°", f.purple_hue), |v| v, |v| {
            Message::DefringeChanged(DefringeParam::PurpleHue, v)
        }),
        slider_row(editor, "Green", f.green_amount, d.green_amount, 0.0..=1.0, 0.01, |v| {
            Message::DefringeChanged(DefringeParam::GreenAmount, v)
        }),
        slider_row_display(editor, "Green Hue", f.green_hue, d.green_hue, 40.0..=180.0, 1.0, format!("{:.0}°", f.green_hue), |v| v, |v| {
            Message::DefringeChanged(DefringeParam::GreenHue, v)
        }),
    ]
    .spacing(6)
    .into()
}

/// Channel tabs, the curve canvas and a reset for the shown channel.
fn curve_section(editor: &RawEditor) -> Element<'_, Message> {
    use crate::core::curves::CurveChannel;
    let mut tabs = row![].spacing(4).align_y(Alignment::Center);
    for ch in CurveChannel::ALL {
        let edited = !editor.current_edit_params.curves.get(ch).is_identity();
        let label = if edited { format!("{}•", ch.label()) } else { ch.label().to_string() };
        tabs = tabs.push(
            button(text(label).size(11))
                .width(Length::Fill)
                .style(if editor.curve_channel == ch {
                    ui::styles::AccentButton::style
                } else {
                    ui::styles::NeutralButton::style
                })
                .on_press(Message::SetCurveChannel(ch)),
        );
    }
    tabs = tabs.push(
        button(text(ui::icons::RESET).font(ICON_FONT).size(11))
            .style(ui::styles::NeutralButton::style)
            .on_press(Message::ResetCurve),
    );
    let canvas = iced::widget::canvas::Canvas::new(crate::ui::curve_editor::CurveEditor {
        curves: editor.current_edit_params.curves,
        channel: editor.curve_channel,
        histogram: *editor.histogram_data.borrow(),
    })
    .width(Length::Fill)
    .height(Length::Fixed(230.0));
    column![tabs, canvas].spacing(6).into()
}

/// Operator picker: two rows of small buttons, the active one accented.
///
/// Buttons rather than a `pick_list` because the whole sidebar is already
/// built from this vocabulary (see the WB Picker toggle) and a stock dropdown
/// would need styling from scratch to not look pasted in. It also keeps every
/// option visible, which matters when the options are looks to compare.
fn tone_mapper_selector(editor: &RawEditor) -> Element<'_, Message> {
    use crate::core::tonemap::ToneMapper;
    let active = editor.current_edit_params.tone_mapper;

    let make = |m: ToneMapper| -> Element<'_, Message> {
        button(text(m.label()).size(11))
            .width(Length::Fill)
            .style(if m == active {
                ui::styles::AccentButton::style
            } else {
                ui::styles::NeutralButton::style
            })
            .on_press(Message::ToneMapperChanged(m))
            .into()
    };

    let mut top = row![].spacing(4);
    let mut bottom = row![].spacing(4);
    for (i, m) in ToneMapper::ALL.into_iter().enumerate() {
        if i < 3 {
            top = top.push(make(m));
        } else {
            bottom = bottom.push(make(m));
        }
    }
    column![top, bottom].spacing(4).into()
}

/// One line of plain language about what the current selection actually does.
/// These operators are not self-explanatory from a six-letter button, and the
/// difference between "matches your camera" and "is a look" is the single
/// most useful thing to know here.
fn tone_mapper_hint(editor: &RawEditor) -> Element<'_, Message> {
    use crate::core::tonemap::ToneMapper;
    let hint = match editor.current_edit_params.tone_mapper {
        ToneMapper::Camera => "Profile curve — calibrated to the camera's own rendering",
        ToneMapper::Filmic => "Built-in S-curve: midtone lift + highlight rolloff",
        ToneMapper::Reinhard => "Reference only — flat midtones by design",
        ToneMapper::Hable => "Uncharted 2 filmic: deep toe, long shoulder",
        ToneMapper::AcesFitted => "ACES tone response (Narkowicz fit) — contrasty",
        ToneMapper::Gt => "Gran Turismo — linear mids, shaped toe and shoulder",
    };
    text(hint)
        .size(10)
        .style(|_theme| text::Style {
            color: Some(Color::from_rgb(0.45, 0.45, 0.45)),
        })
        .into()
}

/// The shape controls belonging to the selected operator: the profile-curve
/// blend for `Camera`, GT's four parameters for `Gt`, nothing for the rest
/// (they are fixed published curves with no exposed knobs).
fn tone_shape_controls(editor: &RawEditor) -> Element<'_, Message> {
    use crate::app::message::GtParam;
    use crate::core::tonemap::ToneMapper;

    let p = &editor.current_edit_params;
    let d = crate::core::types::EditParams::default();
    match p.tone_mapper {
        ToneMapper::Camera => column![slider_row(editor, 
            "Profile Curve",
            p.profile_curve, d.profile_curve,
            0.0..=1.0,
            0.01,
            Message::ProfileCurveChanged,
        )]
        .into(),
        ToneMapper::Gt => column![
            slider_row(editor, "Contrast", p.gt.contrast, d.gt.contrast, 0.1..=3.0, 0.01, |v| {
                Message::GtParamChanged(GtParam::Contrast, v)
            }),
            slider_row(editor, "Linear Start", p.gt.linear_start, d.gt.linear_start, 0.01..=0.9, 0.01, |v| {
                Message::GtParamChanged(GtParam::LinearStart, v)
            }),
            slider_row(editor, "Linear Length", p.gt.linear_length, d.gt.linear_length, 0.0..=1.0, 0.01, |v| {
                Message::GtParamChanged(GtParam::LinearLength, v)
            }),
            slider_row(editor, "Black Tight", p.gt.black_tightness, d.gt.black_tightness, 0.5..=4.0, 0.01, |v| {
                Message::GtParamChanged(GtParam::BlackTightness, v)
            }),
        ]
        .spacing(6)
        .into(),
        _ => column![].into(),
    }
}

/// Shared chrome for the `I` info overlay panels, so the metadata and cache
/// states read as siblings rather than two unrelated boxes.
///
/// A free function rather than a closure: closure lifetime elision ties the
/// argument and return lifetimes to different regions and won't compile here.
fn info_panel<'a>(body: Element<'a, Message>) -> Container<'a, Message> {
    container(body)
        .padding(10)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))),
            border: iced::border::Border {
                radius: 5.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
}

/// Camera / lens / exposure lines for the `I` overlay, shared with Cull.
/// Placeholders rather than stale values when the held metadata belongs to a
/// different image than the one on screen.
pub(crate) fn metadata_lines(editor: &RawEditor) -> [String; 3] {
    match editor.selected_metadata() {
        Some(m) => [
            format!("{} {}", m.make, m.model),
            m.lens.clone(),
            format!("ISO {}  {}  f/{}", m.iso, m.shutter_speed, m.aperture),
        ],
        None => [
            "---".to_string(),
            String::new(),
            "ISO ---  ---  f/---".to_string(),
        ],
    }
}

fn info_line<'a>(s: String) -> Element<'a, Message> {
    text(s)
        .size(12)
        .style(|_| text::Style { color: Some(Color::WHITE) })
        .into()
}

fn slider_row<'a, F>(
    editor: &'a RawEditor,
    label: &'a str,
    value: f32,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    on_change: F,
) -> Element<'a, Message>
where
    F: Fn(f32) -> Message + 'a,
{
    slider_field(editor, label, value, Some(default), range, step, format!("{:.2}", value), |v| v, on_change)
}

/// A slider for UI state rather than an edit (brush settings): no default
/// reset and no undo commit on release.
#[allow(clippy::too_many_arguments)]
fn slider_row_plain<'a, F>(
    editor: &'a RawEditor,
    label: &'a str,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    display: String,
    from_display: impl Fn(f32) -> f32 + 'a,
    on_change: F,
) -> Element<'a, Message>
where
    F: Fn(f32) -> Message + 'a,
{
    slider_field(editor, label, value, None, range, step, display, from_display, on_change)
}

/// Like `slider_row` but with an explicit value display string — used by the
/// WB sliders to show real Kelvin / Adobe-tint units while the underlying
/// slider stays a normalized offset around the as-shot anchor.
/// `from_display` reads a typed value in those units back into the slider's.
#[allow(clippy::too_many_arguments)]
fn slider_row_display<'a, F>(
    editor: &'a RawEditor,
    label: &'a str,
    value: f32,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    display: String,
    from_display: impl Fn(f32) -> f32 + 'a,
    on_change: F,
) -> Element<'a, Message>
where
    F: Fn(f32) -> Message + 'a,
{
    slider_field(editor, label, value, Some(default), range, step, display, from_display, on_change)
}

/// The text field a slider's value turns into while it is typed into. One
/// at a time, so one id.
pub fn slider_entry_id() -> text_input::Id {
    text_input::Id::new("develop-slider-entry")
}

/// Which slider a message comes from: its change message for a fixed value,
/// e.g. `MaskAdjustmentChanged(Contrast, 0.0)`. Unique where labels are not
/// ("Contrast", "Tint" and "Distortion" each appear twice in the sidebar).
fn slider_key(on_change: &impl Fn(f32) -> Message) -> String {
    format!("{:?}", on_change(0.0))
}

/// What the text field starts with: the value as shown, minus its unit.
fn entry_text(display: &str) -> String {
    display.trim_end_matches(|c: char| !c.is_ascii_digit()).to_string()
}

/// A typed slider value: a plain number, in the units the slider shows. A
/// decimal comma, a typographic minus and a trailing unit are all accepted.
fn parse_slider_entry(text: &str) -> Option<f32> {
    let cleaned = text.trim().replace(',', ".").replace('\u{2212}', "-");
    let number = cleaned.trim_end_matches(|c: char| c.is_alphabetic() || c == '°' || c == '%' || c.is_whitespace());
    number.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// One Develop slider: label, track and value. Click the value to type a
/// number (in the units it shows; Enter applies, Escape cancels).
/// Double-click the label or the track to reset it to `default`, as
/// Ctrl/Cmd+click on the track always has. `default: None` marks a tool
/// setting rather than an edit (brush size): no reset, and no undo step.
#[allow(clippy::too_many_arguments)]
fn slider_field<'a>(
    editor: &'a RawEditor,
    label: &'a str,
    value: f32,
    default: Option<f32>,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    display: String,
    from_display: impl Fn(f32) -> f32 + 'a,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    let grey = |_: &Theme| text::Style { color: Some(Color::from_rgb(0.7, 0.7, 0.7)) };
    let key = slider_key(&on_change);
    let commit = default.is_some();
    let (lo, hi) = (*range.start(), *range.end());

    let value_cell: Element<'a, Message> = match editor.slider_entry.as_ref().filter(|e| e.key == key) {
        Some(entry) => {
            let submit = match parse_slider_entry(&entry.text).map(&from_display) {
                Some(v) if v.is_finite() => {
                    Message::SliderSet { change: Box::new(on_change(v.clamp(lo, hi))), commit }
                }
                _ => Message::SliderEntryCancel,
            };
            text_input("", &entry.text)
                .id(slider_entry_id())
                .on_input(Message::SliderEntryChanged)
                .on_submit(submit)
                .size(13)
                .padding([1, 4])
                .width(Length::Fixed(52.0))
                .align_x(iced::alignment::Horizontal::Right)
                .style(ui::styles::text_input_style)
                .into()
        }
        None => mouse_area(
            text(display.clone())
                .width(Length::Fixed(52.0))
                .size(13)
                .align_x(iced::alignment::Horizontal::Right)
                .style(grey),
        )
        .on_press(Message::SliderEntryStart { key, text: entry_text(&display) })
        .interaction(iced::mouse::Interaction::Text)
        .into(),
    };

    let reset = default.map(|d| Message::SliderSet { change: Box::new(on_change(d)), commit: true });
    let mut track = slider(range, value, on_change)
        .step(step)
        .width(Length::Fill)
        .style(crate::ui::styles::ProSlider::style);
    if let Some(d) = default {
        track = track.default(d).on_release(Message::CommitEdit);
    }
    let label_and_track = row![text(label).width(Length::Fixed(90.0)).size(13).style(grey), track]
        .spacing(10)
        .align_y(iced::Alignment::Center);
    let label_and_track: Element<'a, Message> = match reset {
        Some(reset) => ui::widgets::double_click::double_click(label_and_track, reset).into(),
        None => label_and_track.into(),
    };

    row![label_and_track, value_cell]
        .spacing(10)
        .align_y(iced::Alignment::Center)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::update::update;
    use crate::core::types::{MASK_BRUSH, MASK_RANGE};

    #[test]
    fn typed_values_accept_what_people_type() {
        assert_eq!(parse_slider_entry("1.5"), Some(1.5));
        assert_eq!(parse_slider_entry(" +0,25 "), Some(0.25), "decimal comma");
        assert_eq!(parse_slider_entry("\u{2212}12°"), Some(-12.0), "typographic minus, unit");
        assert_eq!(parse_slider_entry("5600K"), Some(5600.0));
        assert_eq!(parse_slider_entry("40 %"), Some(40.0));
        assert_eq!(parse_slider_entry(""), None);
        assert_eq!(parse_slider_entry("abc"), None);
        assert_eq!(parse_slider_entry("inf"), None);
        assert_eq!(entry_text("5092K"), "5092");
        assert_eq!(entry_text("+12°"), "+12");
        assert_eq!(entry_text("0.33"), "0.33");
    }

    /// Labels repeat across the sidebar; the keys that tell sliders apart
    /// must not.
    #[test]
    fn slider_keys_tell_same_named_sliders_apart() {
        let global = slider_key(&Message::ContrastChanged);
        let mask = slider_key(&|v| Message::MaskAdjustmentChanged(crate::app::message::MaskAdjustment::Contrast, v));
        let lens = slider_key(&|v| Message::LensParamChanged(LensParam::Distortion, v));
        let profile = slider_key(&|v| Message::LensParamChanged(LensParam::ProfileDistortion, v));
        assert_ne!(global, mask);
        assert_ne!(lens, profile);
    }

    /// The WB sliders read typed Kelvin and tint back through the exact
    /// inverse of what they display.
    #[test]
    fn typed_kelvin_round_trips_through_the_display() {
        let as_k = 5092.0f32;
        let from_k = |k: f32| (1.0e6 / as_k.clamp(1500.0, 50000.0) - 1.0e6 / k.max(1.0)) / 120.0;
        for k in [3200.0f32, 5092.0, 6500.0] {
            let p = crate::core::types::EditParams { temperature: from_k(k), ..Default::default() };
            assert!((p.kelvin_from_anchor(as_k) - k).abs() < 0.5, "{k}");
        }
    }

    /// Building the sidebar in every new state must not panic (iced asserts
    /// on some widget arguments, e.g. slider ranges).
    #[test]
    fn develop_view_builds_in_every_tool_state() {
        let mut e = RawEditor::new().0;
        e.selected_image_id = Some(1);
        e.editor_readiness = EditorReadiness::Ready(1);
        e.collapsed_sections.clear();
        let _ = view_develop(&e);

        for kind in [0u32, 1, MASK_BRUSH, MASK_RANGE] {
            let _ = update(&mut e, Message::ToggleMaskPlacement(kind));
            if kind < 2 {
                let _ = update(&mut e, Message::MaskPlacementStarted(0.5, 0.5));
            }
            let _ = view_develop(&e);
            let _ = update(&mut e, Message::ToggleBrushArmed);
            let _ = update(&mut e, Message::ToggleMaskLumaRange(true));
            let _ = update(&mut e, Message::PickMaskColor);
            let _ = view_develop(&e);
        }
        e.current_edit_params.lens.profile = true;
        e.current_edit_params.curves.master.insert(0.3, 0.2);
        let _ = view_develop(&e);
        e.is_leveling = true;
        e.slider_entry = Some(crate::app::state::SliderEntry { key: slider_key(&Message::ExposureChanged), text: "1".into() });
        let _ = view_develop(&e);
    }
}
