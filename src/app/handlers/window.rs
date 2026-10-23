use iced::Task;
use crate::app::state::{RawEditor, Modal};
use crate::app::message::Message;
use iced::window;

/// Canonical project URL, shown in About and opened by both Help actions.
pub const REPO_URL: &str = "https://github.com/HappySlappyFace/RawEditor";
/// Where "Check for Updates" goes. Deliberately just a page: querying the
/// GitHub API would mean an HTTP client, TLS, and a release-parsing format to
/// keep in sync, for something the user can read in a second.
pub const RELEASES_URL: &str = "https://github.com/HappySlappyFace/RawEditor/releases";

/// Hand a URL to the platform's default browser.
///
/// Spawned rather than waited on: `xdg-open` can block for a noticeable moment
/// while it resolves a handler, and this runs on the update thread. Failure is
/// logged, never fatal — the URL is also displayed in the About dialog so the
/// user can copy it by hand.
fn open_url(url: &str) {
    let result = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };

    if let Err(e) = result {
        tracing::error!("Failed to open {url}: {e}");
    }
}

/// Show a folder in the platform's file manager, creating it first (the
/// cache folder doesn't exist until something has been imported).
///
/// Blocking (it touches the disk): run it inside `spawn_blocking`. Windows
/// gets `explorer` rather than `cmd /c start`, whose parsing mangles paths
/// containing `&` or `^`.
fn reveal_folder(path: &std::path::Path) {
    if let Err(e) = std::fs::create_dir_all(path) {
        tracing::error!("Failed to create {}: {e}", path.display());
        return;
    }
    let opener = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(opener).arg(path).spawn() {
        Ok(_) => tracing::info!("Opened {}", path.display()),
        Err(e) => tracing::error!("Failed to open {}: {e}", path.display()),
    }
}

/// Preferences side menu. The new page starts at the top: the content pane is
/// one scrollable whatever the page, so it would otherwise keep the previous
/// page's offset.
pub fn handle_set_preferences_section(
    editor: &mut RawEditor,
    section: crate::app::state::PreferencesSection,
) -> Task<Message> {
    if editor.preferences_section == section {
        return Task::none();
    }
    editor.preferences_section = section;
    let status = match section {
        crate::app::state::PreferencesSection::LensProfiles => crate::app::handlers::lensfun::ensure_loaded(),
        crate::app::state::PreferencesSection::CameraProfiles => {
            crate::app::handlers::camera_profiles::ensure_loaded()
        }
        _ => Task::none(),
    };
    Task::batch([
        status,
        iced::widget::scrollable::snap_to(
            crate::app::views::preferences::content_scroll_id(),
            iced::widget::scrollable::RelativeOffset::START,
        ),
    ])
}

pub fn handle_open_link(editor: &mut RawEditor, url: &'static str) -> Task<Message> {
    open_url(url);
    editor.status = format!("Opened {url}");
    Task::none()
}

pub fn handle_open_app_folder(
    editor: &mut RawEditor,
    folder: crate::core::paths::AppFolder,
) -> Task<Message> {
    let Some(path) = folder.path() else {
        editor.status = "This platform reports no cache folder".to_string();
        return Task::none();
    };
    editor.status = format!("Opened {}", path.display());
    Task::future(tokio::task::spawn_blocking(move || reveal_folder(&path))).discard()
}

pub fn handle_open_repository(editor: &mut RawEditor) -> Task<Message> {
    open_url(REPO_URL);
    editor.status = format!("Opened {REPO_URL}");
    Task::none()
}

pub fn handle_check_for_updates(editor: &mut RawEditor) -> Task<Message> {
    open_url(RELEASES_URL);
    editor.status = format!("Current version {}", env!("CARGO_PKG_VERSION"));
    Task::none()
}

/// Ask the window for its current DPI scale factor.
///
/// Called at startup and after every resize: iced has no dedicated rescale
/// event, but moving to a monitor with a different DPI resizes the window.
pub fn query_scale_factor() -> Task<Message> {
    window::get_latest()
        .and_then(window::get_scale_factor)
        .map(Message::ScaleFactorChanged)
}

/// The develop render target is sized in physical pixels, so a new scale
/// factor needs a new render to stay sharp.
pub fn handle_scale_factor_changed(editor: &mut RawEditor, scale: f32) -> Task<Message> {
    if !scale.is_finite() || scale <= 0.0 || (scale - editor.scale_factor).abs() < 0.001 {
        return Task::none();
    }
    tracing::info!("Display scale factor: {scale}");
    editor.scale_factor = scale;
    if editor.current_tab == crate::app::message::AppTab::Develop
        && matches!(editor.editor_readiness, crate::app::state::EditorReadiness::Ready(_))
    {
        return crate::app::handlers::navigation::throttled_render(editor);
    }
    Task::none()
}

pub fn handle_minimize_window() -> Task<Message> {
    window::get_latest().and_then(|id| window::minimize(id, true))
}

pub fn handle_maximize_window() -> Task<Message> {
    window::get_latest().and_then(window::toggle_maximize)
}

pub fn handle_close_window(editor: &mut RawEditor) -> Task<Message> {
    // Last chance to persist: a slider moved but never released still has its
    // edits only in memory.
    editor.flush_edits();
    // And the open image's edited thumbnail, synchronously: a spawned save
    // would never get to run.
    crate::app::handlers::proxy::persist_now(editor);
    window::get_latest().and_then(window::close)
}

pub fn handle_drag_window(editor: &mut RawEditor) -> Task<Message> {
    let now = std::time::Instant::now();
    let double = editor
        .last_titlebar_click
        .map(|t| now.duration_since(t).as_millis() < 400)
        .unwrap_or(false);
    editor.last_titlebar_click = Some(now);
    if double {
        editor.last_titlebar_click = None; // don't chain into a triple-click drag
        window::get_latest().and_then(window::toggle_maximize)
    } else {
        window::get_latest().and_then(window::drag)
    }
}

pub fn handle_open_modal(editor: &mut RawEditor, modal: Modal) -> Task<Message> {
    editor.active_modal = modal;
    // Preferences reopens on its last page; Lens Profiles shows what the
    // database holds, which may not have been read yet.
    let status = match (modal, editor.preferences_section) {
        (Modal::Preferences, crate::app::state::PreferencesSection::LensProfiles) => {
            crate::app::handlers::lensfun::ensure_loaded()
        }
        (Modal::Preferences, crate::app::state::PreferencesSection::CameraProfiles) => {
            crate::app::handlers::camera_profiles::ensure_loaded()
        }
        _ => Task::none(),
    };
    // A modal takes the keyboard, so Space will never deliver its release
    // edge — leave the before/after peek before it can get stranded.
    Task::batch([status, crate::app::handlers::develop::clear_before_peek(editor)])
}

/// Enter — route to whichever modal is open.
///
/// Centralised because `subscription()` emits at most one message per key
/// event, so modals cannot each bind Enter for themselves. Adding a
/// confirmable modal means adding an arm here.
pub fn handle_modal_confirm(editor: &mut RawEditor) -> Task<Message> {
    match editor.active_modal {
        Modal::Delete => crate::app::handlers::delete::handle_delete_from_disk_confirmed(editor),
        Modal::CopySettings => {
            crate::app::handlers::develop::handle_copy_settings_confirmed(editor)
        }
        Modal::Export => crate::app::handlers::export::handle_export_confirmed(editor),
        Modal::RemoveFolder => {
            crate::app::handlers::library::handle_remove_folder_confirmed(editor)
        }
        // Help, Preferences and About have nothing to confirm; Enter closes
        // them, which is the least surprising thing a dialog can do.
        Modal::Help | Modal::Preferences | Modal::About => handle_close_modal(editor),
        Modal::None => Task::none(),
    }
}

pub fn handle_close_modal(editor: &mut RawEditor) -> Task<Message> {
    editor.active_modal = Modal::None;
    editor.pending_delete_ids.clear();
    editor.pending_remove_folder = None;
    Task::none()
}

pub fn handle_escape(editor: &mut RawEditor) -> Task<Message> {
    if editor.slider_entry.take().is_some() {
        // Leaving a typed slider value is all this press means.
    } else if editor.active_modal != Modal::None {
        editor.active_modal = Modal::None;
        editor.pending_delete_ids.clear();
        editor.pending_remove_folder = None;
    } else if editor.is_wb_picking {
        editor.is_wb_picking = false;
        editor.status.clear();
    } else if editor.loupe.targeting {
        editor.loupe.targeting = false;
        editor.status.clear();
    } else if editor.is_leveling {
        editor.is_leveling = false;
        editor.status.clear();
    } else if editor.mask_tool != crate::app::state::MaskTool::Inactive {
        editor.mask_tool = crate::app::state::MaskTool::Inactive;
        editor.status.clear();
    } else if editor.selected_mask.is_some() {
        editor.selected_mask = None;
        editor.canvas_cache.clear();
    } else if editor.multi_selection.len() > 1 {
        // Last in the chain: collapse a multi-selection back to just the
        // current image. Only fires when there is a real multi-selection, so
        // Escape never clears an ordinary single selection out from under the
        // user.
        editor.multi_selection.clear();
        if let Some(id) = editor.selected_image_id {
            editor.multi_selection.insert(id);
        }
        editor.selection_anchor = editor.selected_image_id;
        editor.status.clear();
    }
    Task::none()
}

pub fn handle_toggle_info_hud(editor: &mut RawEditor) -> Task<Message> {
    // Every other key handler carries this guard; this one was the lone
    // exception, so typing an "i" into a modal's text field silently advanced
    // the overlay behind it.
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    editor.info_overlay = editor.info_overlay.next();
    Task::none()
}

pub fn handle_toggle_reduce_motion(editor: &mut RawEditor) -> Task<Message> {
    editor.reduce_motion = !editor.reduce_motion;
    crate::ui::motion::set_reduced(editor.reduce_motion);
    editor.save_preferences();
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

    Task::none()
}

pub fn handle_set_preview_preload_behind(editor: &mut RawEditor, val: f32) -> Task<Message> {
    editor.preview_preload_behind =
        (val.max(0.0) as usize).min(crate::app::state::PREVIEW_PRELOAD_BEHIND_MAX);
    Task::none()
}

pub fn handle_set_preview_preload_ahead(editor: &mut RawEditor, val: f32) -> Task<Message> {
    editor.preview_preload_ahead =
        (val.max(0.0) as usize).min(crate::app::state::PREVIEW_PRELOAD_AHEAD_MAX);
    Task::none()
}

pub fn handle_set_raw_preload_behind(editor: &mut RawEditor, val: f32) -> Task<Message> {
    editor.raw_preload_behind =
        (val.max(0.0) as usize).min(crate::app::state::RAW_PRELOAD_BEHIND_MAX);
    Task::none()
}

pub fn handle_set_raw_preload_ahead(editor: &mut RawEditor, val: f32) -> Task<Message> {
    editor.raw_preload_ahead =
        (val.max(0.0) as usize).min(crate::app::state::RAW_PRELOAD_AHEAD_MAX);
    Task::none()
}

pub fn handle_set_min_free_ram(editor: &mut RawEditor, val: f32) -> Task<Message> {
    editor.min_free_ram_mb =
        (val.max(0.0) as u32).min(crate::app::state::MIN_FREE_RAM_MB_MAX);
    Task::none()
}

/// Slider release in Preferences. The setters above only update state: they
/// fire on every drag tick, and each used to rewrite settings.json on the UI
/// thread while the slider moved.
pub fn handle_save_preferences(editor: &mut RawEditor) -> Task<Message> {
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
    editor.min_free_ram_mb = defaults.min_free_ram_mb;
    editor.reduce_motion = defaults.reduce_motion;
    crate::ui::motion::set_reduced(editor.reduce_motion);

    editor.evict_raw_cache_to_budget();
    editor.save_preferences();
    Task::none()
}

pub fn handle_histogram_toggled(editor: &mut RawEditor, enabled: bool) -> Task<Message> {
    editor.histogram_enabled = enabled;
    editor.save_preferences();
    Task::none()
}
