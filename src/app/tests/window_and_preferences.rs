//! `handlers::window`: dialogs, Escape and Enter, the title bar and the
//! Preferences settings. Opening links and folders is left out: those spawn
//! the desktop's browser or file manager.

use crate::app::message::Message;
use crate::app::state::{EditorReadiness, InfoOverlayState, Modal, PreferencesSection, SliderEntry};
use crate::app::testkit::*;
use crate::core::settings::AppSettings;
use crate::app::state::MaskTool;

fn entry() -> Option<SliderEntry> {
    Some(SliderEntry { key: "Exposure".into(), text: "1.5".into() })
}

/// Escape undoes one thing at a time, the most transient first, and never
/// clears an ordinary single selection.
#[test]
fn escape_steps_back_one_thing_at_a_time() {
    let mut e = editor_with_images(3);
    send(&mut e, Message::ImageSelected(1));
    e.session.multi_selection.extend([1, 2, 3]);
    e.session.selected_mask = Some(0);
    e.session.mask_tool = MaskTool::PlacingRadial;
    e.session.is_leveling = true;
    e.session.loupe.targeting = true;
    e.session.is_wb_picking = true;
    e.active_modal = Modal::Help;
    e.slider_entry = entry();

    send(&mut e, Message::Escape);
    assert!(e.slider_entry.is_none());
    assert_eq!(e.active_modal, Modal::Help, "one press, one step");
    send(&mut e, Message::Escape);
    assert_eq!(e.active_modal, Modal::None);
    assert!(e.session.is_wb_picking);
    send(&mut e, Message::Escape);
    assert!(!e.session.is_wb_picking);
    assert!(e.session.loupe.targeting);
    send(&mut e, Message::Escape);
    assert!(!e.session.loupe.targeting);
    assert!(e.session.is_leveling);
    send(&mut e, Message::Escape);
    assert!(!e.session.is_leveling);
    assert_eq!(e.session.mask_tool, MaskTool::PlacingRadial);
    send(&mut e, Message::Escape);
    assert_eq!(e.session.mask_tool, MaskTool::Inactive);
    assert_eq!(e.session.selected_mask, Some(0));
    send(&mut e, Message::Escape);
    assert_eq!(e.session.selected_mask, None);
    assert_eq!(e.session.multi_selection.len(), 3);
    send(&mut e, Message::Escape);
    assert_eq!(e.session.multi_selection.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(e.selection_anchor, Some(1));

    send(&mut e, Message::Escape);
    assert_eq!(e.session.selected_image_id, Some(1), "Escape cleared a single selection");
    assert_eq!(e.session.multi_selection.len(), 1);
}

/// Closing a dialog, by Escape or its Cancel, forgets what it was about to
/// act on.
#[test]
fn closing_a_dialog_forgets_its_target() {
    for close in [Message::CloseModal, Message::Escape] {
        let mut e = editor_with_images(2);
        e.session.pending_delete_ids = vec![1, 2];
        e.pending_remove_folder = Some("/nonexistent/testkit".into());
        e.drop_import.scanning.insert("/dropped".into());
        e.active_modal = Modal::Delete;
        send(&mut e, close.clone());
        assert_eq!(e.active_modal, Modal::None, "{close:?}");
        assert!(e.session.pending_delete_ids.is_empty(), "{close:?}");
        assert!(e.pending_remove_folder.is_none(), "{close:?}");
        assert!(e.drop_import.scanning.is_empty(), "{close:?}");
    }
}

/// A modal takes the keyboard, so the before/after peek's key release would
/// never arrive: opening one ends the peek.
#[test]
fn opening_a_dialog_ends_the_before_after_peek() {
    let mut e = develop_with_images(1);
    e.session.editor_readiness = EditorReadiness::Ready(1);
    send(&mut e, Message::SetBeforePeek(true));
    assert!(e.session.before_peek, "the peek did not start");
    send(&mut e, Message::OpenModal(Modal::Help));
    assert_eq!(e.active_modal, Modal::Help);
    assert!(!e.session.before_peek, "the peek outlived the dialog opening");
}

/// Enter closes the dialogs with nothing to confirm, and does nothing with
/// no dialog open.
#[test]
fn enter_closes_dialogs_that_confirm_nothing() {
    for modal in [Modal::Help, Modal::Preferences, Modal::About, Modal::Update] {
        let mut e = editor_with_images(1);
        e.active_modal = modal;
        send(&mut e, Message::ModalConfirm);
        assert_eq!(e.active_modal, Modal::None, "{modal:?}");
    }
    let mut e = editor_with_images(1);
    send(&mut e, Message::ImageSelected(1));
    send(&mut e, Message::ModalConfirm);
    assert_eq!(e.active_modal, Modal::None);
    assert_eq!(e.session.selected_image_id, Some(1));
}

/// Enter in the Export dialog is its Export button.
#[test]
fn enter_confirms_the_export_dialog() {
    let mut e = develop_with_images(2);
    send(&mut e, Message::OpenExportModal);
    send(&mut e, Message::ModalConfirm);
    assert_eq!(e.active_modal, Modal::None);
    assert!(e.session.is_exporting);
    assert_eq!(e.session.export_jobs.len(), 1);
}

/// The info panels cycle with `I`, but not while a dialog has the keyboard
/// (typing an "i" in a text field once advanced them behind it).
#[test]
fn the_info_key_cycles_the_panels_except_under_a_dialog() {
    let mut e = editor_with_images(1);
    send(&mut e, Message::ToggleInfoHud);
    assert_eq!(e.info_overlay, InfoOverlayState::Metadata);
    send(&mut e, Message::ToggleInfoHud);
    assert_eq!(e.info_overlay, InfoOverlayState::CacheDebug);
    send(&mut e, Message::ToggleInfoHud);
    assert_eq!(e.info_overlay, InfoOverlayState::Hidden);
    e.active_modal = Modal::Export;
    send(&mut e, Message::ToggleInfoHud);
    assert_eq!(e.info_overlay, InfoOverlayState::Hidden);
}

/// A second title-bar press within 400 ms maximises instead of dragging, and
/// doesn't chain into a third.
#[test]
fn a_title_bar_double_click_is_not_chained() {
    let mut e = editor_with_images(1);
    send(&mut e, Message::DragWindow);
    assert!(e.last_titlebar_click.is_some(), "the first press is remembered");
    send(&mut e, Message::DragWindow);
    assert!(e.last_titlebar_click.is_none(), "the double click is spent");
    send(&mut e, Message::DragWindow);
    assert!(e.last_titlebar_click.is_some(), "a third press starts over");
}

#[test]
fn the_scale_factor_ignores_nonsense_and_repeats() {
    let mut e = develop_with_images(1);
    let start = e.session.scale_factor;
    for bad in [0.0, -1.0, f32::NAN, f32::INFINITY, start] {
        send(&mut e, Message::ScaleFactorChanged(bad));
        assert_eq!(e.session.scale_factor, start, "{bad}");
    }
    send(&mut e, Message::ScaleFactorChanged(2.0));
    assert_eq!(e.session.scale_factor, 2.0);
}

#[test]
fn preferences_remembers_its_page() {
    let mut e = editor_with_images(1);
    send(&mut e, Message::SetPreferencesSection(PreferencesSection::Memory));
    assert_eq!(e.preferences_section, PreferencesSection::Memory);
    send(&mut e, Message::OpenModal(Modal::Preferences));
    send(&mut e, Message::CloseModal);
    send(&mut e, Message::OpenModal(Modal::Preferences));
    assert_eq!(e.preferences_section, PreferencesSection::Memory, "reopened on another page");
}

/// Every Preferences slider keeps to its range, whatever the slider sends.
#[test]
fn preference_sliders_stay_in_range() {
    use crate::app::state::{
        MIN_FREE_RAM_MB_MAX, PREVIEW_PRELOAD_AHEAD_MAX, PREVIEW_PRELOAD_BEHIND_MAX, RAW_PRELOAD_AHEAD_MAX,
        RAW_PRELOAD_BEHIND_MAX,
    };
    let mut e = editor_with_images(1);
    send(&mut e, Message::SetThumbnailSize(10.0));
    assert_eq!(e.thumbnail_size, 100.0);
    send(&mut e, Message::SetThumbnailSize(1e6));
    assert_eq!(e.thumbnail_size, 400.0);

    send(&mut e, Message::SetCacheCapacity(1.0));
    assert_eq!(e.session.cache_capacity, 20);
    assert_eq!(e.session.preview_cache.cap().get(), 20, "the LRU was not resized");
    send(&mut e, Message::SetCacheCapacity(1e6));
    assert_eq!(e.session.cache_capacity, 500);
    assert_eq!(e.session.preview_cache.cap().get(), 500);

    send(&mut e, Message::SetRawPreloadBudget(-5.0));
    assert_eq!(e.session.raw_preload_budget_mb, 0);
    send(&mut e, Message::SetRawPreloadBudget(1e9));
    assert_eq!(e.session.raw_preload_budget_mb, 4096);

    for v in [-3.0, 1e9] {
        send(&mut e, Message::SetPreviewPreloadBehind(v));
        send(&mut e, Message::SetPreviewPreloadAhead(v));
        send(&mut e, Message::SetRawPreloadBehind(v));
        send(&mut e, Message::SetRawPreloadAhead(v));
        send(&mut e, Message::SetMinFreeRam(v));
        let s = &e.session;
        let want = |max: usize| if v < 0.0 { 0 } else { max };
        assert_eq!(s.preview_preload_behind, want(PREVIEW_PRELOAD_BEHIND_MAX));
        assert_eq!(s.preview_preload_ahead, want(PREVIEW_PRELOAD_AHEAD_MAX));
        assert_eq!(s.raw_preload_behind, want(RAW_PRELOAD_BEHIND_MAX));
        assert_eq!(s.raw_preload_ahead, want(RAW_PRELOAD_AHEAD_MAX));
        assert_eq!(s.min_free_ram_mb, if v < 0.0 { 0 } else { MIN_FREE_RAM_MB_MAX });
    }
}

/// A RAW budget of zero drops the preloads, but not the decode Develop is
/// waiting on: without it, `Loading` would wait for nothing.
#[test]
fn a_zero_raw_budget_keeps_the_awaited_decode() {
    let mut e = develop_with_images(6);
    let mut world = World::default();
    // Image 1's RAW lands (cached) and its neighbours' preloads start; then
    // a neighbour whose preload is running is selected, and Develop waits on
    // that preload rather than decoding it twice.
    world.deliver(&mut e, Job::DirectDecode(1), true);
    let awaited = running_raw_preloads(&e)[0];
    send(&mut e, Message::ImageSelected(awaited));
    assert_eq!(e.session.editor_readiness, EditorReadiness::Loading(awaited));
    assert!(e.session.direct_raw_loads.is_empty(), "decoded twice: {}", dump(&e));
    assert!(!e.session.raw_cache.is_empty() && e.session.queued_raw_loads.len() > 1, "{}", dump(&e));

    send(&mut e, Message::SetRawPreloadBudget(0.0));
    assert!(e.session.raw_cache.is_empty());
    assert_eq!(e.session.raw_cache_bytes, 0);
    let left: Vec<i64> = e.session.queued_raw_loads.iter().map(|(id, _)| *id).collect();
    assert_eq!(left, vec![awaited], "{}", dump(&e));
    assert_invariants(&e);
}

/// Reset puts every Preferences setting back to the default.
#[test]
fn resetting_preferences_restores_the_defaults() {
    let mut e = editor_with_images(1);
    send(&mut e, Message::SetThumbnailSize(333.0));
    send(&mut e, Message::SetCacheCapacity(42.0));
    send(&mut e, Message::SetRawPreloadBudget(100.0));
    send(&mut e, Message::SetPreviewPreloadAhead(1.0));
    send(&mut e, Message::SetRawPreloadBehind(0.0));
    send(&mut e, Message::SetMinFreeRam(0.0));
    send(&mut e, Message::HistogramToggled(false));
    send(&mut e, Message::ToggleAutoAdvance);

    send(&mut e, Message::ResetPreferences);
    let d = AppSettings::default();
    assert_eq!(e.thumbnail_size, d.thumbnail_size);
    assert_eq!(e.session.cache_capacity, d.cache_capacity);
    assert_eq!(e.session.preview_cache.cap().get(), d.cache_capacity);
    assert_eq!(e.session.raw_preload_budget_mb, d.raw_preload_budget_mb);
    assert_eq!(e.session.preview_preload_ahead, d.preview_preload_ahead);
    assert_eq!(e.session.raw_preload_behind, d.raw_preload_behind);
    assert_eq!(e.session.min_free_ram_mb, d.min_free_ram_mb);
    assert_eq!(e.histogram_enabled, d.histogram_enabled);
    assert_eq!(e.session.auto_advance, d.auto_advance);
    assert_eq!(e.reduce_motion, d.reduce_motion);
}

