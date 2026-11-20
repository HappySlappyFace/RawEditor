// Tests build params up field by field (`let mut p = default(); p.x = …`),
// which reads better than one giant literal. Production code is still held to
// the lint by the non-test build.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]
// No console window behind the app in release builds (debug keeps it for logs).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod debug;
#[cfg(target_os = "linux")]
mod desktop;
mod ui;

// The engine (GPU pipeline, RAW decoding, colour science, catalogue, AI) is
// the `raw-editor-core` crate. Importing its modules at the root keeps every
// `crate::core::…`/`crate::gpu::…` path in the app working unchanged.
use raw_editor_core::{color, core, database, gpu, ml, raw};

use iced::Font;

/// The app icon (a crop of the wordmark for now): the window's own, and on
/// Linux the desktop entry's (`desktop`).
pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

pub fn main() -> iced::Result {
    if std::env::args().nth(1).is_some_and(|a| a == "--version" || a == "-V") {
        println!("raw-editor {}", core::updates::current_version());
        return Ok(());
    }
    let _guard = core::logging::init();
    // A packaged install ships its own entry in /usr/share/applications.
    #[cfg(target_os = "linux")]
    if core::updates::packaged().is_none() {
        std::thread::spawn(|| match desktop::install() {
            Ok(true) => tracing::info!("Desktop entry installed for {}", desktop::APP_ID),
            Ok(false) => {}
            Err(e) => tracing::warn!("Desktop entry not installed: {e}"),
        });
    }
    iced::application(
        app::state::RawEditor::title,
        app::update::update,
        app::view::view,
    )
    .subscription(app::state::RawEditor::subscription)
    .theme(app::view::theme)
    .font(ui::icons::ICON_FONT_BYTES)
    .font(ui::theme::font::BOLD_BYTES)
    .default_font(Font::with_name("JetBrainsMono Nerd Font"))
    .window(iced::window::Settings {
        size: iced::Size::new(1280.0, 720.0),
        // Wide enough for the title bar: the centred tabs (4 x 112 px) must clear
        // the menus on the left and the window buttons on the right. With the
        // update badge showing the bar needs about 1000 px, so it can crowd the
        // right-hand buttons just above this minimum.
        min_size: Some(iced::Size::new(960.0, 540.0)),
        decorations: false,
        // Closing goes through `Message::CloseWindow` (flush, save the proxy).
        exit_on_close_request: false,
        icon: iced::window::icon::from_file_data(APP_ICON_PNG, None).ok(),
        #[cfg(target_os = "linux")]
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: desktop::APP_ID.to_string(),
            ..Default::default()
        },
        ..Default::default()
    })
    .centered()
    .run_with(app::state::RawEditor::new)
}
