// The line below is to remove all the warnings for unused methods, fields, etc. To optimize token use
#![allow(dead_code)]

mod app;
pub mod color;
mod debug;
mod gpu;
mod raw;
mod state;
mod ui;

use iced::Font;

pub fn main() -> iced::Result {
    iced::application(
        app::state::RawEditor::title,
        app::update::update,
        app::view::view,
    )
    .subscription(app::state::RawEditor::subscription)
    .theme(app::view::theme)
    .font(include_bytes!("../assets/fonts/icons.ttf"))
    .default_font(Font::with_name("JetBrainsMono Nerd Font"))
    .window(iced::window::Settings {
        size: iced::Size::new(900.0, 350.0),
        min_size: Some(iced::Size::new(600.0, 350.0)),
        decorations: false,
        ..Default::default()
    })
    .centered()
    .run_with(app::state::RawEditor::new)
}
