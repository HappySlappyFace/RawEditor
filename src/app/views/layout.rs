use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use iced::widget::{button, column, container, row, stack, text, Image, Space};
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
    view_export_modal, view_help_modal, view_preferences_modal, view_remove_folder_modal,
};

/// Height of the custom title bar. Module-level because the menu bar has to be
/// given this height explicitly — see `view_title_bar`.
const TITLE_BAR_HEIGHT: f32 = 35.0;

// Phase 69: Brand Identity
const LOGO_BYTES: &[u8] = include_bytes!("../../../assets/logo.png");

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
    // backdrop, click-outside-to-close, and — the part that was missing —
    // screen centering. `stack` places non-base layers at their own intrinsic
    // size at the origin with no alignment step, so the old bare shrink-wrapped
    // container pinned every modal to the top-left corner.
    let modal_body: Option<Element<'_, Message>> = match editor.active_modal {
        Modal::None => None,
        Modal::Help => Some(view_help_modal()),
        Modal::Preferences => Some(view_preferences_modal(editor)),
        Modal::Export => Some(view_export_modal(editor)),
        Modal::Delete => Some(view_delete_modal(editor)),
        Modal::CopySettings => Some(view_copy_settings_modal(editor)),
        Modal::About => Some(view_about_modal()),
        Modal::RemoveFolder => Some(view_remove_folder_modal(editor)),
    };

    // push_maybe rather than a zero-sized placeholder layer: Modal::None is the
    // common case and shouldn't allocate a widget tree every frame.
    stack![content]
        .push_maybe(modal_body.map(modal_overlay))
        .into()
}

/// Phase 23: Splash screen shown during database loading
fn view_splash(editor: &RawEditor) -> Element<'_, Message> {
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
            background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
            ..Default::default()
        }
    });
    
    let right_panel = container(
        column![
            Space::with_height(Length::Fill),
            container(
                Image::new(Handle::from_bytes(LOGO_BYTES.to_vec()))
                    .height(Length::Fixed(72.0))
                    .content_fit(iced::ContentFit::Contain)
            )
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center),
            Space::with_height(Length::Fixed(28.0)),
            container(
                column![
                    text(&editor.status)
                        .size(13)
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center)
                        .style(|_| text::Style {
                            color: Some(Color::from_rgb(0.82, 0.82, 0.82))
                        }),
                ]
                .spacing(6)
            )
            .padding([12, 14])
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.11, 0.11, 0.11))),
                border: iced::Border {
                    radius: 10.0.into(),
                    width: 1.0,
                    color: Color::from_rgb(0.18, 0.18, 0.18),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .max_width(360),
            Space::with_height(Length::Fixed(16.0)),
            text("Work in progress build")
                .size(11)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.42, 0.42, 0.42))
                }),
            text(format!("Version {}", crate::app::APP_VERSION))
                .size(11)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .style(|_| text::Style {
                    color: Some(Color::from_rgb(0.38, 0.38, 0.38))
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
            background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
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
        background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.08))),
        text_color: Some(Color::WHITE),
        ..Default::default()
    })
    .into()
}

/// Build the custom window title bar
/// One row inside a dropdown: full-width, left-aligned, with an optional
/// check mark column so toggles and plain actions line up.
fn menu_entry<'a>(label: &'a str, checked: Option<bool>, msg: Message) -> Element<'a, Message> {
    let mark = match checked {
        Some(true) => text(ui::icons::CHECK).font(ICON_FONT).size(11),
        Some(false) | None => text(" ").size(11),
    };
    button(
        row![
            container(mark).width(Length::Fixed(16.0)),
            text(label).size(13),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([6, 10])
    .style(ui::styles::WindowControlButton::style)
    .on_press(msg)
    .into()
}

/// A thin rule used to group menu entries.
fn menu_separator<'a>() -> Element<'a, Message> {
    container(iced::widget::horizontal_rule(1.0).style(|_| iced::widget::rule::Style {
        color: Color::from_rgb(0.25, 0.25, 0.25),
        width: 1,
        radius: 0.0.into(),
        fill_mode: iced::widget::rule::FillMode::Full,
    }))
    .padding([4, 6])
    .into()
}

fn view_title_bar(editor: &RawEditor) -> Element<'_, Message> {
    use iced_aw::menu::{Item, Menu};

    // Top-level labels keep the old button styling so the bar looks unchanged
    // until something is clicked.
    let label = |name: &'static str| {
        button(
            container(text(name).size(13))
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Center),
        )
        .style(ui::styles::WindowControlButton::style)
        .height(Length::Fill)
        .padding([0, 10])
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
            dropdown(vec![
                Item::new(menu_entry(
                    "Keyboard Shortcuts…",
                    None,
                    Message::OpenModal(crate::app::state::Modal::Help),
                )),
                Item::new(menu_entry(
                    "Check for Updates",
                    None,
                    Message::CheckForUpdates,
                )),
                Item::new(menu_separator()),
                Item::new(menu_entry(
                    "About",
                    None,
                    Message::OpenModal(crate::app::state::Modal::About),
                )),
            ]),
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
        menu_background: Background::Color(Color::from_rgb(0.12, 0.12, 0.12)),
        menu_border: Border {
            radius: 6.0.into(),
            width: 1.0,
            color: Color::from_rgb(0.28, 0.28, 0.28),
        },
        menu_shadow: iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: iced::Vector::new(0.0, 2.0),
            blur_radius: 12.0,
        },
        menu_background_expand: 4.into(),

        path: Background::Color(Color::from_rgb(0.22, 0.22, 0.22)),
        path_border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
    });

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

            button(container(row![text(ui::icons::SAVE).font(ICON_FONT).size(14), text(" Export").size(14)].spacing(5).align_y(Alignment::Center)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .height(Length::Fill).padding([0, 15])
                .style(|t, s| ui::styles::TabButton { is_active: editor.current_tab == AppTab::Export }.style(t, s))
                .on_press(Message::TabChanged(AppTab::Export)),
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
            // Bottom layer: the entire bar is draggable. Buttons/tabs stacked
            // above capture their own clicks first (iced dispatches stack
            // events last-child-first), so this only fires on empty space.
            iced::widget::mouse_area(Space::new(Length::Fill, Length::Fill))
                .on_press(Message::DragWindow),
            row![menus, Space::with_width(Length::Fill), controls]
                .height(Length::Fill)
                .align_y(Alignment::Center)
                .padding(0),
            navigation,
        ]
    ).height(Length::Fixed(35.0)).style(|_theme| container::Style { background: Some(Background::Color(Color::from_rgb(0.05, 0.05, 0.05))), ..Default::default() }).into()
}

/// Set the application theme
pub fn theme(_: &RawEditor) -> Theme {
    Theme::Dark
}
