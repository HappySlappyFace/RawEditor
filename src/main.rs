// Tests build params up field by field (`let mut p = default(); p.x = …`),
// which reads better than one giant literal. Production code is still held to
// the lint by the non-test build.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]
// No console window behind the app in release builds (debug keeps it for logs).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
pub mod color;
mod core;
mod database;
mod debug;
mod gpu;
mod raw;
mod ui;

use iced::Font;

/// The app icon (a crop of the wordmark for now): the window's own, and on
/// Linux the desktop entry's (`core::desktop`).
pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

pub fn main() -> iced::Result {
    let _guard = core::logging::init();
    #[cfg(target_os = "linux")]
    std::thread::spawn(|| match core::desktop::install() {
        Ok(true) => tracing::info!("Desktop entry installed for {}", core::desktop::APP_ID),
        Ok(false) => {}
        Err(e) => tracing::warn!("Desktop entry not installed: {e}"),
    });
    iced::application(
        app::state::RawEditor::title,
        app::update::update,
        app::view::view,
    )
    .subscription(app::state::RawEditor::subscription)
    .theme(app::view::theme)
    .font(ui::icons::ICON_FONT_BYTES)
    .default_font(Font::with_name("JetBrainsMono Nerd Font"))
    .window(iced::window::Settings {
        size: iced::Size::new(900.0, 350.0),
        min_size: Some(iced::Size::new(600.0, 350.0)),
        decorations: false,
        icon: iced::window::icon::from_file_data(APP_ICON_PNG, None).ok(),
        #[cfg(target_os = "linux")]
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: core::desktop::APP_ID.to_string(),
            ..Default::default()
        },
        ..Default::default()
    })
    .centered()
    .run_with(app::state::RawEditor::new)
}
