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
    } else if editor.is_wb_picking {
        editor.is_wb_picking = false;
        editor.status.clear();
    }
    Task::none()
}

pub fn handle_toggle_info_hud(editor: &mut RawEditor) -> Task<Message> {
    editor.info_overlay = editor.info_overlay.next();
    Task::none()
}

pub fn handle_set_thumbnail_size(editor: &mut RawEditor, size: f32) -> Task<Message> {
    editor.thumbnail_size = size.clamp(100.0, 400.0);
    editor.save_preferences();
    Task::none()
}

pub fn handle_set_cache_capacity(editor: &mut RawEditor, val: f32) -> Task<Message> {
    let new_capacity = (val as usize).clamp(20, 500);
    editor.cache_capacity = new_capacity;
    if let Some(non_zero_cap) = std::num::NonZeroUsize::new(new_capacity) {
        editor.preview_cache.resize(non_zero_cap);
    }
    editor.save_preferences();
    Task::none()
}

pub fn handle_set_raw_preload_budget(editor: &mut RawEditor, val: f32) -> Task<Message> {
    let budget_mb = (val.max(0.0) as u32).min(4096);
    editor.raw_preload_budget_mb = budget_mb;

    if budget_mb == 0 {
        editor.raw_cache.clear();
        editor.raw_cache_bytes = 0;
        editor.pending_raw_loads.clear();
        editor.queued_raw_loads.clear();
    } else {
        editor.evict_raw_cache_to_budget();
    }

    editor.save_preferences();
    Task::none()
}

pub fn handle_set_preview_preload_behind(editor: &mut RawEditor, val: f32) -> Task<Message> {
    editor.preview_preload_behind =
        (val.max(0.0) as usize).min(crate::app::state::PREVIEW_PRELOAD_BEHIND_MAX);
    editor.save_preferences();
    Task::none()
}

pub fn handle_set_preview_preload_ahead(editor: &mut RawEditor, val: f32) -> Task<Message> {
    editor.preview_preload_ahead =
        (val.max(0.0) as usize).min(crate::app::state::PREVIEW_PRELOAD_AHEAD_MAX);
    editor.save_preferences();
    Task::none()
}

pub fn handle_set_raw_preload_behind(editor: &mut RawEditor, val: f32) -> Task<Message> {
    editor.raw_preload_behind =
        (val.max(0.0) as usize).min(crate::app::state::RAW_PRELOAD_BEHIND_MAX);
    editor.save_preferences();
    Task::none()
}

pub fn handle_set_raw_preload_ahead(editor: &mut RawEditor, val: f32) -> Task<Message> {
    editor.raw_preload_ahead =
        (val.max(0.0) as usize).min(crate::app::state::RAW_PRELOAD_AHEAD_MAX);
    editor.save_preferences();
    Task::none()
}

pub fn handle_reset_preferences(editor: &mut RawEditor) -> Task<Message> {
    let defaults = crate::core::settings::AppSettings::default();

    editor.cache_capacity = defaults.cache_capacity;
    if let Some(non_zero_cap) = std::num::NonZeroUsize::new(editor.cache_capacity) {
        editor.preview_cache.resize(non_zero_cap);
    }

    editor.raw_preload_budget_mb = defaults.raw_preload_budget_mb;
    editor.thumbnail_size = defaults.thumbnail_size;
    editor.auto_advance = defaults.auto_advance;
    editor.histogram_enabled = defaults.histogram_enabled;
    editor.preview_preload_behind = defaults.preview_preload_behind;
    editor.preview_preload_ahead = defaults.preview_preload_ahead;
    editor.raw_preload_behind = defaults.raw_preload_behind;
    editor.raw_preload_ahead = defaults.raw_preload_ahead;

    editor.evict_raw_cache_to_budget();
    editor.save_preferences();
    Task::none()
}

pub fn handle_histogram_toggled(editor: &mut RawEditor, enabled: bool) -> Task<Message> {
    editor.histogram_enabled = enabled;
    editor.save_preferences();
    Task::none()
}
