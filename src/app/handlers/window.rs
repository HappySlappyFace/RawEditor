use iced::Task;
use crate::app::state::{RawEditor, Modal};
use crate::app::message::Message;
use iced::window;

pub fn handle_minimize_window() -> Task<Message> {
    window::get_latest().and_then(|id| window::minimize(id, true))
}

pub fn handle_maximize_window() -> Task<Message> {
    window::get_latest().and_then(|id| window::maximize(id, true))
}

pub fn handle_close_window() -> Task<Message> {
    window::get_latest().and_then(window::close)
}

pub fn handle_drag_window() -> Task<Message> {
    window::get_latest().and_then(window::drag)
}

pub fn handle_open_modal(editor: &mut RawEditor, modal: Modal) -> Task<Message> {
    editor.active_modal = modal;
    Task::none()
}

pub fn handle_close_modal(editor: &mut RawEditor) -> Task<Message> {
    editor.active_modal = Modal::None;
    Task::none()
}

pub fn handle_escape(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        editor.active_modal = Modal::None;
    }
    Task::none()
}

pub fn handle_toggle_info_hud(editor: &mut RawEditor) -> Task<Message> {
    editor.info_overlay = editor.info_overlay.next();
    Task::none()
}

pub fn handle_set_thumbnail_size(editor: &mut RawEditor, size: f32) -> Task<Message> {
    editor.thumbnail_size = size;
    Task::none()
}

pub fn handle_set_cache_capacity(editor: &mut RawEditor, val: f32) -> Task<Message> {
    let new_capacity = val as usize;
    editor.cache_capacity = new_capacity;
    if let Some(non_zero_cap) = std::num::NonZeroUsize::new(new_capacity) {
        editor.preview_cache.resize(non_zero_cap);
    }
    Task::none()
}

pub fn handle_histogram_toggled(editor: &mut RawEditor, enabled: bool) -> Task<Message> {
    editor.histogram_enabled = enabled;
    Task::none()
}
