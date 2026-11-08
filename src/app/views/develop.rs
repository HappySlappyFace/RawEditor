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
use crate::ui::theme;

/// The edit sidebar's background — also what folding sections fade from.
fn sidebar_background() -> Color {
    theme::colours().panel
}

/// The sidebar's width. Fixed for now; see the Develop sidebar notes in
/// CLAUDE.md before making it resizable (HiDPI).
const SIDEBAR_WIDTH: f32 = 300.0;

/// A slider row's label and value columns. Labels are SMALL monospace
/// (~7.2 px a character), so 80 px holds "Blue/Yellow"; what's left is track.
const LABEL_WIDTH: f32 = 80.0;
const VALUE_WIDTH: f32 = 44.0;

/// Build the Develop tab view (full-screen editor with preview)
pub fn view_develop(editor: &RawEditor) -> Element<'_, Message> {
    // Pinned histogram, the sections in between, pinned actions. Only the
    // middle scrolls, so the histogram and Export stay in view however far
    // down the user is.
    let sidebar_container = container(
        column![
            column![].push_maybe(sidebar_histogram(editor)),
            scrollable(view_sidebar(editor))
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new().width(4).scroller_width(4).margin(2),
                ))
                .height(Length::Fill),
            sidebar_actions(editor),
        ]
        .width(Length::Fixed(SIDEBAR_WIDTH))
        .height(Length::Fill),
    )
    .style(|_theme| container::Style {
        background: Some(Background::Color(sidebar_background())),
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

/// An icon with a hover label, for buttons too small to carry their name.
fn with_tooltip<'a>(content: impl Into<Element<'a, Message>>, label: &'a str) -> Element<'a, Message> {
    iced::widget::tooltip(
        content,
        container(text(label).size(theme::size::CAPTION)).padding([3, 6]).style(ui::styles::tooltip_style),
        iced::widget::tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// The pinned top of the sidebar: the histogram, when it is on.
fn sidebar_histogram(editor: &RawEditor) -> Option<Element<'_, Message>> {
    if !editor.histogram_enabled {
        return None;
    }
    let histogram = iced::widget::canvas::Canvas::new(crate::ui::histogram::Histogram {
        data: *editor.histogram_data.borrow(),
    })
    .width(Length::Fill)
    .height(Length::Fixed(96.0));
    let framed = container(histogram).padding(4).style(|_theme| container::Style {
        background: Some(Background::Color(theme::colours().surface)),
        border: Border { color: theme::colours().border, width: 1.0, radius: theme::radius::CONTROL.into() },
        ..Default::default()
    });
    Some(container(framed).padding(iced::Padding { top: 10.0, right: 12.0, bottom: 2.0, left: 12.0 }).into())
}

/// The pinned bottom of the sidebar: the actions on the whole edit, and
/// Export.
fn sidebar_actions(editor: &RawEditor) -> Element<'_, Message> {
    let icon = |glyph: &'static str, label: &'static str, active: bool, msg: Option<Message>| {
        with_tooltip(
            button(text(glyph).font(ICON_FONT).size(theme::size::BODY).align_x(iced::alignment::Horizontal::Center))
                .style(if active { ui::styles::AccentButton::style } else { ui::styles::NeutralButton::style })
                .on_press_maybe(msg)
                .padding([7, 0])
                .width(Length::Fixed(32.0)),
            label,
        )
    };
    let export = button(
        container(
            row![
                text(ui::icons::SAVE).font(ICON_FONT).size(theme::size::BODY),
                text("Export").size(theme::size::BODY),
            ]
            .spacing(theme::space::S)
            .align_y(Alignment::Center),
        )
        .center_x(Length::Fill),
    )
    .style(ui::styles::AccentButton::style)
    .on_press(Message::OpenExportModal)
    .padding([7, 0])
    .width(Length::Fill);
    let row = row![
        icon(
            ui::icons::HISTOGRAM,
            "Histogram",
            editor.histogram_enabled,
            Some(Message::HistogramToggled(!editor.histogram_enabled)),
        ),
        icon(ui::icons::COPY, "Copy settings", false, Some(Message::CopySettings)),
        icon(ui::icons::PASTE, "Paste settings", false, editor.edit_clipboard.as_ref().map(|_| Message::PasteSettings)),
        icon(ui::icons::RESET, "Reset all edits", false, Some(Message::ResetEdits)),
        export,
    ]
    .spacing(theme::space::XS)
    .align_y(Alignment::Center);
    container(row)
        .padding([10, 12])
        .width(Length::Fill)
        .into()
}

/// A foldable sidebar section: a header (fold arrow and title, an optional
/// control or readout, and a reset once the section holds edits) over a body
/// that folds away (`ui::widgets::collapse`).
///
/// Header and body are a single sidebar item, with the gap between them
/// inside the body: a folded section then adds no spacing of its own, and
/// the gap folds away with the body instead of snapping.
fn section<'a>(
    editor: &'a RawEditor,
    title: impl Into<String>,
    which: SidebarSection,
    trailing: Option<Element<'a, Message>>,
    body: Element<'a, Message>,
) -> Element<'a, Message> {
    let open = expanded(editor, which);
    let edited = crate::app::handlers::develop::section_edited(which, &editor.current_edit_params);
    let toggle = button(
        row![
            text(if open { ui::icons::CARET_DOWN } else { ui::icons::CARET_RIGHT })
                .font(ICON_FONT)
                .size(theme::size::SMALL)
                .width(Length::Fixed(10.0))
                .style(|_| text::Style { color: Some(theme::colours().text_muted) }),
            // The loupe's header sits inside Detail, so it reads one step down.
            text(title.into()).size(if which == SidebarSection::DetailPreview { theme::size::SMALL } else { theme::size::LABEL }),
        ]
        .spacing(theme::space::S)
        .align_y(Alignment::Center),
    )
    .style(ui::styles::ghost_button)
    .padding([5, 4])
    .width(Length::Fill)
    .on_press(Message::ToggleSection(which));

    let mut header = row![toggle].spacing(theme::space::XS).align_y(Alignment::Center);
    if let Some(t) = trailing {
        header = header.push(t);
    }
    // Masks have no section reset (it would delete them), and the loupe has
    // no edits of its own.
    if edited && !matches!(which, SidebarSection::Masks | SidebarSection::DetailPreview) {
        header = header.push(with_tooltip(
            button(text(ui::icons::RESET).font(ICON_FONT).size(theme::size::CAPTION))
                .style(ui::styles::ghost_button)
                .padding([5, 6])
                .on_press(Message::ResetSection(which)),
            "Reset this section",
        ));
    }

    let body = container(body).padding(iced::Padding { top: 4.0, bottom: 10.0, left: 4.0, right: 0.0 });
    column![
        header,
        crate::ui::widgets::collapse::collapse(open, body).veil(sidebar_background()),
    ]
    .into()
}

fn expanded(editor: &RawEditor, section: SidebarSection) -> bool {
    !editor.collapsed_sections.contains(&section)
}

/// A muted caption that labels a group of rows inside a section.
fn group_label<'a>(label: &'a str) -> Element<'a, Message> {
    container(ui::text::group_label(label))
        .padding(iced::Padding { top: 4.0, ..iced::Padding::ZERO })
        .into()
}

/// A column of rows at the sidebar's row spacing.
fn rows<'a>(items: impl IntoIterator<Item = Element<'a, Message>>) -> Element<'a, Message> {
    iced::widget::Column::with_children(items).spacing(theme::space::S).into()
}

fn view_sidebar(editor: &RawEditor) -> iced::widget::Column<'_, Message> {
    let p = &editor.current_edit_params;
    let d = crate::core::types::EditParams::default();

    let light = rows([
        slider_row(editor, "Exposure", p.exposure, d.exposure, -5.0..=5.0, 0.1, Message::ExposureChanged),
        slider_row(editor, "Contrast", p.contrast, d.contrast, -10.0..=10.0, 0.005, Message::ContrastChanged),
        slider_row(editor, "Highlights", p.highlights, d.highlights, -1.0..=1.0, 0.01, Message::HighlightsChanged),
        slider_row(editor, "Shadows", p.shadows, d.shadows, -1.0..=1.0, 0.01, Message::ShadowsChanged),
        slider_row(editor, "Whites", p.whites, d.whites, 0.8..=1.2, 0.01, Message::WhitesChanged),
        slider_row(editor, "Blacks", p.blacks, d.blacks, 0.0..=0.2, 0.005, Message::BlacksChanged),
    ]);

    let tone_mapping = rows([tone_mapper_hint(editor), tone_shape_controls(editor)]);

    // WB sliders pivot around the camera's as-shot values, shown in the
    // header; double-clicking either slider returns to them.
    let (as_k, as_t) = editor.as_shot_wb.unwrap_or((5000.0, 0.0));
    let kelvin = p.kelvin_from_anchor(as_k);
    let tint = p.tint_from_anchor(as_t);
    let colour = rows([
        slider_row_display(
            editor,
            "Temp",
            p.temperature,
            d.temperature,
            -1.0..=1.0,
            0.01,
            format!("{:.0}K", kelvin),
            // Inverse of `kelvin_from_anchor`, as the WB picker solves it.
            move |k| (1.0e6 / as_k.clamp(1500.0, 50000.0) - 1.0e6 / k.max(1.0)) / 120.0,
            Message::TemperatureChanged,
        ),
        slider_row_display(
            editor,
            "Tint",
            p.tint,
            d.tint,
            -1.0..=1.0,
            0.01,
            format!("{:+.0}", tint),
            move |t| (t - as_t) / 50.0,
            Message::TintChanged,
        ),
        slider_row(editor, "Vibrance", p.vibrance, d.vibrance, -1.0..=1.0, 0.01, Message::VibranceChanged),
        slider_row(editor, "Saturation", p.saturation, d.saturation, -100.0..=100.0, 1.0, Message::SaturationChanged),
    ]);
    // The picker lives in the header, so it works with Colour folded.
    let wb_picker = with_tooltip(
        button(text(ui::icons::EYEDROPPER).font(ICON_FONT).size(theme::size::SMALL))
            .style(if editor.is_wb_picking { ui::styles::AccentButton::style } else { ui::styles::ghost_button })
            .padding([4, 7])
            .on_press(Message::ToggleWbPicker),
        "Pick white balance",
    );
    let colour_header: Element<'_, Message> = row![
        text(format!("As shot {:.0}K", as_k))
            .size(theme::size::CAPTION)
            .style(|_| text::Style { color: Some(theme::colours().text_hint) }),
        wb_picker,
    ]
    .spacing(theme::space::S)
    .align_y(Alignment::Center)
    .into();

    let presence = rows([
        slider_row(editor, "Texture", p.texture, d.texture, -1.0..=1.0, 0.01, Message::TextureChanged),
        slider_row(editor, "Clarity", p.clarity, d.clarity, -1.0..=1.0, 0.01, Message::ClarityChanged),
        slider_row(editor, "Dehaze", p.dehaze, d.dehaze, -1.0..=1.0, 0.01, Message::DehazeChanged),
    ]);

    let detail = rows([
        section(editor, "Detail Preview", SidebarSection::DetailPreview, None, loupe_section(editor)),
        group_label("Noise reduction"),
        noise_reduction(editor),
        group_label("Sharpening"),
        slider_row(editor, "Amount", p.sharpening, d.sharpening, 0.0..=2.0, 0.01, Message::SharpeningChanged),
        slider_row(editor, "Masking", p.sharpen_masking, d.sharpen_masking, 0.0..=1.0, 0.01, Message::SharpenMaskingChanged),
    ]);

    let geometry = rows([
        geometry_tools(editor),
        slider_row_display(
            editor,
            "Straighten",
            p.rotation,
            d.rotation,
            -45.0..=45.0,
            0.05,
            format!("{:.1}°", p.rotation),
            |v| v,
            Message::RotationChanged,
        ),
        crop_presets(editor),
    ]);

    let lens = rows([lens_section(editor), group_label("Defringe"), defringe_section(editor)]);

    let mask_count = p.mask_count as usize;
    let masks_title = if mask_count > 0 { format!("Masks ({mask_count})") } else { "Masks".to_string() };

    let mut sidebar = column![
        container(camera_profile_block(editor)).padding(iced::Padding { bottom: 6.0, ..iced::Padding::ZERO }),
        section(editor, "Light", SidebarSection::Light, None, light),
        section(editor, "Tone Curve", SidebarSection::ToneCurve, None, curve_section(editor)),
        section(editor, "Tone Mapping", SidebarSection::ToneMapping, Some(tone_mapper_selector(editor)), tone_mapping),
        section(editor, "Colour", SidebarSection::Colour, Some(colour_header), colour),
        section(editor, "Presence", SidebarSection::Presence, None, presence),
        section(editor, "Detail", SidebarSection::Detail, None, detail),
        section(editor, "Geometry", SidebarSection::Geometry, None, geometry),
        section(editor, "Lens", SidebarSection::Lens, None, lens),
        section(editor, masks_title, SidebarSection::Masks, None, mask_section(editor)),
    ];

    if crate::debug::SHOW_SENSOR_CORRECTION {
        sidebar = sidebar.push(sensor_correction(editor));
    }

    sidebar.spacing(theme::space::XXS).padding(iced::Padding { top: 10.0, right: 12.0, bottom: 12.0, left: 8.0 })
}

/// Debug-only black-level controls (`debug::SHOW_SENSOR_CORRECTION`).
fn sensor_correction(editor: &RawEditor) -> Element<'_, Message> {
    let p = &editor.current_edit_params;
    let mut col = column![
        group_label("Sensor correction"),
        checkbox("Shift Grid X", p.black_phase_x != 0)
            .on_toggle(|checked| Message::BlackPhaseChanged(false, if checked { 1 } else { 0 })),
        checkbox("Shift Grid Y", p.black_phase_y != 0)
            .on_toggle(|checked| Message::BlackPhaseChanged(true, if checked { 1 } else { 0 })),
    ]
    .spacing(theme::space::S);
    for (i, name) in ["Black TL (Red)", "Black TR (Green)", "Black BL (Green)", "Black BR (Blue)"].into_iter().enumerate() {
        col = col
            .push(text(format!("{name}: {:.1}", p.black_offsets[i])).size(theme::size::SMALL))
            .push(
                slider(-50.0..=50.0, p.black_offsets[i], move |v| Message::BlackOffsetChanged(i, v))
                    .step(0.1_f32)
                    .style(ui::styles::ProSlider::style),
            );
    }
    col.into()
}

/// The crop tool (also on `C`), then the centred aspect presets.
fn crop_presets(editor: &RawEditor) -> Element<'_, Message> {
    let tool = button(
        container(
            row![
                text(ui::icons::CROP).font(ICON_FONT).size(theme::size::SMALL),
                text(if editor.is_cropping { "Done" } else { "Crop Tool" }).size(theme::size::SMALL),
            ]
            .spacing(theme::space::S)
            .align_y(Alignment::Center),
        )
        .center_x(Length::Fill),
    )
    .style(if editor.is_cropping { ui::styles::AccentButton::style } else { ui::styles::NeutralButton::style })
    .on_press(Message::ToggleCrop)
    .padding([4, 0])
    .width(Length::Fill);
    column![
        row![
            text("Crop")
                .width(Length::Fixed(LABEL_WIDTH))
                .size(theme::size::SMALL)
                .style(|_| text::Style { color: Some(theme::colours().text_secondary) }),
            tool,
        ]
        .spacing(theme::space::XS)
        .align_y(Alignment::Center),
        row![
            iced::widget::Space::with_width(Length::Fixed(LABEL_WIDTH)),
            aspect_crop_button(editor, "1:1", 1.0),
            aspect_crop_button(editor, "16:9", 16.0 / 9.0),
            aspect_crop_button(editor, "2:3", 2.0 / 3.0),
            button(text("Full").size(theme::size::CAPTION).align_x(iced::alignment::Horizontal::Center))
                .style(ui::styles::NeutralButton::style)
                .on_press(Message::SetCrop([0.0, 0.0, 1.0, 1.0]))
                .padding([4, 0])
                .width(Length::Fill),
        ]
        .spacing(theme::space::XS),
    ]
    .spacing(theme::space::XS)
    .into()
}

fn geometry_tools(editor: &RawEditor) -> Element<'_, Message> {
    let ready = editor
        .image_resources
        .as_ref()
        .is_some_and(|r| Some(r.image_id) == editor.selected_image_id);
    let turn = |icon: &'static str, label: &'static str, clockwise: bool| {
        with_tooltip(
            button(text(icon).font(ICON_FONT).size(theme::size::BODY))
                .style(ui::styles::NeutralButton::style)
                .on_press_maybe(ready.then_some(Message::TurnImage { clockwise }))
                .padding([4, 12]),
            label,
        )
    };
    let level = button(
        row![
            text(ui::icons::LEVEL).font(ICON_FONT).size(theme::size::SMALL),
            // Armed shows as the accent style; the status line says what to do.
            text("Level").size(theme::size::SMALL),
        ]
        .spacing(theme::space::S)
        .align_y(Alignment::Center),
    )
    .style(if editor.is_leveling { ui::styles::AccentButton::style } else { ui::styles::NeutralButton::style })
    .on_press_maybe(ready.then_some(Message::ToggleLevelTool))
    .padding([4, 10]);
    row![
        text("Rotate")
            .width(Length::Fixed(LABEL_WIDTH))
            .size(theme::size::SMALL)
            .style(|_| text::Style { color: Some(theme::colours().text_secondary) }),
        turn(ui::icons::ROTATE_LEFT, "Rotate left", false),
        turn(ui::icons::ROTATE_RIGHT, "Rotate right", true),
        iced::widget::Space::with_width(Length::Fill),
        level,
    ]
    .spacing(theme::space::XS)
    .align_y(Alignment::Center)
    .into()
}

/// A centred crop preset. The crop rectangle lives in display (EXIF-oriented)
/// space, so the ratio must be solved against the oriented dimensions — the
/// raw sensor dims are landscape even for a portrait shot, which turned
/// "16:9" into a tall sliver on every vertical frame.
fn aspect_crop_button<'a>(editor: &RawEditor, label: &'a str, ratio: f32) -> Element<'a, Message> {
    button(text(label).size(theme::size::CAPTION).align_x(iced::alignment::Horizontal::Center))
        .style(ui::styles::NeutralButton::style)
        .padding([4, 0])
        .width(Length::Fill)
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
                        .size(theme::size::LABEL)
                        .style(|_| text::Style {
                            color: Some(Color::WHITE)
                        }),
                    text(" Loading RAW…").size(theme::size::LABEL).style(|_| text::Style {
                        color: Some(Color::WHITE)
                    })
                ]]
                .padding(8),
            )
            .style(|_theme| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
                border: Border {
                    radius: theme::radius::CONTROL.into(),
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
                ui::text::empty_state("Preview failed", ui::text::tinted(error.clone(), theme::size::BODY, theme::colours().error))
                    .padding(40),
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
        return container(ui::text::empty_state("No image selected", ui::text::body("Pick a photo in Library to start editing.")))
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
                    ai_select: (!peeking && editor.ai_select.as_ref().is_some_and(|s| s.engine.is_some()))
                        .then(|| crate::app::handlers::ai_mask::display_points(editor)),
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
                                .size(theme::size::CAPTION)
                                .font(theme::font::BOLD)
                                .style(|_theme| text::Style { color: Some(theme::colours().text_strong) }),
                        )
                        .padding([4, 10])
                        .style(|_theme| container::Style {
                            background: Some(Background::Color(theme::colours().accent)),
                            border: Border { radius: theme::radius::CONTROL.into(), ..Default::default() },
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
                    text("CACHE").size(theme::size::SMALL).style(|_| text::Style {
                        color: Some(theme::colours().accent)
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
                .spacing(theme::space::XXS)
                .into(),
            )
            .into()
        }
        crate::app::state::InfoOverlayState::Metadata => {
            let mut lines = column![
                // 100% = one sensor pixel per screen pixel, not "fitted".
                text(format!("Zoom {:.0}%", editor.zoom_percent()))
                    .size(theme::size::SMALL)
                    .style(|_| text::Style {
                        color: Some(theme::colours().accent)
                    }),
            ]
            .spacing(theme::space::XXS);
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
        .size(theme::size::SMALL)
        .style(|_| text::Style {
            color: Some(Color::WHITE),
        }),
    )
    .padding(5)
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.4))),
        border: iced::border::Border {
            radius: theme::radius::CONTROL.into(),
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
            background: Some(Background::Color(theme::colours().surface_raised)),
            ..Default::default()
        })
        .into()
}

/// "Local Masks" sidebar section: add-mask buttons, the mask list, and the
/// per-mask adjustment sliders for the selected mask.
fn mask_section(editor: &RawEditor) -> Element<'_, Message> {
    let sidebar = mask_rows(column![].spacing(theme::space::S), editor);
    sidebar.into()
}

/// The Masks section's rows: add buttons, the mask list, then the selected
/// mask's settings.
fn mask_rows<'a>(
    mut sidebar: iced::widget::Column<'a, Message>,
    editor: &'a RawEditor,
) -> iced::widget::Column<'a, Message> {
    use crate::app::message::{BrushSetting, MaskAdjustment};
    use crate::app::state::MaskTool;
    use crate::core::types::{MASK_AI, MASK_BRUSH, MASK_LINEAR, MASK_RADIAL, MASK_RANGE, MAX_MASKS};

    let params = &editor.current_edit_params;
    let count = params.mask_count as usize;

    // Add-mask buttons. Linear/radial toggle placement mode (the next
    // click-drag on the preview places the mask); brush and range masks have
    // no geometry and are created straight away. Disabled at the mask limit,
    // except to cancel a mode.
    let can_add = count < MAX_MASKS;
    let linear_active = editor.mask_tool == MaskTool::PlacingLinear;
    let radial_active = editor.mask_tool == MaskTool::PlacingRadial;
    let add_button = |label: &'static str, active: bool, msg: Option<Message>| {
        button(text(label).size(theme::size::CAPTION).align_x(iced::alignment::Horizontal::Center))
            .style(if active {
                ui::styles::AccentButton::style
            } else {
                ui::styles::NeutralButton::style
            })
            .on_press_maybe(msg)
            .padding([5, 0])
            .width(Length::Fill)
    };
    let place = |kind: u32, active: bool| (can_add || active).then_some(Message::ToggleMaskPlacement(kind));
    sidebar = sidebar.push(
        row![
            add_button("Linear", linear_active, place(0, linear_active)),
            add_button("Radial", radial_active, place(1, radial_active)),
            add_button("Brush", false, place(MASK_BRUSH, false)),
            add_button("Range", false, place(MASK_RANGE, false)),
            add_button(
                "AI",
                editor.ai_select.is_some(),
                (can_add && editor.ai_select.is_none()).then_some(Message::AiSelectStart),
            ),
        ]
        .spacing(theme::space::XS),
    );
    if linear_active || radial_active {
        sidebar = sidebar.push(hint("Drag on the image to place the mask.".to_string()));
    }

    // An open AI selection owns the canvas: say what clicks do, and how to
    // finish.
    if let Some(s) = &editor.ai_select {
        let status = if let Some(e) = &s.error {
            format!("Unavailable: {e}")
        } else if s.analysing() {
            "Analysing the image…".to_string()
        } else if s.prompts.is_empty() {
            "Click the subject to select it.".to_string()
        } else {
            "Click to add · right-click or Alt-click to subtract · drag a box.".to_string()
        };
        sidebar = sidebar.push(
            container(
                column![
                    text("AI Select").size(theme::size::SMALL).font(theme::font::BOLD),
                    text(status).size(theme::size::CAPTION).style(|_| text::Style { color: Some(theme::colours().text_secondary) }),
                    row![
                        button(text("Cancel").size(theme::size::SMALL))
                            .style(ui::styles::NeutralButton::style)
                            .on_press(Message::AiSelectCancel)
                            .width(Length::Fill),
                        button(text("Done").size(theme::size::SMALL))
                            .style(ui::styles::AccentButton::style)
                            .on_press(Message::AiSelectDone)
                            .width(Length::Fill),
                    ]
                    .spacing(5),
                ]
                .spacing(theme::space::S),
            )
            .padding(8)
            .style(|_| container::Style {
                border: Border { color: theme::colours().accent, width: 1.0, radius: 0.0.into() },
                ..Default::default()
            }),
        );
    }

    // Mask list: select / enable / delete
    for i in 0..count {
        let m = params.masks[i];
        let selected = editor.selected_mask == Some(i);
        let kind = match m.mask_type {
            MASK_LINEAR => "Linear",
            MASK_RADIAL => "Radial",
            MASK_BRUSH => "Brush",
            MASK_AI => "AI",
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
                button(text(label).size(theme::size::SMALL))
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
                button(text(ui::icons::TIMES).font(ICON_FONT).size(theme::size::SMALL))
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
        checkbox(label, on).size(14).text_size(theme::size::SMALL).style(ui::styles::checkbox_style)
    };
    sidebar = sidebar.push(
        row![
            small("Invert", m.invert != 0).on_toggle(Message::ToggleMaskInvert),
            small("Show overlay", editor.show_mask_overlay).on_toggle(Message::ToggleMaskOverlay),
        ]
        .spacing(theme::space::XL),
    );
    if m.mask_type == MASK_AI && editor.ai_select.is_none() {
        sidebar = sidebar.push(
            button(text("Refine Selection").size(theme::size::CAPTION))
                .style(ui::styles::NeutralButton::style)
                .on_press(Message::AiSelectRefine(i)),
        );
    }

    // Brush: painting is the whole of a brush mask, and a refinement
    // (add / erase) for any other.
    let b = editor.brush_settings;
    let is_brush = m.mask_type == MASK_BRUSH;
    let mut brush_row = row![].spacing(theme::space::XS).align_y(Alignment::Center);
    if !is_brush {
        brush_row = brush_row.push(
            button(text(if editor.brush_armed { "Brushing" } else { "Brush" }).size(theme::size::CAPTION))
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
                button(text(label).size(theme::size::CAPTION))
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
            button(text("Clear Brush").size(theme::size::CAPTION))
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
            small("Colour range", m.color_range != 0 || picking).on_toggle(Message::ToggleMaskColorRange),
            iced::widget::Space::with_width(Length::Fill),
            button(text(if picking { "Picking…" } else { "Pick Colour" }).size(theme::size::CAPTION))
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
        .push(text("Adjustments").size(theme::size::SMALL))
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

/// The noise sliders, or, with AI denoise applied, just AI denoise: it
/// replaces them (`DenoiseSettings::from_params`), so they would do nothing.
/// Their values are kept for when it is removed.
fn noise_reduction(editor: &RawEditor) -> Element<'_, Message> {
    let p = &editor.current_edit_params;
    let d = crate::core::types::EditParams::default();
    if p.ai_denoise > 0.0 {
        return rows([
            ai_denoise_row(editor),
            hint("AI denoise replaces the noise sliders. Remove it in AI Denoise to use them again.".to_string()),
        ]);
    }
    rows([
        slider_row(editor, "Luminance", p.luma_noise, d.luma_noise, 0.0..=1.0, 0.01, Message::LumaNoiseChanged),
        slider_row(editor, "Detail", p.luma_detail, d.luma_detail, 0.0..=1.0, 0.01, Message::LumaDetailChanged),
        slider_row(editor, "Colour", p.color_noise, d.color_noise, 0.0..=1.0, 0.01, Message::ColorNoiseChanged),
        ai_denoise_row(editor),
    ])
}

/// `AI Denoise…` and, when applied, how much.
fn ai_denoise_row(editor: &RawEditor) -> Element<'_, Message> {
    let amount = editor.current_edit_params.ai_denoise;
    let label = if amount > 0.0 { format!("AI Denoise · {:.0}%", amount * 100.0) } else { "AI Denoise…".to_string() };
    button(
        row![text(ui::icons::MAGIC).font(ICON_FONT).size(theme::size::SMALL), text(label).size(theme::size::SMALL)]
            .spacing(theme::space::M)
            .align_y(Alignment::Center),
    )
    .style(if amount > 0.0 { ui::styles::AccentButton::style } else { ui::styles::NeutralButton::style })
    .on_press(Message::OpenAiDenoise)
    .width(Length::Fill)
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
            iced::widget::canvas::Canvas::new(ui::loupe::LoupeDrag { on_pan: Message::LoupePan })
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
        background: Some(Background::Color(theme::colours().surface)),
        border: Border { color: theme::colours().border_strong, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    });

    let targeting = editor.loupe.targeting;
    let pick = button(
        row![
            text(ui::icons::CROSSHAIRS).font(ICON_FONT).size(theme::size::SMALL), // crosshairs
            text(if targeting { "Click the image…" } else { "Pick Point" }).size(theme::size::CAPTION),
        ]
        .spacing(theme::space::S)
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
    .spacing(theme::space::S)
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
        text("Profile").size(theme::size::BODY).width(Length::Fixed(60.0)),
        iced::widget::pick_list(options, selected, |o: ProfileOption| Message::CameraProfileSelected(o.choice))
            .text_size(12)
            .padding([4, 8])
            .width(Length::Fill)
            .style(ui::styles::pick_list_style)
            .menu_style(ui::styles::pick_list_menu_style),
    ]
    .spacing(theme::space::M)
    .align_y(Alignment::Center)
    .into()
}

/// No camera profile for this camera: say what is being used instead, and
/// offer RawTherapee's open-source profile or an import.
fn missing_profile_prompt(editor: &RawEditor, camera: String) -> Element<'_, Message> {
    use crate::app::state::ProfileJob;
    let state = &editor.camera_profiles;
    let unavailable = state.unavailable.as_deref() == Some(camera.as_str());
    let warn = theme::colours().warn;

    let download = button(
        row![
            text(ui::icons::DOWNLOAD).font(ICON_FONT).size(theme::size::SMALL), // download
            text(if state.busy == Some(ProfileJob::Downloading) { "Downloading…" } else { "Download" }).size(theme::size::SMALL),
        ]
        .spacing(theme::space::S)
        .align_y(Alignment::Center),
    )
    .style(ui::styles::AccentButton::style)
    .on_press_maybe((state.busy.is_none() && !unavailable).then_some(Message::DownloadCameraProfile))
    .padding([5, 10]);
    let import = button(text("Import…").size(theme::size::SMALL))
        .style(ui::styles::NeutralButton::style)
        .on_press_maybe(state.busy.is_none().then_some(Message::ImportCameraProfiles))
        .padding([5, 10]);

    let mut col = column![
        text("No DCP profile found").size(theme::size::BODY),
        hint(format!(
            "{camera} is using the default colour pipeline. Would you like to download an open-source profile for it?"
        )),
        row![download, import].spacing(theme::space::S),
    ]
    .spacing(theme::space::S);
    if unavailable {
        col = col.push(
            text(format!("The RawTherapee project has no profile for {camera}. You can import one of your own instead."))
                .size(theme::size::CAPTION)
                .style(move |_theme| text::Style { color: Some(warn) }),
        );
    } else if let Some(Err(e)) = &state.notice {
        col = col.push(text(e.clone()).size(theme::size::CAPTION).style(move |_theme| text::Style { color: Some(warn) }));
    }
    container(col)
        .padding(10)
        .width(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(Background::Color(theme::colours().surface_raised)),
            border: Border { color: theme::colours().accent_selection_border, width: 1.0, radius: theme::radius::CONTROL.into() },
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
            text(ui::icons::DOWNLOAD).font(ICON_FONT).size(theme::size::BODY), // download
            text(if downloading { "Downloading…" } else { "Download Lens Profiles" }).size(theme::size::SMALL),
        ]
        .spacing(theme::space::S)
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
    .spacing(theme::space::S);
    if let Some(Err(e)) = &editor.lensfun.installed {
        col = col.push(
            text(format!("Download failed: {e}"))
                .size(theme::size::CAPTION)
                .style(|_theme| text::Style { color: Some(theme::colours().warn) }),
        );
    }
    col.into()
}

fn hint<'a>(s: String) -> Element<'a, Message> {
    ui::text::hint(s).into()
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
        text(lens_name).size(theme::size::SMALL),
        status,
        checkbox("Enable profile corrections", l.profile)
            .on_toggle_maybe(has_profile.then_some(|on| Message::LensToggled(LensToggle::Profile, on)))
            .size(14)
            .text_size(theme::size::SMALL)
            .style(ui::styles::checkbox_style),
        checkbox("Remove chromatic aberration", l.profile_ca)
            .on_toggle_maybe(has_profile.then_some(|on| Message::LensToggled(LensToggle::ProfileCa, on)))
            .size(14)
            .text_size(theme::size::SMALL)
            .style(ui::styles::checkbox_style),
    ]
    .spacing(theme::space::S);
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
    col.push(group_label("Manual"))
        .push(slider_row(editor, "Distortion", l.distortion, d.distortion, -1.0..=1.0, 0.01, |v| {
            Message::LensParamChanged(LensParam::Distortion, v)
        }))
        .push(slider_row(editor, "Vignetting", l.vignetting, d.vignetting, -1.0..=1.0, 0.01, |v| {
            Message::LensParamChanged(LensParam::Vignetting, v)
        }))
        .push(slider_row(editor, "Red/Cyan", l.ca_red, d.ca_red, -1.0..=1.0, 0.01, |v| {
            Message::LensParamChanged(LensParam::CaRed, v)
        }))
        .push(slider_row(editor, "Blue/Yellow", l.ca_blue, d.ca_blue, -1.0..=1.0, 0.01, |v| {
            Message::LensParamChanged(LensParam::CaBlue, v)
        }))
        .push(
            checkbox("Constrain (no empty borders)", l.constrain)
                .on_toggle(|on| Message::LensToggled(LensToggle::Constrain, on))
                .size(14)
                .text_size(theme::size::SMALL)
                .style(ui::styles::checkbox_style),
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
    .spacing(theme::space::S)
    .into()
}

/// Channel tabs, the curve canvas and a reset for the shown channel.
fn curve_section(editor: &RawEditor) -> Element<'_, Message> {
    use crate::core::curves::CurveChannel;
    let mut tabs = row![].spacing(theme::space::XS).align_y(Alignment::Center);
    for ch in CurveChannel::ALL {
        let edited = !editor.current_edit_params.curves.get(ch).is_identity();
        let label = if edited { format!("{}•", ch.label()) } else { ch.label().to_string() };
        tabs = tabs.push(
            button(text(label).size(theme::size::CAPTION))
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
        button(text(ui::icons::RESET).font(ICON_FONT).size(theme::size::CAPTION))
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
    column![tabs, canvas].spacing(theme::space::S).into()
}

/// Operator picker, in the Tone Mapping header so it stays reachable with
/// the section folded. The hint under it says what the choice does.
fn tone_mapper_selector(editor: &RawEditor) -> Element<'_, Message> {
    use crate::core::tonemap::ToneMapper;
    iced::widget::pick_list(ToneMapper::ALL, Some(editor.current_edit_params.tone_mapper), Message::ToneMapperChanged)
        .text_size(theme::size::SMALL)
        .padding([3, 8])
        .width(Length::Fixed(112.0))
        .style(ui::styles::pick_list_style)
        .menu_style(ui::styles::pick_list_menu_style)
        .into()
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
        .size(theme::size::CAPTION)
        .style(|_theme| text::Style {
            color: Some(theme::colours().text_hint),
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
            "Strength",
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
        .spacing(theme::space::S)
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
                radius: theme::radius::CONTROL.into(),
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
        .size(theme::size::SMALL)
        .style(|_| text::Style { color: Some(theme::colours().text_strong) })
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
/// The colours a slider's rail shows, left to right, for the sliders whose
/// effect has an obvious picture (`ui::widgets::slider`). Keyed by the change
/// message, like `slider_key`, so one table covers the global and mask
/// sliders alike. These are the colours of what the slider DOES, not theme
/// colours, so they stay the same in any theme.
fn rail_gradient(change: &Message) -> Option<&'static [Color]> {
    use crate::app::message::MaskAdjustment as M;
    use std::sync::LazyLock;
    const DARK_TO_LIGHT: &[Color] = &[Color::from_rgb(0.08, 0.08, 0.08), Color::from_rgb(0.82, 0.82, 0.82)];
    const COOL_TO_WARM: &[Color] = &[Color::from_rgb(0.27, 0.47, 0.86), Color::from_rgb(0.93, 0.78, 0.30)];
    const GREEN_TO_MAGENTA: &[Color] = &[Color::from_rgb(0.33, 0.70, 0.36), Color::from_rgb(0.80, 0.36, 0.76)];
    const GREY_TO_COLOUR: &[Color] = &[Color::from_rgb(0.42, 0.42, 0.42), Color::from_rgb(0.88, 0.32, 0.24)];
    // Defringe hue sliders show the hues they select, over their own ranges.
    static PURPLE_HUES: LazyLock<Vec<Color>> = LazyLock::new(|| hue_strip(200.0, 360.0));
    static GREEN_HUES: LazyLock<Vec<Color>> = LazyLock::new(|| hue_strip(40.0, 180.0));
    Some(match change {
        Message::ExposureChanged(_) | Message::MaskAdjustmentChanged(M::Exposure, _) => DARK_TO_LIGHT,
        Message::TemperatureChanged(_) | Message::MaskAdjustmentChanged(M::Warmth, _) => COOL_TO_WARM,
        Message::TintChanged(_) | Message::MaskAdjustmentChanged(M::Tint, _) => GREEN_TO_MAGENTA,
        Message::VibranceChanged(_) | Message::SaturationChanged(_) | Message::MaskAdjustmentChanged(M::Saturation, _) => {
            GREY_TO_COLOUR
        }
        Message::DefringeChanged(DefringeParam::PurpleHue, _) => PURPLE_HUES.as_slice(),
        Message::DefringeChanged(DefringeParam::GreenHue, _) => GREEN_HUES.as_slice(),
        _ => return None,
    })
}

/// Evenly spaced, slightly muted hues from `from`° to `to`°.
fn hue_strip(from: f32, to: f32) -> Vec<Color> {
    (0..=8)
        .map(|i| {
            let h = (from + (to - from) * i as f32 / 8.0).rem_euclid(360.0) / 60.0;
            let x = 1.0 - (h % 2.0 - 1.0).abs();
            let (r, g, b) = match h as u32 {
                0 => (1.0, x, 0.0),
                1 => (x, 1.0, 0.0),
                2 => (0.0, 1.0, x),
                3 => (0.0, x, 1.0),
                4 => (x, 0.0, 1.0),
                _ => (1.0, 0.0, x),
            };
            // Value 0.8, saturation 0.7: vivid, but not neon on the panel.
            let m = |c: f32| 0.8 * (1.0 - 0.7 * (1.0 - c));
            Color::from_rgb(m(r), m(g), m(b))
        })
        .collect()
}

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
    let grey = |_: &Theme| text::Style { color: Some(theme::colours().text_secondary) };
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
                .size(theme::size::SMALL)
                .padding([1, 4])
                .width(Length::Fixed(VALUE_WIDTH))
                .align_x(iced::alignment::Horizontal::Right)
                .style(ui::styles::text_input_style)
                .into()
        }
        None => mouse_area(
            text(display.clone())
                .width(Length::Fixed(VALUE_WIDTH))
                .size(theme::size::SMALL)
                .align_x(iced::alignment::Horizontal::Right)
                .style(grey),
        )
        .on_press(Message::SliderEntryStart { key, text: entry_text(&display) })
        .interaction(iced::mouse::Interaction::Text)
        .into(),
    };

    let reset = default.map(|d| Message::SliderSet { change: Box::new(on_change(d)), commit: true });
    let gradient = rail_gradient(&on_change(0.0));
    let mut track = ui::widgets::slider::slider(range, value, on_change)
        .step(step)
        .width(Length::Fill)
        .style(crate::ui::styles::ProSlider::style);
    if let Some(stops) = gradient {
        track = track.gradient(stops);
    }
    if let Some(d) = default {
        track = track.default(d).on_release(Message::CommitEdit);
    }
    let label_and_track = row![text(label).width(Length::Fixed(LABEL_WIDTH)).size(theme::size::SMALL).style(grey), track]
        .spacing(theme::space::S)
        .align_y(iced::Alignment::Center);
    let label_and_track: Element<'a, Message> = match reset {
        Some(reset) => ui::widgets::double_click::double_click(label_and_track, reset).into(),
        None => label_and_track.into(),
    };

    row![label_and_track, value_cell]
        .spacing(theme::space::S)
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
        // The whole sidebar, unoptimised, nests deeper than a test thread's
        // default 2 MB stack (CI aborts with a stack overflow), so build it
        // on a thread with room — the app's main thread has 8 MB.
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(develop_view_builds_in_every_tool_state_body)
            .expect("spawn test thread")
            .join()
            .expect("develop view build panicked");
    }

    fn develop_view_builds_in_every_tool_state_body() {
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
