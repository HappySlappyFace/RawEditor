use iced::{Alignment, Background, Color, Element, Length, Theme};
use iced::widget::{button, column, container, row, stack, text, Image, Space};
use iced::widget::image::Handle;

use crate::ui;
use crate::app::message::{Message, AppTab};
use crate::app::state::{RawEditor, Modal};
use crate::ui::icons::ICON_FONT;

use super::library::view_library;
use super::cull::view_cull;
use super::develop::view_develop;
use super::modals::{view_help_modal, view_preferences_modal, view_export_modal};

// Phase 69: Brand Identity
const LOGO_BYTES: &[u8] = include_bytes!("../../../assets/logo.png");

/// Build the user interface
pub fn view(editor: &RawEditor) -> Element<'_, Message> {
    // Phase 23: Show splash screen if database is still loading
    let content = match &editor.library {
        None => view_splash(editor),
        Some(_) => view_main(editor),
    };
    
    // Phase 84: Generic Modal System
    let modal = match editor.active_modal {
        Modal::None => container(text("")).height(0).width(0).into(),
        Modal::Help => container(view_help_modal()).style(ui::styles::modal_container_style).padding(20),
        Modal::Preferences => container(view_preferences_modal(editor)).style(ui::styles::modal_container_style).padding(20),
        Modal::Export => container(view_export_modal(editor)).style(ui::styles::modal_container_style).padding(20),
    };
    
    stack![
        content,
        modal
    ].into()
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
    
    // Phase 116: Root layout container with sleek dark polish
    container(column![title_bar, content])
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
fn view_title_bar(editor: &RawEditor) -> Element<'_, Message> {
    let menus = row![
        button(container(text("File").size(13)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).style(ui::styles::WindowControlButton::style).height(Length::Fill).padding([0, 10]),
        button(container(text("Edit").size(13)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center)).style(ui::styles::WindowControlButton::style).height(Length::Fill).padding([0, 10]).on_press(Message::OpenModal(crate::app::state::Modal::Preferences)),
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

/// Set the application theme
pub fn theme(_: &RawEditor) -> Theme {
    Theme::Dark
}
