use iced::widget::{
    checkbox, column, container, row, scrollable, stack, text, Space,
};
use crate::ui::widgets::button;
use iced::{Element, Length, Theme};

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::ui;
use crate::ui::theme;
use crate::ui::dialog::{self, Kind};

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

    dialog::frame(
        "Keyboard Shortcuts",
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
        dialog::action("Close", Kind::Neutral).on_press(Message::CloseModal),
    )
    .into()
}

pub fn view_export_modal<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    let count = if editor.multi_selection.is_empty() {
        1
    } else {
        editor.multi_selection.len()
    };
    let label = if count == 1 { "Export 1 Image".to_string() } else { format!("Export {count} Images") };

    dialog::frame(
        "Export",
        // The same panel as the Export tab's, so the two can't drift apart.
        crate::app::views::export::settings_panel(editor),
        dialog::actions([
            dialog::action("Cancel", Kind::Neutral).on_press(Message::CloseModal).into(),
            dialog::action(label, Kind::Primary).on_press(Message::ExportConfirmed).into(),
        ]),
    )
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

    let body = column![
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
    ]
    .spacing(theme::space::XL);

    dialog::frame(
        "Remove Folder from Library",
        body,
        dialog::actions([
            dialog::action("Cancel", Kind::Neutral).on_press(Message::CloseModal).into(),
            dialog::action("Remove from Library", Kind::Primary).on_press(Message::RemoveFolderConfirmed).into(),
        ]),
    )
    .width(dialog::WIDE)
    .into()
}

/// Help > About. Version comes from Cargo so it can't drift.
pub fn view_about_modal<'a>() -> Element<'a, Message> {
    let line = |s: String| {
        text(s).size(theme::size::BODY).style(|_theme: &Theme| text::Style {
            color: Some(theme::colours().text_secondary),
        })
    };

    let body = column![
        line(format!("Version {}", env!("CARGO_PKG_VERSION"))),
        line("A native RAW photo editor written in Rust.".to_string()),
        line(format!("By {}", env!("CARGO_PKG_AUTHORS"))),
        line("MIT licensed.".to_string()),
        text(crate::app::handlers::window::REPO_URL)
            .size(theme::size::CAPTION)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
    ]
    .spacing(theme::space::L);

    dialog::frame(
        "RawEditor",
        body,
        dialog::actions([
            dialog::action("Open Repository", Kind::Neutral).on_press(Message::OpenRepository).into(),
            dialog::action("Close", Kind::Primary).on_press(Message::CloseModal).into(),
        ]),
    )
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
        ("Colour", "Vibrance, saturation".into(), cats.color, CopyCategory::Color),
        ("White Balance", "Temperature, tint".into(), cats.white_balance, CopyCategory::WhiteBalance),
        ("Presence", "Texture, clarity, dehaze".into(), cats.presence, CopyCategory::Presence),
        ("Noise Reduction", "Luminance, detail and colour noise".into(), cats.noise, CopyCategory::Noise),
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

    let body = column![
        text("Only the ticked groups are pasted. Everything else on the target image is left alone.")
            .size(theme::size::CAPTION)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
        list,
        mask_caution,
        row![
            button(text("All").size(theme::size::BODY).align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::SetAllCopyCategories(true))
                .padding([theme::space::S, theme::space::XL])
                .width(Length::Fixed(70.0))
                .style(ui::styles::NeutralButton::style),
            button(text("None").size(theme::size::BODY).align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::SetAllCopyCategories(false))
                .padding([theme::space::S, theme::space::XL])
                .width(Length::Fixed(70.0))
                .style(ui::styles::NeutralButton::style),
        ]
        .spacing(theme::space::M),
        text("Enter copies, Escape cancels. Ctrl+Shift+C copies everything without this dialog.")
            .size(theme::size::CAPTION)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
    ]
    .spacing(theme::space::XL);

    dialog::frame(
        "Copy Settings",
        body,
        dialog::actions([
            dialog::action("Cancel", Kind::Neutral).on_press(Message::CloseModal).into(),
            // Greyed out rather than allowing a copy that could paste nothing.
            dialog::action("Copy", Kind::Primary)
                .on_press_maybe((!nothing_selected).then_some(Message::CopySettingsConfirmed))
                .into(),
        ]),
    )
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

    let body = column![
        text(body).size(theme::size::BODY).style(|_theme: &Theme| text::Style {
            color: Some(theme::colours().text_secondary)
        }),
        text("\"Mark\" hides it from Develop but keeps it (and the file) untouched. \"Delete from Disk\" moves the RAW and cache files to the system trash and removes it from the library. Enter confirms Delete from Disk, D confirms Mark for Removal.")
            .size(theme::size::CAPTION)
            .style(|_theme: &Theme| text::Style {
                color: Some(theme::colours().text_muted)
            }),
    ]
    .spacing(theme::space::XL);

    dialog::frame(
        "Delete Image",
        body,
        dialog::actions([
            dialog::action("Cancel", Kind::Neutral).on_press(Message::CloseModal).into(),
            dialog::action("Mark for Removal", Kind::Primary).on_press(Message::MarkForRemovalConfirmed).into(),
            dialog::action("Delete from Disk", Kind::Danger).on_press(Message::DeleteFromDiskConfirmed).into(),
        ]),
    )
    .width(dialog::WIDE)
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
        Some((_, progress)) => dialog::progress(
            progress.fraction(),
            format!(
                "Downloading… {} of {}",
                megabytes((progress.fraction() as f64 * model.total_bytes() as f64) as u64),
                megabytes(model.total_bytes())
            ),
            dialog::inline_action("Cancel").on_press(Message::ModelDownloadCancel),
        )
        .into(),
        None => dialog::actions([
            dialog::action("Not Now", Kind::Neutral).on_press(Message::CloseModal).into(),
            dialog::action(format!("Download {}", megabytes(model.total_bytes())), Kind::Primary)
                .on_press(Message::ModelDownload(id))
                .into(),
        ])
        .into(),
    };
    let error = editor
        .models
        .error
        .as_ref()
        .filter(|(e, _)| *e == id)
        .map(|(_, e)| text(e.clone()).size(theme::size::SMALL).style(|_theme: &Theme| text::Style { color: Some(theme::colours().error) }));

    let body = column![
        ui::text::tinted(format!("{} — {}", model.title, megabytes(model.total_bytes())), theme::size::BODY, theme::colours().text_secondary),
        ui::text::tinted(format!("Licence: {}. Source: {}", model.licence, model.upstream), theme::size::CAPTION, theme::colours().text_muted),
        ui::text::tinted("Downloaded once from this app's GitHub release and checked against a published checksum. \
             It runs on your computer — nothing is uploaded — and only while you use the feature."
                .to_string(), theme::size::CAPTION, theme::colours().text_muted),
    ]
    .push_maybe(error)
    .spacing(theme::space::XL);

    dialog::frame(format!("{} needs a model", model.purpose), body, actions)
        .width(dialog::WIDE)
        .into()
}

pub fn view_ai_denoise_modal(editor: &RawEditor) -> Element<'_, Message> {
    crate::app::views::ai_denoise::view(editor)
}




/// First run: may RawEditor check for updates at startup? Until this is
/// answered the app makes no request (`handlers::updates`).
pub fn view_update_prompt_modal(editor: &RawEditor) -> Element<'_, Message> {
    let body = column![
        checkbox("Check for updates at startup", editor.updates.prompt_choice)
            .on_toggle(Message::UpdatePromptToggled)
            .size(16)
            .spacing(theme::space::M)
            .text_size(13)
            .style(ui::styles::checkbox_style),
        ui::text::tinted("Each time it starts, RawEditor asks GitHub whether a newer version has been released. \
             Nothing is downloaded or installed: you choose whether to get it.", theme::size::SMALL, theme::colours().text_muted),
        ui::text::tinted("You can change this any time in Preferences › General.", theme::size::CAPTION, theme::colours().text_muted),
    ]
    .spacing(theme::space::XL);

    dialog::frame(
        "Updates",
        body,
        dialog::action("Continue", Kind::Primary).on_press(Message::UpdatePromptConfirmed),
    )
    .into()
}

/// A check's outcome: checking, a newer version, up to date, or an error.
pub fn view_update_modal(editor: &RawEditor) -> Element<'_, Message> {
    let st = &editor.updates;
    let current = crate::core::updates::current_version();
    let close = |label: &str| dialog::action(label.to_string(), Kind::Neutral).on_press(Message::CloseModal);

    let (title, body, actions): (&str, Element<'_, Message>, Element<'_, Message>) = if st.checking {
        ("Checking for Updates", ui::text::tinted("Asking GitHub for the newest release…", theme::size::BODY, theme::colours().text_secondary).into(), close("Close").into())
    } else if let Some(a) = &st.available {
        let date = a.published.get(..10).map(|d| format!(", released {d}")).unwrap_or_default();
        (
            "Update Available",
            column![
                ui::text::tinted(format!("RawEditor {}{date}.", a.version), theme::size::LABEL, theme::colours().text),
                ui::text::tinted(format!("You have {current}."), theme::size::SMALL, theme::colours().text_muted),
                ui::text::tinted("The release page has what's new and a download for this computer.", theme::size::SMALL, theme::colours().text_muted),
            ]
            .spacing(theme::space::S)
            .into(),
            dialog::actions([
                close("Not Now").into(),
                dialog::action("Open Release Page", Kind::Primary).on_press(Message::OpenUpdatePage).into(),
            ])
            .into(),
        )
    } else if let Some(Err(e)) = &st.shown {
        (
            "Couldn't Check for Updates",
            column![ui::text::tinted(e.clone(), theme::size::BODY, theme::colours().text_secondary), ui::text::tinted(format!("You have {current}."), theme::size::SMALL, theme::colours().text_muted)].spacing(theme::space::S).into(),
            close("Close").into(),
        )
    } else {
        (
            "You're Up to Date",
            ui::text::tinted(format!("RawEditor {current} is the newest release."), theme::size::BODY, theme::colours().text_secondary).into(),
            close("Close").into(),
        )
    };

    dialog::frame(title, body, actions).into()
}
