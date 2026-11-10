use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use iced::widget::{column, container, row, stack, text, Image, Space};
use crate::ui::widgets::button;
use iced::widget::image::Handle;

use crate::ui;
use crate::app::message::{Message, AppTab};
use crate::app::state::{RawEditor, Modal};
use crate::ui::icons::ICON_FONT;

use super::library::view_library;
use super::cull::view_cull;
use super::develop::view_develop;
use super::export::view_export;
use super::modals::{
    modal_overlay, view_about_modal, view_copy_settings_modal, view_delete_modal,
    view_export_modal, view_help_modal, view_remove_folder_modal,
};
use super::preferences::view_preferences_modal;
use crate::ui::theme;

/// Height of the custom title bar. Module-level because the menu bar has to be
/// given this height explicitly — see `view_title_bar`.
pub(crate) const TITLE_BAR_HEIGHT: f32 = 35.0;

// Phase 69: Brand Identity
const LOGO_BYTES: &[u8] = include_bytes!("../../../assets/logo.png");
const LOGO_LIGHT_BYTES: &[u8] = include_bytes!("../../../assets/logo-light.png");

/// The logo's image handle, built once.
///
/// `Handle::from_bytes` mints a fresh unique id on every call, and iced keys
/// its decoded-image cache by that id — so building the handle inside `view`
/// copied the PNG and decoded it again on every view rebuild, which iced
/// does after every message.
fn logo() -> Handle {
    static LOGO: std::sync::LazyLock<Handle> =
        std::sync::LazyLock::new(|| Handle::from_bytes(LOGO_BYTES));
    // The logo is opaque, on the title bar's own grey, so each theme has its
    // own (the light one is the dark one inverted onto `LIGHT.chrome`).
    static LOGO_LIGHT: std::sync::LazyLock<Handle> =
        std::sync::LazyLock::new(|| Handle::from_bytes(LOGO_LIGHT_BYTES));
    match crate::ui::theme::appearance() {
        crate::ui::theme::Appearance::Dark => LOGO.clone(),
        crate::ui::theme::Appearance::Light => LOGO_LIGHT.clone(),
    }
}

/// The splash artwork, looked up beside the executable first.
///
/// A bare relative path only resolved when the app was launched from the
/// repository root; started any other way (a desktop launcher, the release
/// zip) the splash was simply blank. Not embedded: it is ~10 MB. Inside a
/// macOS `.app` the executable sits in `Contents/MacOS` and the assets in
/// `Contents/Resources`, so that sibling is tried too.
fn splash() -> Handle {
    static SPLASH: std::sync::LazyLock<Handle> = std::sync::LazyLock::new(|| {
        let relative = std::path::Path::new("assets").join("splash.png");
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
            .and_then(|dir| {
                [dir.join(&relative), dir.join("../Resources").join(&relative)]
                    .into_iter()
                    .find(|path| path.exists())
            });
        Handle::from_path(beside_exe.unwrap_or(relative))
    });
    SPLASH.clone()
}

/// Build the user interface
pub fn view(editor: &RawEditor) -> Element<'_, Message> {
    // Phase 23: Show splash screen if database is still loading
    let content = match &editor.library {
        None => view_splash(editor),
        Some(_) => view_main(editor),
    };
    
    // Phase 84: Generic Modal System.
    //
    // Everything goes through `modal_overlay`, which supplies the dimmed
    // backdrop, click-outside-to-close, screen centering and the entrance /
    // exit motion. A modal that has just closed stays mounted, empty, for its
    // exit (`modal_exit`, maintained in `update`); its contents are not
    // rebuilt, since the state they describe is usually already gone (the
    // Delete dialog would read "0 images" on its way out).
    let (modal, exiting) = match (editor.active_modal, editor.modal_exit) {
        (Modal::None, Some(exit)) => (exit.modal, true),
        (active, _) => (active, false),
    };
    let modal_body: Option<Element<'_, Message>> = if exiting {
        Some(Space::new(Length::Shrink, Length::Shrink).into())
    } else {
        match modal {
            Modal::None => None,
            Modal::Help => Some(view_help_modal()),
            Modal::Preferences => Some(view_preferences_modal(editor)),
            Modal::Export => Some(view_export_modal(editor)),
            Modal::Delete => Some(view_delete_modal(editor)),
            Modal::CopySettings => Some(view_copy_settings_modal(editor)),
            Modal::About => Some(view_about_modal()),
            Modal::RemoveFolder => Some(view_remove_folder_modal(editor)),
            Modal::ImportDropped => Some(crate::app::views::modals::view_import_dropped_modal(editor)),
            Modal::AiModel => Some(crate::app::views::modals::view_ai_model_modal(editor)),
            Modal::AiDenoise => Some(crate::app::views::modals::view_ai_denoise_modal(editor)),
            Modal::UpdatePrompt => Some(crate::app::views::modals::view_update_prompt_modal(editor)),
            Modal::Update => Some(crate::app::views::modals::view_update_modal(editor)),
        }
    };

    // push_maybe rather than a zero-sized placeholder layer: Modal::None is the
    // common case and shouldn't allocate a widget tree every frame.
    stack![content]
        .push_maybe(modal_body.map(|body| modal_overlay(body, modal as u64, exiting)))
        .into()
}

/// Phase 23: Splash screen shown during database loading
fn view_splash(editor: &RawEditor) -> Element<'_, Message> {
    let left_content = column![
        Space::with_height(Length::Fill),
        iced::widget::image(splash())
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
            background: Some(Background::Color(theme::colours().bg)),
            ..Default::default()
        }
    });
    
    let right_panel = container(
        column![
            Space::with_height(Length::Fill),
            container(
                Image::new(logo())
                    .height(Length::Fixed(72.0))
                    .content_fit(iced::ContentFit::Contain)
            )
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center),
            Space::with_height(Length::Fixed(28.0)),
            container(
                column![
                    text(&editor.status)
                        .size(theme::size::BODY)
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center)
                        .style(|_| text::Style {
                            color: Some(theme::colours().text)
                        }),
                ]
                .spacing(theme::space::S)
            )
            .padding([theme::space::XL, theme::space::XL])
            .style(|_| container::Style {
                background: Some(Background::Color(theme::colours().surface)),
                border: iced::Border {
                    radius: theme::radius::DIALOG.into(),
                    width: 1.0,
                    color: theme::colours().border_subtle,
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .max_width(360),
            Space::with_height(Length::Fixed(16.0)),
            text("Work in progress build")
                .size(theme::size::CAPTION)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .style(|_| text::Style {
                    color: Some(theme::colours().text_disabled)
                }),
            text(format!("Version {}", crate::app::APP_VERSION))
                .size(theme::size::CAPTION)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .style(|_| text::Style {
                    color: Some(theme::colours().text_disabled)
                }),
            Space::with_height(Length::Fill),
        ]
        .align_x(Alignment::Center)
        .width(Length::Fill)
    )
    .padding([0, 30])
    .width(Length::FillPortion(3))
    .height(Length::Fill)
    .style(|_theme| {
        container::Style {
            background: Some(Background::Color(theme::colours().bg)),
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
        AppTab::Export => view_export(editor),
    };
    // Each tab fades up through the app background as it arrives (and the
    // whole workspace does once, as the splash hands over).
    let content = crate::ui::widgets::transition::fade_through(
        tab_index(editor.current_tab) as u64,
        app_background(),
        content,
    );
    
    // Phase 116: Root layout container with sleek dark polish
    //
    // The title bar is the LAST stack child (not the first column child) so it
    // always paints over `content` — some content widgets (notably scrolled
    // Image thumbnails) don't respect ancestor clip/scissor bounds and would
    // otherwise bleed over the bar. The Space reserves its 35px in the normal
    // layout flow so `content` is sized identically to before.
    // Profiler HUD lives here rather than inside view_develop so Window >
    // Performance HUD does something in every tab. Render stats are
    // develop-specific, hence None elsewhere.
    let profiler = editor.show_profiler.then(|| {
        let stats = (editor.current_tab == AppTab::Develop).then(|| {
            crate::ui::widgets::profiler_graph::RenderStats {
                zoom_percent: editor.zoom_percent(),
                target: editor.rendered_preview_dims,
                view_extent: (editor.rendered_view_rect[2], editor.rendered_view_rect[3]),
            }
        });
        container(crate::ui::widgets::profiler_graph::view_profiler_overlay(
            &editor.profiler,
            &editor.profiler_cache,
            stats,
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Left)
        .align_y(iced::alignment::Vertical::Top)
        .padding([TITLE_BAR_HEIGHT + 10.0, 10.0])
    });

    container(
        stack![
            column![Space::with_height(Length::Fixed(TITLE_BAR_HEIGHT)), content]
                .width(Length::Fill)
                .height(Length::Fill),
            container(title_bar)
                .width(Length::Fill)
                .align_y(iced::alignment::Vertical::Top),
        ]
        .push_maybe(profiler),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_theme| container::Style {
        background: Some(Background::Color(app_background())),
        text_color: Some(theme::colours().text_strong),
        ..Default::default()
    })
    .into()
}

/// The workspace background, which tab content fades up from.
fn app_background() -> Color {
    theme::colours().bg
}

/// Title bar tabs, in order. Their position is what the sliding indicator
/// animates between.
const TABS: [(AppTab, &str, &str); 4] = [
    (AppTab::Library, ui::icons::FOLDER, "Library"),
    (AppTab::Cull, ui::icons::CHECK, "Cull"),
    (AppTab::Develop, ui::icons::PAINTBRUSH, "Develop"),
    (AppTab::Export, ui::icons::SAVE, "Export"),
];

/// Every tab is the same width, so the indicator's position is arithmetic.
/// Wide enough for the longest label ("Library"/"Develop") plus padding.
const TAB_WIDTH: f32 = 112.0;

fn tab_index(tab: AppTab) -> usize {
    TABS.iter().position(|(t, _, _)| *t == tab).unwrap_or(0)
}

/// Build the custom window title bar
/// One row inside a dropdown: full-width, left-aligned, with an optional
/// check mark column so toggles and plain actions line up.
fn menu_entry<'a>(label: &'a str, checked: Option<bool>, msg: Message) -> Element<'a, Message> {
    let mark = match checked {
        Some(true) => text(ui::icons::CHECK).font(ICON_FONT).size(theme::size::CAPTION),
        Some(false) | None => text(" ").size(theme::size::CAPTION),
    };
    button(
        row![
            container(mark).width(Length::Fixed(16.0)),
            text(label).size(theme::size::BODY),
        ]
        .spacing(theme::space::XS)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([theme::space::S, theme::space::L])
    .style(ui::styles::WindowControlButton::style)
    .on_press(Message::MenuItem(Box::new(msg)))
    .into()
}

/// A thin rule used to group menu entries.
fn menu_separator<'a>() -> Element<'a, Message> {
    container(iced::widget::horizontal_rule(1.0).style(|_| iced::widget::rule::Style {
        color: theme::colours().border,
        width: 1,
        radius: 0.0.into(),
        fill_mode: iced::widget::rule::FillMode::Full,
    }))
    .padding([theme::space::XS, theme::space::S])
    .into()
}

fn view_title_bar(editor: &RawEditor) -> Element<'_, Message> {
    use iced_aw::menu::{Item, Menu};

    // Top-level labels keep the old button styling so the bar looks unchanged
    // until something is clicked.
    let label = |name: &'static str| {
        button(
            container(text(name).size(theme::size::BODY))
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Center),
        )
        .style(ui::styles::WindowControlButton::style)
        .height(Length::Fill)
        .padding([0, theme::space::L])
        // A menu root still needs an on_press or iced treats it as disabled
        // and greys the label; ModalNoOp is the established no-op message.
        .on_press(Message::ModalNoOp)
    };

    // Free function, not a closure: closure lifetime elision binds the argument
    // and return lifetimes to different regions and won't compile.
    fn dropdown<'a>(
        items: Vec<Item<'a, Message, Theme, iced::Renderer>>,
    ) -> Menu<'a, Message, Theme, iced::Renderer> {
        Menu::new(items).max_width(220.0).offset(2.0).spacing(0.0)
    }

    let menus = iced_aw::menu::MenuBar::new(vec![
        Item::with_menu(
            label("File"),
            dropdown(vec![
                Item::new(menu_entry("Import Folder…", None, Message::ImportFolder)),
                Item::new(menu_separator()),
                Item::new(menu_entry("Quit", None, Message::CloseWindow)),
            ]),
        ),
        Item::with_menu(
            label("Edit"),
            dropdown(vec![Item::new(menu_entry(
                "Preferences…",
                None,
                Message::OpenModal(crate::app::state::Modal::Preferences),
            ))]),
        ),
        Item::with_menu(
            label("Window"),
            dropdown(vec![
                Item::new(menu_entry(
                    "Performance HUD",
                    Some(editor.show_profiler),
                    Message::ToggleProfiler,
                )),
                Item::new(menu_entry(
                    "Info Overlay",
                    Some(editor.info_overlay != crate::app::state::InfoOverlayState::Hidden),
                    Message::ToggleInfoHud,
                )),
            ]),
        ),
        Item::with_menu(
            label("Help"),
            dropdown(
                [
                    Some(Item::new(menu_entry(
                        "Keyboard Shortcuts…",
                        None,
                        Message::OpenModal(crate::app::state::Modal::Help),
                    ))),
                    // A distro package is updated by its package manager.
                    crate::core::updates::packaged().is_none().then(|| {
                        Item::new(menu_entry("Check for Updates", None, Message::CheckForUpdates))
                    }),
                    Some(Item::new(menu_separator())),
                    Some(Item::new(menu_entry(
                        "About",
                        None,
                        Message::OpenModal(crate::app::state::Modal::About),
                    ))),
                ]
                .into_iter()
                .flatten()
                .collect(),
            ),
        ),
    ])
    .draw_path(iced_aw::menu::DrawPath::Backdrop)
    .padding(0)
    // MUST be explicit. `MenuBar::new` defaults to `height: Shrink`, and the
    // root labels are `height: Fill` — a Fill child inside a Shrink flex
    // container collapses to nothing, so the bar's bounds ended up ~0px tall.
    // `on_event` gates opening on `cursor.is_over(bar_bounds)`, so the menu
    // could never open, `overlay()` always returned None, and the dropdowns
    // silently did not exist. No error, no warning — just no menu.
    .height(Length::Fixed(TITLE_BAR_HEIGHT))
    .style(|_theme, _status| iced_aw::style::menu_bar::Style {
        // The title bar paints its own background; the bar itself must not
        // add a second one over it.
        bar_background: Background::Color(Color::TRANSPARENT),
        bar_border: Border::default(),
        bar_shadow: iced::Shadow::default(),
        bar_background_expand: 0.into(),

        // Dropdown panels match the modal card so the app has one surface look.
        menu_background: Background::Color(theme::colours().surface_raised),
        menu_border: Border {
            radius: theme::radius::CARD.into(),
            width: 1.0,
            color: theme::colours().border_strong,
        },
        menu_shadow: iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: iced::Vector::new(0.0, 2.0),
            blur_radius: 12.0,
        },
        menu_background_expand: 4.into(),

        path: Background::Color(theme::colours().control_hover),
        path_border: Border {
            radius: theme::radius::CONTROL.into(),
            ..Default::default()
        },
    });

    // Choosing an entry must close its dropdown, as native menus do. iced_aw
    // 0.10 keeps it open until the pointer leaves, and an open dropdown
    // captures *every* event — `RedrawRequested` included — so nothing
    // beneath it animates: a dialog opened from the menu sat invisible at
    // its first frame. Entries send `MenuItem`, which bumps the generation;
    // alternating the wrapper changes the widget tag at this position, which
    // remounts the bar with fresh (closed) state. (A `column`, not a
    // `container`: a container is transparent to the widget tree — it hands
    // over its content's tag and state — so wrapping in one changes nothing.)
    let menus: Element<'_, Message> = if editor.menu_generation.is_multiple_of(2) {
        menus.into()
    } else {
        column![menus].into()
    };
    // And an open dropdown must never be handed a widget operation: iced_aw
    // panics on one (see `ui::widgets::no_operate`), and a scroll task — the
    // filmstrip still coasting — can arrive at any moment.
    let menus: Element<'_, Message> = crate::ui::widgets::no_operate::no_operate(menus).into();

    // The tabs sit over a highlight that slides (and stretches) between
    // them — `ui::widgets::indicator`. The buttons themselves are transparent
    // when active and only tint on hover, so the highlight shows through.
    let tab_buttons = TABS.iter().fold(row![].height(Length::Fill), |tabs, &(tab, icon, label)| {
        let is_active = editor.current_tab == tab;
        tabs.push(
            button(
                container(
                    row![text(icon).font(ICON_FONT).size(theme::size::LABEL), text(label).size(theme::size::LABEL)]
                        .spacing(theme::space::M)
                        .align_y(Alignment::Center),
                )
                .height(Length::Fill)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Center),
            )
            .width(Length::Fixed(TAB_WIDTH))
            .height(Length::Fill)
            .padding(0)
            .style(move |t, s| ui::styles::TabButton { is_active }.style(t, s))
            .on_press(Message::TabChanged(tab)),
        )
    });
    let highlight = crate::ui::widgets::indicator::indicator(
        TABS.len(),
        tab_index(editor.current_tab),
        TAB_WIDTH,
        crate::ui::widgets::indicator::Appearance {
            fill: theme::colours().surface_raised,
            accent: theme::colours().accent,
            accent_height: 2.0,
            accent_inset: 22.0,
        },
    );
    let navigation = container(stack![highlight, tab_buttons].height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center);

    let controls = row![
        container(Image::new(logo()).height(Length::Fixed(24.0)).content_fit(iced::ContentFit::Contain)).height(Length::Fill).align_y(iced::alignment::Vertical::Center).padding([theme::space::M, theme::space::XXL]),
        iced::widget::Space::with_width(Length::Fixed(5.0)),
        button(container(text(ui::icons::MINIMIZE).font(ICON_FONT).size(theme::size::LABEL)).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).on_press(Message::MinimizeWindow).style(ui::styles::WindowControlButton::style).width(Length::Fixed(45.0)).height(Length::Fill),
        button(container(text(ui::icons::MAXIMIZE).font(ICON_FONT).size(theme::size::LABEL)).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).on_press(Message::MaximizeWindow).style(ui::styles::WindowControlButton::style).width(Length::Fixed(45.0)).height(Length::Fill),
        button(container(text(ui::icons::CLOSE).font(ICON_FONT).size(theme::size::LABEL)).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).on_press(Message::CloseWindow)
            .style(|_theme, status| if status == button::Status::Hovered { button::Style { background: Some(Background::Color(theme::colours().danger_hover)), text_color: theme::colours().on_accent, ..button::Style::default() } } else { button::Style { text_color: theme::colours().text_secondary, ..button::text(_theme, status) } })
            .width(Length::Fixed(45.0)).height(Length::Fill),
    ].spacing(0).height(Length::Fill).align_y(Alignment::Center);

    container(
        stack![
            // Bottom layer: the entire bar is draggable. Buttons/tabs stacked
            // above capture their own clicks first (iced dispatches stack
            // events last-child-first), so this only fires on empty space.
            iced::widget::mouse_area(Space::new(Length::Fill, Length::Fill))
                .on_press(Message::DragWindow),
            row![menus, Space::with_width(Length::Fill)]
                .push_maybe(update_badge(editor))
                .push(controls)
                .height(Length::Fill)
                .align_y(Alignment::Center)
                .padding(0),
            navigation,
        ]
    ).height(Length::Fixed(35.0)).style(|_theme| container::Style { background: Some(Background::Color(theme::colours().chrome)), ..Default::default() }).into()
}

/// "Update available" in the title bar, after a check found a newer
/// release. Opens the update dialog.
fn update_badge(editor: &RawEditor) -> Option<Element<'_, Message>> {
    let available = editor.updates.available.as_ref()?;
    Some(
        container(
            button(
                row![
                    text("\u{f0aa}").font(ICON_FONT).size(theme::size::SMALL), // arrow-circle-up
                    text(format!("Update {}", available.version)).size(theme::size::SMALL),
                ]
                .spacing(theme::space::S)
                .align_y(Alignment::Center),
            )
            .on_press(Message::OpenModal(Modal::Update))
            .padding([theme::space::XS, theme::space::L])
            .style(ui::styles::AccentButton::style),
        )
        .height(Length::Fill)
        .align_y(iced::alignment::Vertical::Center)
        .padding([0, theme::space::L])
        .into(),
    )
}

/// Set the application theme
pub fn theme(_: &RawEditor) -> Theme {
    crate::ui::theme::iced_theme()
}
