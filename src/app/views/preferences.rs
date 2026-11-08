//! Preferences: a side menu of pages (`state::PreferencesSection`) beside a
//! scrolling content pane.
//!
//! The card has a fixed size so switching pages never resizes it under the
//! pointer; the menu's highlight slides between rows (`indicator::vertical`)
//! and the page fades up through the dialog surface (`fade_through`).

use iced::font::{Font, Weight};
use iced::widget::{
    checkbox, column, container, horizontal_rule, row, scrollable, slider, stack, text,
    vertical_rule, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::message::Message;
use crate::app::state::{PreferencesSection, RawEditor};
use crate::core::paths::AppFolder;
use crate::ui;
use crate::ui::icons::ICON_FONT;
use crate::ui::widgets::button;
use crate::ui::theme;

const MENU_WIDTH: f32 = 200.0;
const MENU_ROW: f32 = 34.0;
const BODY_HEIGHT: f32 = 460.0;
const CARD_WIDTH: f32 = 700.0;

/// The content pane's scrollable. Reset to the top on every page change
/// (`handlers::window::handle_set_preferences_section`): iced matches widget
/// state by position, so without that a new page opens at the old one's
/// scroll offset.
pub fn content_scroll_id() -> scrollable::Id {
    scrollable::Id::new("preferences-content")
}

impl PreferencesSection {
    fn label(self) -> &'static str {
        match self {
            PreferencesSection::General => "General",
            PreferencesSection::Performance => "Performance",
            PreferencesSection::Memory => "Memory",
            PreferencesSection::CameraProfiles => "Camera Profiles",
            PreferencesSection::LensProfiles => "Lens Profiles",
            PreferencesSection::AiModels => "AI Models",
            PreferencesSection::Folders => "Folders",
            PreferencesSection::Licenses => "Licenses",
        }
    }

    /// Font Awesome codepoints in the embedded Nerd Font.
    fn icon(self) -> &'static str {
        match self {
            PreferencesSection::General => "\u{f013}",     // cog
            PreferencesSection::Performance => "\u{f0e4}", // tachometer
            PreferencesSection::Memory => "\u{f2db}",      // microchip
            PreferencesSection::CameraProfiles => "\u{f042}", // adjust (colour)
            PreferencesSection::LensProfiles => "\u{f030}", // camera
            PreferencesSection::AiModels => "\u{f0d0}",     // magic
            PreferencesSection::Folders => ui::icons::FOLDER,
            PreferencesSection::Licenses => "\u{f24e}",    // balance scale
        }
    }
}

pub fn view_preferences_modal(editor: &RawEditor) -> Element<'_, Message> {
    let section = editor.preferences_section;

    let page = match section {
        PreferencesSection::General => general(editor),
        PreferencesSection::Performance => performance(editor),
        PreferencesSection::Memory => memory(editor),
        PreferencesSection::CameraProfiles => camera_profiles(editor),
        PreferencesSection::LensProfiles => lens_profiles(editor),
        PreferencesSection::AiModels => ai_models(editor),
        PreferencesSection::Folders => folders(),
        PreferencesSection::Licenses => licenses(),
    };
    let content = scrollable(container(page).padding([4, 18]).width(Length::Fill))
        .id(content_scroll_id())
        .width(Length::Fill)
        .height(Length::Fill);
    let content = crate::ui::widgets::transition::fade_through(
        section.index() as u64,
        ui::styles::modal_surface().background,
        content,
    );

    column![
        text("Preferences").size(theme::size::TITLE).font(bold()).style(|_| text::Style {
            color: Some(theme::colours().text)
        }),
        rule(),
        row![menu(section), vertical_rule(1).style(rule_style), content]
            .spacing(0)
            .height(Length::Fixed(BODY_HEIGHT)),
        rule(),
        row![
            // Resets every page, not just the one showing — the label says so.
            button(text("Reset All to Defaults").size(theme::size::BODY))
                .on_press(Message::ResetPreferences)
                .padding([8, 14])
                .style(ui::styles::NeutralButton::style),
            Space::with_width(Length::Fill),
            button(text("Close").size(theme::size::BODY).align_x(iced::alignment::Horizontal::Center))
                .on_press(Message::CloseModal)
                .padding([8, 14])
                .width(Length::Fixed(110.0))
                .style(ui::styles::AccentButton::style),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(theme::space::XL)
    .width(CARD_WIDTH)
    .into()
}

/// The side menu: transparent row buttons over a highlight that slides
/// between them, as the title bar's tabs do.
fn menu(active: PreferencesSection) -> Element<'static, Message> {
    let rows = PreferencesSection::ALL.iter().fold(column![], |rows, &section| {
        let is_active = section == active;
        rows.push(
            button(
                container(
                    row![
                        text(section.icon())
                            .font(ICON_FONT)
                            .size(theme::size::BODY)
                            .width(Length::Fixed(18.0))
                            .align_x(iced::alignment::Horizontal::Center),
                        text(section.label()).size(theme::size::BODY),
                    ]
                    .spacing(theme::space::L)
                    .align_y(Alignment::Center),
                )
                .height(Length::Fill)
                .align_y(iced::alignment::Vertical::Center),
            )
            .width(Length::Fill)
            .height(Length::Fixed(MENU_ROW))
            .padding([0, 14])
            .style(move |t, s| ui::styles::TabButton { is_active }.style(t, s))
            .on_press(Message::SetPreferencesSection(section)),
        )
    });
    let highlight = crate::ui::widgets::indicator::indicator(
        PreferencesSection::ALL.len(),
        active.index(),
        MENU_ROW,
        crate::ui::widgets::indicator::Appearance {
            fill: theme::colours().control,
            accent: theme::colours().accent,
            accent_height: 2.0,
            accent_inset: 8.0,
        },
    )
    .vertical();

    container(stack![highlight, rows])
        .width(Length::Fixed(MENU_WIDTH))
        .height(Length::Fill)
        .padding([0, 8])
        .into()
}

// ── pages ───────────────────────────────────────────────────────────────────

fn general(editor: &RawEditor) -> Element<'_, Message> {
    column![
        heading("General"),
        group_label("Workflow"),
        column![
            checkbox("Auto-Advance on Rate/Flag", editor.auto_advance)
                .on_toggle(|_| Message::ToggleAutoAdvance)
                .size(16)
                .spacing(theme::space::M)
                .text_size(13)
                .style(checkbox_style),
            hint("After rating or flagging an image, move on to the next one."),
        ]
        .spacing(theme::space::XS),
        // Animations are short and never block input, but some people find
        // motion uncomfortable; this lands every one on its end state.
        column![
            checkbox("Reduce motion", editor.reduce_motion)
                .on_toggle(|_| Message::ToggleReduceMotion)
                .size(16)
                .spacing(theme::space::M)
                .text_size(13)
                .style(checkbox_style),
            hint("Transitions and animations jump straight to their end state."),
        ]
        .spacing(theme::space::XS),
        group_label("Updates"),
        column![
            checkbox("Check for updates at startup", editor.update_check == Some(true))
                .on_toggle(|_| Message::ToggleUpdateCheck)
                .size(16)
                .spacing(theme::space::M)
                .text_size(13)
                .style(checkbox_style),
            hint("One small request to GitHub when RawEditor starts. Nothing is downloaded or installed."),
        ]
        .spacing(theme::space::XS),
        row![
            button(text("Check Now").size(theme::size::BODY))
                .on_press_maybe((!editor.updates.checking).then_some(Message::CheckForUpdates))
                .padding([6, 12])
                .style(ui::styles::NeutralButton::style),
            text(format!("Version {}", crate::core::updates::current_version()))
                .size(theme::size::SMALL)
                .font(iced::Font::MONOSPACE)
                .style(|_| text::Style { color: Some(theme::colours().text_muted) }),
        ]
        .spacing(theme::space::XL)
        .align_y(Alignment::Center),
    ]
    .push_maybe(update_status(editor))
    .spacing(theme::space::XL)
    .into()
}

/// The last Check Now's result, under its button.
fn update_status(editor: &RawEditor) -> Option<Element<'_, Message>> {
    let st = &editor.updates;
    let warn = theme::colours().warn;
    let ok = theme::colours().text_secondary;
    let line = |s: String, color: Color| -> Element<'_, Message> {
        text(s).size(theme::size::SMALL).style(move |_| text::Style { color: Some(color) }).into()
    };
    if st.checking {
        return Some(line("Checking…".into(), ok));
    }
    if let Some(a) = &st.available {
        return Some(
            row![
                line(format!("RawEditor {} is available.", a.version), theme::colours().accent_text),
                button(text("Open Release Page").size(theme::size::SMALL))
                    .on_press(Message::OpenUpdatePage)
                    .padding([4, 10])
                    .style(ui::styles::AccentButton::style),
            ]
            .spacing(theme::space::L)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    match &st.shown {
        Some(Err(e)) => Some(line(format!("Couldn't check: {e}"), warn)),
        Some(Ok(_)) => Some(line("You're up to date.".into(), ok)),
        None => None,
    }
}

fn performance(editor: &RawEditor) -> Element<'_, Message> {
    let used_mb = editor.raw_cache_bytes as f32 / (1024.0 * 1024.0);
    let budget_mb = editor.raw_preload_budget_mb as f32;

    column![
        heading("Performance"),
        readout(format!("RAW Cache Usage: {used_mb:.0} / {budget_mb:.0} MB")),
        group_label("Previews"),
        value_row("Memory Cache Size:", format!("{} Images", editor.cache_capacity)),
        slider(20.0..=500.0, editor.cache_capacity as f32, Message::SetCacheCapacity)
            .on_release(Message::SavePreferences)
            .step(10.0_f32)
            .style(ui::styles::ProSlider::style),
        hint("Controls how many high-res previews are kept in RAM."),
        value_row(
            "Preview Preload (Behind/Ahead):",
            format!("{}/{}", editor.preview_preload_behind, editor.preview_preload_ahead),
        ),
        labelled_slider(
            "Behind",
            slider(
                0.0..=crate::app::state::PREVIEW_PRELOAD_BEHIND_MAX as f32,
                editor.preview_preload_behind as f32,
                Message::SetPreviewPreloadBehind,
            )
            .on_release(Message::SavePreferences)
            .step(1.0_f32)
            .style(ui::styles::ProSlider::style),
        ),
        labelled_slider(
            "Ahead",
            slider(
                0.0..=crate::app::state::PREVIEW_PRELOAD_AHEAD_MAX as f32,
                editor.preview_preload_ahead as f32,
                Message::SetPreviewPreloadAhead,
            )
            .on_release(Message::SavePreferences)
            .step(1.0_f32)
            .style(ui::styles::ProSlider::style),
        ),
        hint("How many working previews stay queued around your current image."),
        group_label("RAW Files"),
        value_row("RAW Preload Budget:", format!("{} MB", editor.raw_preload_budget_mb)),
        slider(0.0..=4096.0, editor.raw_preload_budget_mb as f32, Message::SetRawPreloadBudget)
            .on_release(Message::SavePreferences)
            .step(64.0_f32)
            .style(ui::styles::ProSlider::style),
        hint("Develop RAW preloading budget. Set to 0 to disable RAW preloading."),
        value_row(
            "RAW Preload (Behind/Ahead):",
            format!("{}/{}", editor.raw_preload_behind, editor.raw_preload_ahead),
        ),
        labelled_slider(
            "Behind",
            slider(
                0.0..=crate::app::state::RAW_PRELOAD_BEHIND_MAX as f32,
                editor.raw_preload_behind as f32,
                Message::SetRawPreloadBehind,
            )
            .on_release(Message::SavePreferences)
            .step(1.0_f32)
            .style(ui::styles::ProSlider::style),
        ),
        labelled_slider(
            "Ahead",
            slider(
                0.0..=crate::app::state::RAW_PRELOAD_AHEAD_MAX as f32,
                editor.raw_preload_ahead as f32,
                Message::SetRawPreloadAhead,
            )
            .on_release(Message::SavePreferences)
            .step(1.0_f32)
            .style(ui::styles::ProSlider::style),
        ),
        hint("How many full RAWs are pre-decoded for faster Develop navigation."),
    ]
    .spacing(theme::space::M)
    .into()
}

fn memory(editor: &RawEditor) -> Element<'_, Message> {
    // Live readout. On a platform where available memory can't be read, say
    // so rather than showing a number that isn't there.
    let available = match crate::core::memory::available_bytes() {
        Some(b) => format!("System memory available: {} MB", crate::core::memory::as_mb(b)),
        None => "System memory unavailable on this platform — check disabled".to_string(),
    };

    column![
        heading("Memory"),
        group_label("Memory Safety"),
        value_row(
            "Keep RAM Free:",
            if editor.min_free_ram_mb == 0 {
                "Off".to_string()
            } else {
                format!("{} MB", editor.min_free_ram_mb)
            },
        ),
        slider(
            0.0..=crate::app::state::MIN_FREE_RAM_MB_MAX as f32,
            editor.min_free_ram_mb as f32,
            Message::SetMinFreeRam,
        )
        .on_release(Message::SavePreferences)
        .step(256.0_f32)
        .style(ui::styles::ProSlider::style),
        readout(available),
        hint("Batch export pauses if free memory drops below this, after first dropping its own caches. 0 disables the check."),
    ]
    .spacing(theme::space::M)
    .into()
}

// ── camera profiles ─────────────────────────────────────────────────────────

/// One entry of a camera's "Default" dropdown: a profile file name,
/// `"builtin"`, or `None` for automatic.
#[derive(Debug, Clone, PartialEq)]
struct DefaultOption {
    value: Option<String>,
    label: String,
}

impl std::fmt::Display for DefaultOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// Camera profiles (DCP): what is installed, each camera's default, getting
/// RawTherapee's open-source ones and keeping them current, and importing
/// the user's own (`handlers::camera_profiles`).
fn camera_profiles(editor: &RawEditor) -> Element<'_, Message> {
    use crate::app::state::ProfileJob;
    use crate::core::camera_profiles::{camera_name, Catalog, Origin};

    let catalog = Catalog::loaded();
    let state = &editor.camera_profiles;
    let idle = state.busy.is_none();

    let installed: Element<'_, Message> = match &catalog {
        None => hint("Reading the profile folders…"),
        Some(c) if c.profiles.is_empty() => hint(
            "No camera profiles are installed yet. When a camera has none, Develop offers RawTherapee's — or import your own below.",
        ),
        Some(c) => c.cameras().into_iter().fold(column![].spacing(14), |list, camera| {
            let profiles: Vec<_> = c.profiles.iter().filter(|p| p.camera.eq_ignore_ascii_case(&camera)).collect();
            let mut options = vec![DefaultOption { value: None, label: "Automatic".to_string() }];
            options.extend(profiles.iter().map(|p| DefaultOption {
                value: Some(p.file_name.clone()),
                label: p.name.clone(),
            }));
            options.push(DefaultOption { value: Some("builtin".to_string()), label: "None (built-in colour)".to_string() });
            let current = editor.camera_profile_defaults.get(&camera).cloned();
            let selected = options.iter().find(|o| o.value == current).or(options.first()).cloned();
            let key = camera.clone();
            let default = iced::widget::pick_list(options, selected, move |o: DefaultOption| {
                Message::SetCameraProfileDefault(key.clone(), o.value)
            })
            .text_size(12)
            .padding([4, 8])
            .width(Length::Fixed(210.0))
            .style(ui::styles::pick_list_style)
            .menu_style(ui::styles::pick_list_menu_style);

            let rows = profiles.iter().fold(column![].spacing(3), |rows, p| {
                let origin = match p.origin {
                    Origin::Own => "Your file",
                    Origin::Downloaded => "Downloaded",
                };
                rows.push(
                    row![
                        text(p.name.clone()).size(theme::size::SMALL).style(|_| text::Style {
                            color: Some(theme::colours().text)
                        }),
                        Space::with_width(Length::Fill),
                        text(format!("{origin} · {}", p.file_name)).size(theme::size::CAPTION).style(|_| text::Style {
                            color: Some(theme::colours().text_muted)
                        }),
                    ]
                    .spacing(theme::space::M)
                    .align_y(Alignment::Center),
                )
            });
            list.push(
                column![
                    row![
                        card_title_owned(camera),
                        Space::with_width(Length::Fill),
                        text("Default").size(theme::size::CAPTION).style(|_| text::Style {
                            color: Some(theme::colours().text_muted)
                        }),
                        default,
                    ]
                    .spacing(theme::space::M)
                    .align_y(Alignment::Center),
                    rows,
                ]
                .spacing(theme::space::S),
            )
        })
        .into(),
    };

    let warn = theme::colours().warn;
    let ok = theme::colours().ok;
    let status: Option<(String, Color)> = match state.busy {
        Some(ProfileJob::Downloading) => Some(("Downloading…".into(), ok)),
        Some(ProfileJob::Importing) => Some(("Importing…".into(), ok)),
        Some(ProfileJob::Updating) => Some(("Checking RawTherapee for newer profiles…".into(), ok)),
        Some(ProfileJob::Rescanning) => Some(("Reading the profile folders…".into(), ok)),
        None => state.notice.as_ref().map(|n| match n {
            Ok(m) => (m.clone(), ok),
            Err(e) => (e.clone(), warn),
        }),
    };

    // Download for the image open in Develop, when there is one.
    let open_camera = editor.current_metadata.as_ref().map(|m| camera_name(&m.make, &m.model));
    let mut actions = row![].spacing(theme::space::M);
    if let Some(camera) = &open_camera {
        actions = actions.push(
            button(
                row![
                    text("\u{f019}").font(ICON_FONT).size(theme::size::BODY), // download
                    text(format!("Download for {camera}")).size(theme::size::BODY),
                ]
                .spacing(theme::space::M)
                .align_y(Alignment::Center),
            )
            .on_press_maybe(idle.then_some(Message::DownloadCameraProfile))
            .padding([6, 12])
            .style(ui::styles::AccentButton::style),
        );
    }
    actions = actions.push(
        button(text("Update Downloaded").size(theme::size::BODY))
            .on_press_maybe(idle.then_some(Message::UpdateCameraProfiles))
            .padding([6, 12])
            .style(ui::styles::NeutralButton::style),
    );
    let mut open_source = column![
        card_title("Open-Source Profiles"),
        hint("The RawTherapee project publishes free camera profiles for about 160 cameras. When an image's camera has no profile, Develop offers to download its one; RawEditor goes online only when you click."),
        actions,
    ]
    .spacing(theme::space::L);
    if let Some((line, color)) = status {
        open_source = open_source.push(text(line).size(theme::size::SMALL).style(move |_| text::Style { color: Some(color) }));
    }

    let own_dir = crate::core::paths::profiles_dir();
    let own = column![
        row![
            card_title("Your Own Profiles"),
            Space::with_width(Length::Fill),
            button(text("Rescan").size(theme::size::BODY))
                .on_press_maybe(idle.then_some(Message::RescanCameraProfiles))
                .padding([6, 12])
                .style(ui::styles::NeutralButton::style),
            button(text("Import…").size(theme::size::BODY))
                .on_press_maybe(idle.then_some(Message::ImportCameraProfiles))
                .padding([6, 12])
                .style(ui::styles::NeutralButton::style),
            button(
                row![text(ui::icons::FOLDER_OPEN).font(ICON_FONT).size(theme::size::BODY), text("Open Folder").size(theme::size::BODY)]
                    .spacing(theme::space::M)
                    .align_y(Alignment::Center),
            )
            .on_press(Message::OpenAppFolder(AppFolder::Profiles))
            .padding([6, 12])
            .style(ui::styles::NeutralButton::style),
        ]
        .spacing(theme::space::M)
        .align_y(Alignment::Center),
        text(own_dir.display().to_string()).size(theme::size::SMALL).style(|_| text::Style {
            color: Some(theme::colours().accent_text)
        }),
        hint("Any DNG camera profile (.dcp) works. Your own profiles are offered before downloaded ones; Rescan after copying files in by hand."),
    ]
    .spacing(theme::space::L);

    column![
        heading("Camera Profiles"),
        hint("A camera profile (DCP) describes how a sensor records colour. Each image can pick its own in Develop; the default below is used otherwise."),
        card(column![card_title("Installed"), installed].spacing(theme::space::XL)),
        card(open_source),
        card(own),
    ]
    .spacing(theme::space::XL)
    .into()
}

fn card_title_owned(title: String) -> Element<'static, Message> {
    text(title)
        .size(theme::size::LABEL)
        .font(bold())
        .style(|_| text::Style { color: Some(theme::colours().text) })
        .into()
}

// ── lens profiles ───────────────────────────────────────────────────────────

/// The lensfun database: what is installed and where from, downloading and
/// updating it (`handlers::lensfun`), and the folder for the user's own files.
fn lens_profiles(editor: &RawEditor) -> Element<'_, Message> {
    use crate::app::state::LensfunJob;
    use crate::core::lensfun::format_date;

    let db = crate::core::lens::LensDb::loaded();
    let state = &editor.lensfun;
    let busy = state.busy;
    let own_dir = crate::core::paths::lensfun_dir();
    let downloaded_dir = crate::core::paths::lensfun_updates_dir();

    // What is installed, folder by folder.
    let installed: Element<'_, Message> = match &db {
        None => hint("Reading the lens folders…"),
        Some(db) if db.lenses.is_empty() => hint(
            "No lens profiles are installed yet. Download the lensfun database below — or add XML files by hand.",
        ),
        Some(db) => {
            let sources = db.sources.iter().fold(column![].spacing(theme::space::S), |list, s| {
                let origin = if s.dir == own_dir {
                    "Your own files"
                } else if s.dir == downloaded_dir {
                    "Downloaded by RawEditor"
                } else {
                    "Installed on this system"
                };
                let files = if s.files == 1 { "1 file".to_string() } else { format!("{} files", s.files) };
                let detail = match s.timestamp {
                    Some(t) => format!("{files} · published {}", format_date(t)),
                    None => files,
                };
                list.push(
                    column![
                        row![
                            text(origin).size(theme::size::SMALL).style(|_| text::Style {
                                color: Some(theme::colours().text)
                            }),
                            Space::with_width(Length::Fill),
                            text(detail).size(theme::size::CAPTION).style(|_| text::Style {
                                color: Some(theme::colours().text_muted)
                            }),
                        ]
                        .align_y(Alignment::Center),
                        text(s.dir.display().to_string()).size(theme::size::CAPTION).style(|_| text::Style {
                            color: Some(theme::colours().accent_text)
                        }),
                    ]
                    .spacing(theme::space::XXS),
                )
            });
            column![
                readout(format!("{} lenses · {} cameras", db.lenses.len(), db.cameras.len())),
                sources,
            ]
            .spacing(theme::space::L)
            .into()
        }
    };

    // The newest copy on disk, for "up to date?".
    let newest_local = db.as_ref().and_then(|d| d.sources.iter().filter_map(|s| s.timestamp).max());
    let has_download = db.as_ref().is_some_and(|d| d.sources.iter().any(|s| s.dir == downloaded_dir));
    let warn = theme::colours().warn;
    let ok = theme::colours().ok;
    let status: Option<(String, Color)> = match busy {
        Some(LensfunJob::Checking) => Some(("Checking lensfun.github.io…".into(), ok)),
        Some(LensfunJob::Downloading) => Some(("Downloading…".into(), ok)),
        Some(LensfunJob::Rescanning) => Some(("Reading the lens folders…".into(), ok)),
        None => match (&state.installed, &state.remote) {
            (Some(Err(e)), _) => Some((format!("Download failed: {e}"), warn)),
            (Some(Ok(i)), _) => Some((
                format!("Installed {} lenses, published {}", i.lenses, format_date(i.timestamp)),
                ok,
            )),
            (None, Some(Err(e))) => Some((format!("Couldn't check: {e}"), warn)),
            (None, Some(Ok(r))) if newest_local.is_some_and(|t| t >= r.timestamp) => {
                Some((format!("Up to date — the newest database was published {}", format_date(r.timestamp)), ok))
            }
            (None, Some(Ok(r))) => Some((
                format!("A newer database is available, published {}", format_date(r.timestamp)),
                theme::colours().accent_text,
            )),
            (None, None) => None,
        },
    };
    let idle = busy.is_none();
    let download = button(
        row![
            text("\u{f019}").font(ICON_FONT).size(theme::size::BODY), // download
            text(if has_download { "Update Now" } else { "Download Lens Profiles" }).size(theme::size::BODY),
        ]
        .spacing(theme::space::M)
        .align_y(Alignment::Center),
    )
    .on_press_maybe(idle.then_some(Message::LensfunDownload))
    .padding([6, 12])
    .style(ui::styles::AccentButton::style);
    let check = button(text("Check for Updates").size(theme::size::BODY))
        .on_press_maybe(idle.then_some(Message::LensfunCheck))
        .padding([6, 12])
        .style(ui::styles::NeutralButton::style);
    let mut updates = column![
        card_title("Download & Updates"),
        row![download, check].spacing(theme::space::M),
    ]
    .spacing(theme::space::L);
    if let Some((line, color)) = status {
        updates = updates.push(text(line).size(theme::size::SMALL).style(move |_| text::Style { color: Some(color) }));
    }
    updates = updates.push(hint(
        "About 0.4 MB from the lensfun project's site (lensfun.github.io). RawEditor goes online only when you click.",
    ));

    let rescan = button(text("Rescan").size(theme::size::BODY))
        .on_press_maybe(idle.then_some(Message::LensfunRescan))
        .padding([6, 12])
        .style(ui::styles::NeutralButton::style);
    let open = button(
        row![text(ui::icons::FOLDER_OPEN).font(ICON_FONT).size(theme::size::BODY), text("Open Folder").size(theme::size::BODY)]
            .spacing(theme::space::M)
            .align_y(Alignment::Center),
    )
    .on_press(Message::OpenAppFolder(AppFolder::Lensfun))
    .padding([6, 12])
    .style(ui::styles::NeutralButton::style);

    column![
        heading("Lens Profiles"),
        hint("Profile corrections for distortion, vignetting and chromatic aberration use calibrations from the open lensfun database."),
        card(column![card_title("Installed"), installed].spacing(theme::space::L)),
        card(updates),
        card(
            column![
                row![card_title("Your Own Profiles"), Space::with_width(Length::Fill), rescan, open]
                    .spacing(theme::space::M)
                    .align_y(Alignment::Center),
                text(own_dir.display().to_string()).size(theme::size::SMALL).style(|_| text::Style {
                    color: Some(theme::colours().accent_text)
                }),
                hint("Put lensfun XML files here — they take priority over the downloaded database. Rescan after adding them."),
            ]
            .spacing(theme::space::L),
        ),
        hint("Lens profiles © the lensfun contributors, CC BY-SA 3.0."),
    ]
    .spacing(theme::space::XL)
    .into()
}

/// Preferences › AI Models: what each model is for, whether it is installed,
/// and Download / Remove. Downloads also start from the feature that needs
/// them (`handlers::models::offer`).
fn ai_models(editor: &RawEditor) -> Element<'_, Message> {
    use crate::app::views::modals::megabytes;
    use crate::ml::models::{is_installed, ALL};

    let cards = ALL.iter().fold(column![].spacing(theme::space::XL), |list, model| {
        let id = model.id;
        let installed = is_installed(id);
        let job = editor.models.job.as_ref().filter(|(j, _)| *j == id);
        let busy = editor.models.job.is_some();
        let action: Element<'_, Message> = match (job, installed) {
            (Some((_, progress)), _) => row![
                iced::widget::progress_bar(0.0..=1.0, progress.fraction()).height(6).width(Length::Fixed(160.0)),
                button(text("Cancel").size(theme::size::BODY))
                    .on_press(Message::ModelDownloadCancel)
                    .padding([6, 12])
                    .style(ui::styles::NeutralButton::style),
            ]
            .spacing(theme::space::M)
            .align_y(Alignment::Center)
            .into(),
            (None, true) => button(text("Remove").size(theme::size::BODY))
                .on_press(Message::ModelRemove(id))
                .padding([6, 12])
                .style(ui::styles::NeutralButton::style)
                .into(),
            (None, false) => button(
                row![
                    text("\u{f019}").font(ICON_FONT).size(theme::size::BODY), // download
                    text(format!("Download {}", megabytes(model.total_bytes()))).size(theme::size::BODY),
                ]
                .spacing(theme::space::M)
                .align_y(Alignment::Center),
            )
            .on_press_maybe((!busy).then_some(Message::ModelDownload(id)))
            .padding([6, 12])
            .style(ui::styles::AccentButton::style)
            .into(),
        };
        let state = if installed { "Installed" } else { "Not installed" };
        let mut body = column![
            row![card_title(model.purpose), Space::with_width(Length::Fill), action]
                .spacing(theme::space::M)
                .align_y(Alignment::Center),
            readout(format!("{} · {} · {state}", model.title, megabytes(model.total_bytes()))),
            hint(model.licence),
        ]
        .spacing(theme::space::M);
        if let Some((_, e)) = editor.models.error.as_ref().filter(|(e, _)| *e == id) {
            body = body.push(text(format!("Download failed: {e}")).size(theme::size::SMALL).style(|_| text::Style {
                color: Some(theme::colours().warn),
            }));
        }
        list.push(card(body))
    });

    column![
        heading("AI Models"),
        hint("AI masks and AI denoise run on your computer, on the CPU, only while you use them. Their models are downloaded once, on request, from RawEditor's GitHub release and checked against published checksums."),
        cards,
        row![
            Space::with_width(Length::Fill),
            button(
                row![text(ui::icons::FOLDER_OPEN).font(ICON_FONT).size(theme::size::BODY), text("Open Folder").size(theme::size::BODY)]
                    .spacing(theme::space::M)
                    .align_y(Alignment::Center),
            )
            .on_press(Message::OpenAppFolder(AppFolder::Models))
            .padding([6, 12])
            .style(ui::styles::NeutralButton::style),
        ],
    ]
    .spacing(theme::space::XL)
    .into()
}

fn card_title(title: &str) -> Element<'_, Message> {
    text(title)
        .size(theme::size::LABEL)
        .font(bold())
        .style(|_| text::Style { color: Some(theme::colours().text) })
        .into()
}

fn folders() -> Element<'static, Message> {
    column![
        heading("Folders"),
        hint("RawEditor keeps its own files in two folders. Your RAW files stay where they are and are never copied into either."),
        folder_card(
            "Library & Settings",
            AppFolder::Data,
            &[
                ("raw_editor.db", "The catalogue: imported images, ratings, flags and edits"),
                ("settings.json", "These preferences"),
                ("profiles/", "Camera profiles (.dcp): your own; downloaded ones are in profiles/downloaded/"),
                ("lensfun/", "Your own lens profiles; the downloaded database is in lensfun/updates/"),
                ("logs/", "Diagnostic logs"),
            ],
            "Back this folder up to keep your edits.",
        ),
        folder_card(
            "Cache",
            AppFolder::Cache,
            &[
                ("thumb/", "Library grid thumbnails"),
                ("instant/", "Previews for fast full-screen browsing"),
                ("working/", "Develop previews"),
                ("edited/", "Thumbnails of your images as edited"),
            ],
            "Safe to delete while RawEditor is closed: anything missing is rebuilt.",
        ),
    ]
    .spacing(theme::space::XL)
    .into()
}

fn folder_card(
    title: &'static str,
    folder: AppFolder,
    contents: &'static [(&'static str, &'static str)],
    note: &'static str,
) -> Element<'static, Message> {
    let path = folder.path();
    let path_text = path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Not available on this platform".to_string());

    let listing = contents.iter().fold(column![].spacing(theme::space::XS), |list, &(name, what)| {
        list.push(
            row![
                text(name).size(theme::size::SMALL).width(Length::Fixed(120.0)).style(|_| text::Style {
                    color: Some(theme::colours().text)
                }),
                text(what).size(theme::size::CAPTION).style(|_| text::Style {
                    color: Some(theme::colours().text_muted)
                }),
            ]
            .spacing(theme::space::M)
            .align_y(Alignment::Center),
        )
    });

    let open = button(
        row![text(ui::icons::FOLDER_OPEN).font(ICON_FONT).size(theme::size::BODY), text("Open Folder").size(theme::size::BODY)]
            .spacing(theme::space::M)
            .align_y(Alignment::Center),
    )
    .on_press_maybe(path.is_some().then_some(Message::OpenAppFolder(folder)))
    .padding([6, 12])
    .style(ui::styles::NeutralButton::style);

    card(
        column![
            row![
                text(title).size(theme::size::LABEL).font(bold()).style(|_| text::Style {
                    color: Some(theme::colours().text)
                }),
                Space::with_width(Length::Fill),
                open,
            ]
            .align_y(Alignment::Center),
            text(path_text).size(theme::size::SMALL).style(|_| text::Style {
                color: Some(theme::colours().accent_text)
            }),
            listing,
            hint(note),
        ]
        .spacing(theme::space::L),
    )
}

// ── licenses ────────────────────────────────────────────────────────────────

/// One credited work. `crates` ties library entries to `Cargo.toml`, so a
/// test can insist every direct dependency is credited.
struct Credit {
    name: &'static str,
    #[cfg_attr(not(test), allow(dead_code))]
    crates: &'static [&'static str],
    license: &'static str,
    purpose: &'static str,
    url: Option<&'static str>,
}

const fn credit(
    name: &'static str,
    crates: &'static [&'static str],
    license: &'static str,
    purpose: &'static str,
    url: &'static str,
) -> Credit {
    Credit { name, crates, license, purpose, url: Some(url) }
}

/// Compiled or embedded into the binary, beyond the crates below.
const BUNDLED: &[Credit] = &[
    credit(
        "JetBrains Mono Nerd Font",
        &[],
        "OFL-1.1",
        "Interface and icon font",
        "https://github.com/ryanoasis/nerd-fonts",
    ),
    credit(
        "SQLite",
        &[],
        "Public domain",
        "Catalogue database engine, compiled in by rusqlite",
        "https://sqlite.org/copyright.html",
    ),
    credit(
        "ONNX Runtime",
        &[],
        "MIT",
        "Runs the AI models, compiled in by ort",
        "https://github.com/microsoft/onnxruntime",
    ),
];

/// Direct dependencies (and wgpu, which iced_wgpu brings in but which does
/// the app's heaviest work), alphabetical.
const LIBRARIES: &[Credit] = &[
    credit("bytemuck", &["bytemuck"], "Zlib OR Apache-2.0 OR MIT", "GPU buffer layout", "https://github.com/Lokathor/bytemuck"),
    credit("bzip2-rs", &["bzip2-rs"], "MIT OR Apache-2.0", "Unpacking the lens-profile download", "https://github.com/paolobarbolini/bzip2-rs"),
    credit("cgmath", &["cgmath"], "Apache-2.0", "Matrix maths", "https://github.com/rustgd/cgmath"),
    credit("chrono", &["chrono"], "MIT OR Apache-2.0", "Dates and times", "https://github.com/chronotope/chrono"),
    credit("dirs", &["dirs"], "MIT OR Apache-2.0", "Platform folder locations", "https://github.com/soc/dirs-rs"),
    credit("half", &["half"], "MIT OR Apache-2.0", "16-bit floats", "https://github.com/VoidStarKat/half-rs"),
    credit("iced", &["iced", "iced_wgpu"], "MIT", "GUI toolkit", "https://github.com/iced-rs/iced"),
    credit("iced_aw", &["iced_aw"], "MIT", "Menu bar", "https://github.com/iced-rs/iced_aw"),
    credit("image · png", &["image", "png"], "MIT OR Apache-2.0", "Image decoding, PNG export", "https://github.com/image-rs/image"),
    credit("jpeg-encoder", &["jpeg-encoder"], "(MIT OR Apache-2.0) AND IJG", "JPEG export", "https://github.com/vstroebel/jpeg-encoder"),
    credit("kamadak-exif", &["kamadak-exif"], "BSD-2-Clause", "EXIF metadata", "https://github.com/kamadak/exif-rs"),
    credit("lru", &["lru"], "MIT", "Preview cache", "https://github.com/jeromefroe/lru-rs"),
    credit("ort", &["ort"], "MIT OR Apache-2.0", "ONNX Runtime bindings, for the AI features", "https://github.com/pykeio/ort"),
    credit("rawler", &["rawler"], "LGPL-2.1", "RAW decoding, from the dnglab project", "https://github.com/dnglab/dnglab"),
    credit("rfd", &["rfd"], "MIT", "Native file dialogs", "https://github.com/PolyMeilex/rfd"),
    credit("ring", &[], "Apache-2.0 AND ISC", "Cryptography for HTTPS", "https://github.com/briansmith/ring"),
    credit("roxmltree", &["roxmltree"], "MIT OR Apache-2.0", "lensfun XML parsing", "https://github.com/RazrFalcon/roxmltree"),
    credit("rusqlite", &["rusqlite"], "MIT", "SQLite bindings", "https://github.com/rusqlite/rusqlite"),
    credit("rustls", &[], "Apache-2.0 OR ISC OR MIT", "HTTPS for the lens-profile download", "https://github.com/rustls/rustls"),
    credit("serde · serde_json", &["serde", "serde_json"], "MIT OR Apache-2.0", "Settings and edit storage", "https://github.com/serde-rs/serde"),
    credit("sha2", &["sha2"], "MIT OR Apache-2.0", "Checking downloaded AI models", "https://github.com/RustCrypto/hashes"),
    credit("tar", &["tar"], "MIT OR Apache-2.0", "Unpacking the lens-profile download", "https://github.com/composefs/tar-rs"),
    credit("thiserror", &["thiserror"], "MIT OR Apache-2.0", "Error types", "https://github.com/dtolnay/thiserror"),
    credit("tiff", &["tiff"], "MIT", "TIFF export, DCP parsing", "https://github.com/image-rs/image-tiff"),
    credit("tokio", &["tokio"], "MIT", "Background work", "https://github.com/tokio-rs/tokio"),
    credit("tracing", &["tracing", "tracing-subscriber", "tracing-appender"], "MIT", "Logging", "https://github.com/tokio-rs/tracing"),
    credit("trash", &["trash"], "MIT", "Moving files to the system trash", "https://github.com/ArturKovacs/trash"),
    credit("ureq", &["ureq"], "MIT OR Apache-2.0", "Downloading lens profiles", "https://github.com/algesten/ureq"),
    credit("walkdir", &["walkdir"], "Unlicense OR MIT", "Folder import", "https://github.com/BurntSushi/walkdir"),
    credit("webpki-roots", &[], "CDLA-Permissive-2.0", "Mozilla's trusted root certificates, for HTTPS", "https://github.com/rustls/webpki-roots"),
    credit("wgpu", &[], "MIT OR Apache-2.0", "GPU rendering", "https://github.com/gfx-rs/wgpu"),
    credit("zune-jpeg", &["zune-jpeg", "zune-core"], "MIT OR Apache-2.0 OR Zlib", "Fast JPEG decoding (fast-jpeg builds)", "https://github.com/etemesi254/zune-image"),
];

/// Read at runtime when the user has them; never shipped with RawEditor.
const EXTERNAL: &[Credit] = &[
    credit(
        "Lensfun database",
        &[],
        "CC BY-SA 3.0",
        "Lens profiles — downloaded when you ask, or read from your system",
        "https://lensfun.github.io",
    ),
    credit(
        "SAM 2.1 (hiera-tiny)",
        &[],
        "Apache-2.0",
        "AI masks — Meta's Segment Anything 2, converted to ONNX; downloaded when you ask",
        "https://github.com/facebookresearch/sam2",
    ),
    credit(
        "PMRID",
        &[],
        "Apache-2.0",
        "AI denoise — Megvii's raw denoiser (ECCV 2020), converted to ONNX; downloaded when you ask",
        "https://github.com/MegEngine/PMRID",
    ),
    credit(
        "RawTherapee camera profiles",
        &[],
        "GPL-3.0-or-later",
        "Open-source DCP profiles — downloaded when you ask",
        crate::core::camera_profiles::RT_PAGE,
    ),
    Credit {
        name: "Camera profiles you add",
        crates: &[],
        license: "Their makers' terms",
        purpose: "DCP files you import or copy into the profiles folder",
        url: None,
    },
];

const LICENSE_TEXT: &str = include_str!("../../../LICENSE");

fn licenses() -> Element<'static, Message> {
    let list = |credits: &'static [Credit]| {
        credits.iter().fold(column![].spacing(theme::space::XXS), |list, c| list.push(credit_row(c)))
    };

    column![
        heading("Licenses"),
        row![
            text(format!("RawEditor {}", env!("CARGO_PKG_VERSION"))).size(theme::size::LABEL).font(bold()).style(
                |_| text::Style { color: Some(theme::colours().text) }
            ),
            Space::with_width(Length::Fill),
            text("MIT").size(theme::size::CAPTION).style(|_| text::Style { color: Some(theme::colours().accent_text) }),
        ]
        .align_y(Alignment::Center),
        card(text(LICENSE_TEXT.trim()).size(theme::size::CAPTION).style(|_| text::Style {
            color: Some(theme::colours().text_muted)
        })),
        group_label("Bundled"),
        list(BUNDLED),
        group_label("Libraries"),
        list(LIBRARIES),
        hint("These depend on several hundred further crates, almost all under MIT, Apache-2.0, BSD or Zlib terms; each one's license is listed on crates.io."),
        // The IJG license asks for exactly this sentence wherever only the
        // executable is distributed (jpeg-encoder ports libjpeg code).
        hint("This software is based in part on the work of the Independent JPEG Group."),
        group_label("Used if installed"),
        list(EXTERNAL),
        hint("None of these is bundled. Lens and camera profiles are downloaded only when you ask; profiles you add stay yours."),
    ]
    .spacing(theme::space::M)
    .into()
}

/// Name and purpose on the left, license on the right; the whole row opens
/// the project page when there is one.
fn credit_row(c: &'static Credit) -> Element<'static, Message> {
    let body = row![
        column![
            text(c.name).size(theme::size::BODY).font(bold()).style(|_| text::Style {
                color: Some(theme::colours().text)
            }),
            text(c.purpose).size(theme::size::CAPTION).style(|_| text::Style {
                color: Some(theme::colours().text_muted)
            }),
        ]
        .spacing(theme::space::XXS)
        .width(Length::Fill),
        text(c.license).size(theme::size::CAPTION).style(|_| text::Style {
            color: Some(theme::colours().accent_text)
        }),
    ]
    .spacing(theme::space::XL)
    .align_y(Alignment::Center);

    match c.url {
        Some(url) => button(body)
            .width(Length::Fill)
            .padding([6, 8])
            .style(|t, s| ui::styles::TabButton { is_active: false }.style(t, s))
            .on_press(Message::OpenLink(url))
            .into(),
        None => container(body).padding([6, 8]).into(),
    }
}

// ── pieces ──────────────────────────────────────────────────────────────────

fn bold() -> Font {
    Font {
        weight: Weight::Bold,
        ..Default::default()
    }
}

fn heading(label: &str) -> Element<'_, Message> {
    text(label)
        .size(theme::size::HEADING)
        .font(bold())
        .style(|_| text::Style { color: Some(theme::colours().text) })
        .into()
}

fn group_label(label: &str) -> Element<'_, Message> {
    container(text(label).size(theme::size::SMALL).font(bold()).style(|_| text::Style {
        color: Some(theme::colours().text_muted)
    }))
    .padding(iced::Padding { top: 8.0, ..Default::default() })
    .into()
}

fn hint(s: &str) -> Element<'_, Message> {
    text(s)
        .size(theme::size::CAPTION)
        .style(|_| text::Style { color: Some(theme::colours().text_hint) })
        .into()
}

fn readout(s: String) -> Element<'static, Message> {
    text(s)
        .size(theme::size::SMALL)
        .style(|_| text::Style { color: Some(theme::colours().ok) })
        .into()
}

fn value_row(label: &str, value: String) -> Element<'_, Message> {
    row![
        text(label).size(theme::size::BODY).style(|_| text::Style {
            color: Some(theme::colours().text_secondary)
        }),
        text(value).size(theme::size::BODY).font(bold()).style(|_| text::Style {
            color: Some(theme::colours().text)
        }),
    ]
    .spacing(theme::space::M)
    .align_y(Alignment::Center)
    .into()
}

fn labelled_slider<'a>(
    label: &'a str,
    slider: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    row![
        text(label).size(theme::size::CAPTION).width(Length::Fixed(52.0)).style(|_| text::Style {
            color: Some(theme::colours().text_muted)
        }),
        slider.into(),
    ]
    .spacing(theme::space::M)
    .align_y(Alignment::Center)
    .into()
}

fn card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .padding(12)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(theme::colours().surface_raised)),
            border: Border {
                color: theme::colours().border_subtle,
                width: 1.0,
                radius: theme::radius::CARD.into(),
            },
            ..Default::default()
        })
        .into()
}

fn rule() -> Element<'static, Message> {
    horizontal_rule(1.0).style(rule_style).into()
}

fn rule_style(_theme: &Theme) -> iced::widget::rule::Style {
    iced::widget::rule::Style {
        color: theme::colours().border_strong,
        width: 1,
        radius: 0.0.into(),
        fill_mode: iced::widget::rule::FillMode::Full,
    }
}

fn checkbox_style(_theme: &Theme, _status: checkbox::Status) -> checkbox::Style {
    checkbox::Style {
        background: Background::Color(theme::colours().control_hover),
        icon_color: theme::colours().text,
        border: Border {
            color: theme::colours().border_control,
            width: 1.0,
            radius: theme::radius::CONTROL.into(),
        },
        text_color: Some(theme::colours().text_secondary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `[dependencies]` entry in Cargo.toml must be credited, so adding
    /// a crate without a Licenses line fails here rather than going unnoticed.
    #[test]
    fn every_direct_dependency_is_credited() {
        let manifest = include_str!("../../../Cargo.toml");
        let deps = manifest
            .split("\n[dependencies]\n")
            .nth(1)
            .and_then(|rest| rest.split("\n[").next())
            .expect("Cargo.toml has a [dependencies] table");
        let credited: Vec<&str> =
            LIBRARIES.iter().flat_map(|c| c.crates.iter().copied()).collect();

        let mut missing = Vec::new();
        for line in deps.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, _)) = line.split_once('=') else { continue };
            let name = name.trim();
            if !credited.contains(&name) {
                missing.push(name);
            }
        }
        assert!(missing.is_empty(), "not credited in Preferences › Licenses: {missing:?}");
    }

    #[test]
    fn the_menu_lists_every_section_once_in_order() {
        for (i, s) in PreferencesSection::ALL.iter().enumerate() {
            assert_eq!(s.index(), i);
            assert!(!s.label().is_empty());
        }
    }
}
