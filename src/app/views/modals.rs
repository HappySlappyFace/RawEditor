use iced::widget::{
    checkbox, column, container, radio, row, scrollable, slider, stack, text, Space,
    text_input,
};
use crate::ui::widgets::button;
use iced::{Alignment, Element, Length, Theme};

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::ui;
use crate::ui::theme;

/// Wrap a modal's body in a dimmed, click-to-dismiss, screen-centred overlay
/// that animates in and out (`ui::widgets::modal`). `key` identifies the
/// dialog; `exiting` is set while a closed one plays its exit.
///
/// The two-container split is load-bearing. `iced::widget::stack` resolves its
/// size from child 0 only and then places every later layer at that layer's own
/// intrinsic size, at the origin — there is no alignment step. A shrink-wrapped
/// card therefore pins to the top-left corner of the window, which is exactly
/// how every modal used to render.
///
/// So the fill + centering must live on an OUTER container, and the card
/// surface on the inner `Sheet`, which shrink-wraps its content.
pub fn modal_overlay<'a>(
    content: Element<'a, Message>,
    key: u64,
    exiting: bool,
) -> Element<'a, Message> {
    use crate::ui::widgets::modal::{scrim, sheet};

    // The card. Uses the shared surface rather than its own colours so there
    // is one modal look in the app, not two that drift apart.
    let card = sheet(key, exiting, ui::styles::modal_surface(), content);

    stack![
        // The backdrop: dims the app, closes on click.
        scrim(Message::CloseModal, exiting),
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
    ]
    .into()
}

pub fn view_help_modal<'a>() -> Element<'a, Message> {
    let shortcut = |key: &str, desc: &str| {
        row![
            text(key.to_string())
                .font(theme::font::BOLD)
                .width(Length::Fixed(140.0))
                .size(theme::size::BODY)
                .style(|_| text::Style {
                    color: Some(theme::colours().text_secondary)
                }),
            text(desc.to_string()).size(theme::size::BODY).style(|_| text::Style {
                color: Some(theme::colours().text_muted)
            })
        ]
        .spacing(theme::space::M)
    };

    column![
        ui::text::title("Keyboard Shortcuts"),
        iced::widget::horizontal_rule(1).style(ui::styles::rule),
        scrollable(column![
            text("Navigation")
                .size(theme::size::SMALL)
                .font(theme::font::BOLD)
                .style(|_| text::Style {
                    color: Some(theme::colours().text_muted)
                }),
            shortcut("Arrow Keys", "Previous / Next Image"),
            shortcut("Space (hold)", "Compare with the original — Develop only"),
            shortcut("Esc", "Close dialog, or cancel the active tool"),
            text("Selection")
                .size(theme::size::SMALL)
                .font(theme::font::BOLD)
                .style(|_| text::Style {
                    color: Some(theme::colours().text_muted)
                }),
            shortcut("Ctrl + Click", "Add / remove one image"),
            shortcut("Shift + Click", "Select a range"),
            shortcut("Ctrl + A", "Select everything visible"),
            text("Rating & Culling")
                .size(theme::size::SMALL)
                .font(theme::font::BOLD)
                .style(|_| text::Style {
                    color: Some(theme::colours().text_muted)
                }),
            shortcut("0 - 5", "Star Rating"),
            shortcut("P", "Pick"),
            shortcut("X", "Reject"),
            shortcut("U", "Unflag"),
            shortcut("Delete / Backspace", "Delete Image (or unmark if already marked)"),
            shortcut("D", "Mark for Removal (in Delete dialog)"),
            shortcut("Enter", "Confirm the open dialog"),
            text("Editing")
                .size(theme::size::SMALL)
                .font(theme::font::BOLD)
                .style(|_| text::Style {
                    color: Some(theme::colours().text_muted)
                }),
            shortcut("R", "Reset Edits"),
            shortcut("Ctrl + Z", "Undo"),
            shortcut("Ctrl + Shift + Z", "Redo"),
            shortcut("Ctrl + Y", "Redo (alternative)"),
            shortcut("Ctrl + C", "Copy Settings (choose what)"),
            shortcut("Ctrl + Shift + C", "Copy All Settings"),
            shortcut("Ctrl + V", "Paste Settings"),
            text("View & Tools")
                .size(theme::size::SMALL)
                .font(theme::font::BOLD)
                .style(|_| text::Style {
                    color: Some(theme::colours().text_muted)
                }),
            shortcut("C", "Crop mode — Develop only"),
            shortcut("W", "White balance eyedropper — Develop only"),
            shortcut("I", "Cycle the info overlay"),
            shortcut("F3", "Performance HUD"),
            shortcut("F1 / ?", "This dialog"),
            text("Mouse")
                .size(theme::size::SMALL)
                .font(theme::font::BOLD)
                .style(|_| text::Style {
                    color: Some(theme::colours().text_muted)
                }),
            shortcut("Double Click", "Reset zoom and pan"),
            shortcut("Click slider value", "Type a number — Enter applies, Esc cancels"),
            shortcut("Dbl click slider", "Reset it (or Ctrl + click)"),
            shortcut("Scroll", "Zoom in Develop, scroll elsewhere"),
        ]
        .spacing(theme::space::S))
        // The list outgrew a short window once View & Tools and Selection were
        // added; cap it so the Close button stays reachable.
        .height(Length::Fixed(420.0)),
        button("Close")
            .on_press(Message::CloseModal)
            .padding(8)
            .width(Length::Fill)
            .style(ui::styles::NeutralButton::style)
    ]
    .spacing(theme::space::XL)
    .width(420)
    .into()
}

pub fn view_export_modal<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    let settings = &editor.export_settings;

    let format_section = column![
        text("File Format")
            .size(theme::size::SMALL)
            .font(theme::font::BOLD)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
        row![
            radio(
                "JPEG",
                crate::app::state::ExportFormat::Jpeg,
                Some(settings.format),
                Message::SetExportFormat
            )
            .size(theme::size::LABEL)
            .spacing(5)
            .text_size(13)
            .style(ui::styles::radio_style),
            radio(
                "PNG",
                crate::app::state::ExportFormat::Png,
                Some(settings.format),
                Message::SetExportFormat
            )
            .size(theme::size::LABEL)
            .spacing(5)
            .text_size(13)
            .style(ui::styles::radio_style),
            radio(
                "TIFF (16-bit)",
                crate::app::state::ExportFormat::Tiff,
                Some(settings.format),
                Message::SetExportFormat
            )
            .size(theme::size::LABEL)
            .spacing(5)
            .text_size(13)
            .style(ui::styles::radio_style),
        ]
        .spacing(20),
        if settings.format == crate::app::state::ExportFormat::Jpeg {
            Element::from(
                column![
                    row![
                        text("Quality:")
                            .size(theme::size::BODY)
                            .style(|_theme: &Theme| text::Style {
                                color: Some(theme::colours().text_secondary)
                            }),
                        text(format!("{}", settings.quality))
                            .size(theme::size::BODY)
                            .font(theme::font::BOLD)
                            .style(|_theme: &Theme| text::Style {
                                color: Some(theme::colours().text)
                            }),
                    ]
                    .spacing(theme::space::M)
                    .align_y(Alignment::Center),
                    slider(10..=100, settings.quality, Message::SetExportQuality).step(1),
                ]
                .spacing(5),
            )
        } else {
            column![].into()
        }
    ]
    .spacing(theme::space::L);

    let dimensions_section = column![
        text("Dimensions")
            .size(theme::size::SMALL)
            .font(theme::font::BOLD)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
        checkbox("Resize Long Edge", settings.resize)
            .on_toggle(Message::ToggleExportResize)
            .size(16)
            .text_size(13)
            .style(ui::styles::checkbox_style),
        if settings.resize {
            Element::from(
                row![
                    text("Max Width (px):")
                        .size(theme::size::BODY)
                        .style(|_theme: &Theme| text::Style {
                            color: Some(theme::colours().text_secondary)
                        }),
                    text_input("2048", &settings.max_width.to_string())
                        .on_input(|s| s
                            .parse()
                            .ok()
                            .map(Message::SetExportWidth)
                            .unwrap_or(Message::SetExportWidth(settings.max_width)))
                        .width(Length::Fixed(80.0))
                        .padding(5)
                        .style(ui::styles::text_input_style)
                ]
                .spacing(theme::space::L)
                .align_y(Alignment::Center),
            )
        } else {
            row![].into()
        }
    ]
    .spacing(theme::space::L);

    let destination_section = column![
        text("Destination")
            .size(theme::size::SMALL)
            .font(theme::font::BOLD)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
        row![
            text("Base Folder:")
                .size(theme::size::BODY)
                .style(|_theme: &Theme| text::Style {
                    color: Some(theme::colours().text_secondary)
                }),
            button(
                row![
                    text(
                        settings
                            .base_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                    )
                    .size(theme::size::BODY),
                    text("...").size(theme::size::BODY)
                ]
                .spacing(5)
                .align_y(Alignment::Center)
            )
            .on_press(Message::PickExportBasePath)
            .padding(5)
            .style(ui::styles::button_style),
        ]
        .spacing(theme::space::L)
        .align_y(Alignment::Center),
        row![
            text("Subfolder:")
                .size(theme::size::BODY)
                .style(|_theme: &Theme| text::Style {
                    color: Some(theme::colours().text_secondary)
                }),
            text_input("Export", &settings.subfolder)
                .on_input(Message::SetExportSubfolder)
                .padding(5)
                .style(ui::styles::text_input_style)
        ]
        .spacing(theme::space::L)
        .align_y(Alignment::Center),
        text(format!(
            "Full Path: {}/{}",
            settings.base_path.display(),
            settings.subfolder
        ))
        .size(theme::size::CAPTION)
        .style(|_theme: &Theme| text::Style {
            color: Some(theme::colours().text_muted)
        })
    ]
    .spacing(theme::space::L);

    let count = if editor.multi_selection.is_empty() {
        1
    } else {
        editor.multi_selection.len()
    };
    let export_btn = button(
        text(format!("Export {} Images", count)).align_x(iced::alignment::Horizontal::Center),
    )
    .on_press(Message::ExportConfirmed)
    .width(Length::Fill)
    .padding(10)
    .style(ui::styles::AccentButton::style);

    column![
        ui::text::title("Export Studio"),
        iced::widget::horizontal_rule(1).style(ui::styles::rule),
        format_section,
        dimensions_section,
        destination_section,
        iced::widget::vertical_space().height(10),
        row![
            button("Cancel")
                .on_press(Message::CloseModal)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::NeutralButton::style),
            export_btn
        ]
        .spacing(theme::space::L)
    ]
    .spacing(15)
    .width(400)
    .into()
}

/// Confirmation for forgetting a folder.
///
/// The wording is deliberately explicit that files survive — this is the only
/// removal in the app that does NOT touch the disk, and confusing it with
/// "Delete from Disk" would be expensive for the user.
pub fn view_remove_folder_modal(editor: &RawEditor) -> Element<'_, Message> {
    let folder = editor.pending_remove_folder.clone().unwrap_or_default();
    let count = crate::app::handlers::library::folder_image_count(editor, &folder);

    column![
        ui::text::title("Remove Folder from Library"),
        iced::widget::horizontal_rule(1).style(ui::styles::rule),
        text(folder.clone()).size(theme::size::SMALL).style(|_theme: &Theme| text::Style {
            color: Some(theme::colours().text_secondary)
        }),
        text(format!(
            "{count} image{} will be removed from the catalogue, along with any edits made to them.",
            if count == 1 { "" } else { "s" }
        ))
        .size(theme::size::BODY)
        .style(|_theme: &Theme| text::Style {
            color: Some(theme::colours().text_secondary)
        }),
        text("Your RAW files are NOT deleted — they stay exactly where they are on disk, and the folder can be imported again at any time. Only cached previews are cleared.")
            .size(theme::size::CAPTION)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
        iced::widget::vertical_space().height(10),
        row![
            button(text("Cancel").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::CloseModal)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::NeutralButton::style),
            button(text("Remove from Library").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::RemoveFolderConfirmed)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::AccentButton::style),
        ]
        .spacing(theme::space::L)
    ]
    .spacing(theme::space::XL)
    .width(460)
    .into()
}

/// Help > About. Version comes from Cargo so it can't drift.
pub fn view_about_modal<'a>() -> Element<'a, Message> {
    let line = |s: String| {
        text(s).size(theme::size::BODY).style(|_theme: &Theme| text::Style {
            color: Some(theme::colours().text_secondary),
        })
    };

    column![
        ui::text::title("RAW Editor"),
        iced::widget::horizontal_rule(1).style(ui::styles::rule),
        line(format!("Version {}", env!("CARGO_PKG_VERSION"))),
        line("A native RAW photo editor written in Rust.".to_string()),
        line(format!("By {}", env!("CARGO_PKG_AUTHORS"))),
        line("MIT licensed.".to_string()),
        text(crate::app::handlers::window::REPO_URL)
            .size(theme::size::CAPTION)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
        iced::widget::vertical_space().height(10),
        row![
            button(text("Open Repository").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::OpenRepository)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::NeutralButton::style),
            button(text("Close").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::CloseModal)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::AccentButton::style),
        ]
        .spacing(theme::space::L)
    ]
    .spacing(theme::space::L)
    .width(380)
    .into()
}

/// Per-category picker for Ctrl+C.
///
/// The chosen categories travel WITH the copied params (see
/// `core::types::EditClipboard`), so paste merges only these groups onto the
/// target and leaves everything else it had alone.
pub fn view_copy_settings_modal(editor: &RawEditor) -> Element<'_, Message> {
    use crate::app::message::CopyCategory;

    let cats = editor.copy_categories;
    let mask_count = editor.current_edit_params.mask_count;

    // label, description, current value, which category
    let mut rows: Vec<(&str, String, bool, CopyCategory)> = vec![
        ("Tone", "Exposure, contrast, highlights, shadows, whites, blacks, curves".into(), cats.tone, CopyCategory::Tone),
        ("Color", "Vibrance, saturation".into(), cats.color, CopyCategory::Color),
        ("White Balance", "Temperature, tint".into(), cats.white_balance, CopyCategory::WhiteBalance),
        ("Presence", "Texture, clarity, dehaze".into(), cats.presence, CopyCategory::Presence),
        ("Noise Reduction", "Luminance, detail and color noise".into(), cats.noise, CopyCategory::Noise),
        ("Detail", "Sharpening and masking".into(), cats.detail, CopyCategory::Detail),
        ("Geometry & Crop", "Straighten angle and crop rectangle".into(), cats.geometry, CopyCategory::Geometry),
        ("Profile & Tone Mapping", "Camera profile, profile curve, tone mapper, GT shape".into(), cats.profile, CopyCategory::Profile),
        ("Lens Corrections", "Profile, distortion, vignetting, CA, defringe".into(), cats.lens, CopyCategory::Lens),
    ];

    rows.push((
        "Masks",
        match mask_count {
            0 => "No local adjustments on this image".to_string(),
            1 => "1 local adjustment".to_string(),
            n => format!("{n} local adjustments"),
        },
        cats.masks,
        CopyCategory::Masks,
    ));

    // Sensor calibration, not a look — and camera-specific. Shown on the same
    // terms as the develop sidebar's sensor section.
    if crate::debug::SHOW_SENSOR_CORRECTION {
        rows.push((
            "Black Levels",
            "Per-channel sensor black offsets".into(),
            cats.black_levels,
            CopyCategory::BlackLevels,
        ));
    }

    let mut list = column![].spacing(theme::space::M);
    for (label, description, checked, category) in rows {
        // Nothing to copy → nothing to tick.
        let enabled = category != CopyCategory::Masks || mask_count > 0;
        let mut cb = checkbox(label, checked && enabled)
            .size(16)
            .text_size(13)
            .style(ui::styles::checkbox_style);
        if enabled {
            cb = cb.on_toggle(move |v| Message::ToggleCopyCategory(category, v));
        }
        list = list.push(
            column![
                cb,
                text(description)
                    .size(theme::size::CAPTION)
                    .style(|_theme: &Theme| text::Style {
                        color: Some(theme::colours().text_hint)
                    }),
            ]
            .spacing(theme::space::XXS),
        );
    }

    // Masks are positioned in normalized image UV, so the same numbers land
    // somewhere else on a frame with a different shape.
    let mask_caution: Element<'_, Message> = if cats.masks && mask_count > 0 {
        text("Mask positions are relative to the frame — pasting onto a different aspect ratio will shift them.")
            .size(theme::size::CAPTION)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().warn)
            })
            .into()
    } else {
        column![].into()
    };

    let nothing_selected = cats.is_empty();

    column![
        ui::text::title("Copy Settings"),
        iced::widget::horizontal_rule(1).style(ui::styles::rule),
        text("Only the ticked groups are pasted. Everything else on the target image is left alone.")
            .size(theme::size::CAPTION)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
        list,
        mask_caution,
        row![
            button(text("All").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::SetAllCopyCategories(true))
                .padding(6)
                .width(Length::Fixed(70.0))
                .style(ui::styles::NeutralButton::style),
            button(text("None").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::SetAllCopyCategories(false))
                .padding(6)
                .width(Length::Fixed(70.0))
                .style(ui::styles::NeutralButton::style),
        ]
        .spacing(theme::space::M),
        text("Enter copies, Escape cancels. Ctrl+Shift+C copies everything without this dialog.")
            .size(theme::size::CAPTION)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
        iced::widget::vertical_space().height(10),
        row![
            button(text("Cancel").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::CloseModal)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::NeutralButton::style),
            button(text("Copy").align_x(iced::alignment::Horizontal::Center))
                // Greyed out rather than allowing a copy that could paste nothing.
                .on_press_maybe((!nothing_selected).then_some(Message::CopySettingsConfirmed))
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::AccentButton::style),
        ]
        .spacing(theme::space::L)
    ]
    .spacing(theme::space::XL)
    .width(420)
    .into()
}

pub fn view_delete_modal<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    let count = editor.pending_delete_ids.len();
    let body = if count == 1 {
        let name = editor
            .images
            .iter()
            .find(|i| i.id == editor.pending_delete_ids[0])
            .map(|i| i.filename.clone())
            .unwrap_or_default();
        format!("\"{}\" will be removed.", name)
    } else {
        format!("{} images will be removed.", count)
    };

    column![
        ui::text::title("Delete Image"),
        iced::widget::horizontal_rule(1).style(ui::styles::rule),
        text(body).size(theme::size::BODY).style(|_theme: &Theme| text::Style {
            color: Some(theme::colours().text_secondary)
        }),
        text("\"Mark\" hides it from Develop but keeps it (and the file) untouched. \"Delete from Disk\" moves the RAW and cache files to the system trash and removes it from the library. Enter confirms Delete from Disk, D confirms Mark for Removal.")
            .size(theme::size::CAPTION)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
        iced::widget::vertical_space().height(10),
        row![
            button(text("Cancel").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::CloseModal)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::NeutralButton::style),
            button(text("Mark for Removal").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::MarkForRemovalConfirmed)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::AccentButton::style),
            button(text("Delete from Disk").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::DeleteFromDiskConfirmed)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::DangerButton::style),
        ]
        .spacing(theme::space::L)
    ]
    .spacing(15)
    .width(440)
    .into()
}

/// Megabytes for a byte count, as the dialogs show sizes.
pub fn megabytes(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / 1_000_000.0)
}

/// "This feature needs a model": what it is, how big, from where, under what
/// licence — then a download with progress. Closes itself and carries on
/// with the feature once installed (`handlers::models`).
pub fn view_ai_model_modal(editor: &RawEditor) -> Element<'_, Message> {
    let Some(id) = editor.models.offered else {
        return Space::new(Length::Shrink, Length::Shrink).into();
    };
    let model = id.model();
    let job = editor.models.job.as_ref().filter(|(j, _)| *j == id);
    let actions: Element<'_, Message> = match job {
        Some((_, progress)) => column![
            iced::widget::progress_bar(0.0..=1.0, progress.fraction()).height(6),
            row![
                ui::text::tinted(format!(
                        "Downloading… {} of {}",
                        megabytes((progress.fraction() as f64 * model.total_bytes() as f64) as u64),
                        megabytes(model.total_bytes())
                    ), 12, theme::colours().text_muted),
                iced::widget::horizontal_space(),
                button(text("Cancel")).on_press(Message::ModelDownloadCancel).padding([6, 12])
                    .style(ui::styles::NeutralButton::style),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(theme::space::L)
        .into(),
        None => row![
            button(text("Not now").align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::CloseModal)
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::NeutralButton::style),
            button(text(format!("Download {}", megabytes(model.total_bytes()))).align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::ModelDownload(id))
                .padding(10)
                .width(Length::Fill)
                .style(ui::styles::AccentButton::style),
        ]
        .spacing(theme::space::L)
        .into(),
    };
    let error = editor
        .models
        .error
        .as_ref()
        .filter(|(e, _)| *e == id)
        .map(|(_, e)| text(e.clone()).size(theme::size::SMALL).style(|_theme: &Theme| text::Style { color: Some(theme::colours().error) }));

    column![
        ui::text::title(format!("{} needs a model", model.purpose)),
        iced::widget::horizontal_rule(1).style(ui::styles::rule),
        ui::text::tinted(format!("{} — {}", model.title, megabytes(model.total_bytes())), 13, theme::colours().text_secondary),
        ui::text::tinted(format!("Licence: {}. Source: {}", model.licence, model.upstream), 11, theme::colours().text_muted),
        ui::text::tinted("Downloaded once from this app's GitHub release and checked against a published checksum. \
             It runs on your computer — nothing is uploaded — and only while you use the feature."
                .to_string(), 11, theme::colours().text_muted),
    ]
    .push_maybe(error)
    .push(iced::widget::vertical_space().height(6))
    .push(actions)
    .spacing(theme::space::XL)
    .width(460)
    .into()
}

pub fn view_ai_denoise_modal(editor: &RawEditor) -> Element<'_, Message> {
    crate::app::views::ai_denoise::view(editor)
}




/// First run: may RawEditor check for updates at startup? Until this is
/// answered the app makes no request (`handlers::updates`).
pub fn view_update_prompt_modal(editor: &RawEditor) -> Element<'_, Message> {
    column![
        ui::text::title("Updates"),
        iced::widget::horizontal_rule(1).style(ui::styles::rule),
        checkbox("Check for updates at startup", editor.updates.prompt_choice)
            .on_toggle(Message::UpdatePromptToggled)
            .size(16)
            .spacing(theme::space::M)
            .text_size(13)
            .style(ui::styles::checkbox_style),
        ui::text::tinted("Each time it starts, RawEditor asks GitHub whether a newer version has been released. \
             Nothing is downloaded or installed: you choose whether to get it.", 12, theme::colours().text_muted),
        ui::text::tinted("You can change this any time in Preferences › General.", 11, theme::colours().text_muted),
        iced::widget::vertical_space().height(6),
        button(text("Continue").align_x(iced::alignment::Horizontal::Center))
            .on_press(Message::UpdatePromptConfirmed)
            .padding(10)
            .width(Length::Fill)
            .style(ui::styles::AccentButton::style),
    ]
    .spacing(theme::space::XL)
    .width(420)
    .into()
}

/// A check's outcome: checking, a newer version, up to date, or an error.
pub fn view_update_modal(editor: &RawEditor) -> Element<'_, Message> {
    let st = &editor.updates;
    let current = crate::core::updates::current_version();
    let close = |label: &str| {
        button(text(label.to_string()).align_x(iced::alignment::Horizontal::Center))
            .on_press(Message::CloseModal)
            .padding(10)
            .width(Length::Fill)
            .style(ui::styles::NeutralButton::style)
    };

    let (title, body, actions): (&str, Element<'_, Message>, Element<'_, Message>) = if st.checking {
        ("Checking for Updates", ui::text::tinted("Asking GitHub for the newest release…", 13, theme::colours().text_secondary).into(), close("Close").into())
    } else if let Some(a) = &st.available {
        let date = a.published.get(..10).map(|d| format!(", released {d}")).unwrap_or_default();
        (
            "Update Available",
            column![
                ui::text::tinted(format!("RawEditor {}{date}.", a.version), 14, theme::colours().text),
                ui::text::tinted(format!("You have {current}."), 12, theme::colours().text_muted),
                ui::text::tinted("The release page has what's new and a download for this computer.", 12, theme::colours().text_muted),
            ]
            .spacing(theme::space::S)
            .into(),
            row![
                close("Not Now"),
                button(text("Open Release Page").align_x(iced::alignment::Horizontal::Center))
                    .on_press(Message::OpenUpdatePage)
                    .padding(10)
                    .width(Length::Fill)
                    .style(ui::styles::AccentButton::style),
            ]
            .spacing(theme::space::L)
            .into(),
        )
    } else if let Some(Err(e)) = &st.shown {
        (
            "Couldn't Check for Updates",
            column![ui::text::tinted(e.clone(), 13, theme::colours().text_secondary), ui::text::tinted(format!("You have {current}."), 12, theme::colours().text_muted)].spacing(theme::space::S).into(),
            close("Close").into(),
        )
    } else {
        (
            "You're Up to Date",
            ui::text::tinted(format!("RawEditor {current} is the newest release."), 13, theme::colours().text_secondary).into(),
            close("Close").into(),
        )
    };

    column![ui::text::title(title), iced::widget::horizontal_rule(1).style(ui::styles::rule), body, iced::widget::vertical_space().height(6), actions]
        .spacing(theme::space::XL)
        .width(420)
        .into()
}
