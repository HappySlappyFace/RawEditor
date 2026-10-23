use crate::app::handlers;
use crate::app::message::Message;
use crate::app::state::{Modal, ModalExit, RawEditor};
use iced::Task;

pub fn update(editor: &mut RawEditor, message: Message) -> Task<Message> {
    // Ctrl/Cmd+click resets a slider to its default. iced fires `on_change`
    // for that click but no `on_release`, so the CommitEdit every other slider
    // gesture ends with never arrives — commit it here instead. Holding the
    // modifier can't mean anything else: there is no Ctrl+drag on a slider.
    let slider_reset = editor.last_modifiers.command() && is_slider_change(&message);
    // iced's text field has no blur event, so a typed slider value is
    // abandoned as soon as the user does something else instead.
    if editor.slider_entry.is_some() && ends_slider_entry(&message) {
        editor.slider_entry = None;
    }
    let modal_before = editor.active_modal;
    let task = dispatch(editor, message);
    let task = if slider_reset {
        editor.commit_current_state();
        Task::batch([task, handlers::develop::finish_stage_drag(editor)])
    } else {
        task
    };
    match track_modal_exit(editor, modal_before) {
        Some(exit) => Task::batch([task, exit]),
        None => task,
    }
}

/// Keep a just-closed modal mounted while it plays its exit.
///
/// Done here, after every message, rather than in the close handlers: a
/// dozen paths close modals (Escape, the backdrop, Cancel, every confirm),
/// and a new one would otherwise silently skip the animation.
fn track_modal_exit(editor: &mut RawEditor, before: Modal) -> Option<Task<Message>> {
    if editor.active_modal != Modal::None {
        // Opened (or re-opened) — any exit in progress is superseded.
        editor.modal_exit = None;
        return None;
    }
    if before == Modal::None || crate::ui::motion::reduced() {
        return None;
    }
    editor.modal_exit_seq = editor.modal_exit_seq.wrapping_add(1);
    let token = editor.modal_exit_seq;
    editor.modal_exit = Some(ModalExit { modal: before, token });
    let wait = crate::ui::motion::wall_duration(crate::ui::motion::duration::MODAL_EXIT);
    // `async move`, not `sleep(wait)` directly: tokio builds the timer when
    // `sleep` is *called*, which needs a runtime context; inside the block it
    // is built when the executor first polls it.
    Some(Task::perform(
        async move { tokio::time::sleep(wait).await },
        move |_| Message::ModalExitFinished(token),
    ))
}

/// The user moved on from a slider's typed value: another slider, the image,
/// a tool, or another photo or tab.
fn ends_slider_entry(message: &Message) -> bool {
    is_slider_change(message)
        || matches!(
            message,
            Message::CommitEdit
                | Message::MousePressed
                | Message::ImageSelected(_)
                | Message::TabChanged(_)
                | Message::ResetEdits
                | Message::Undo
                | Message::Redo
                | Message::TurnImage { .. }
                | Message::ToggleCrop
                | Message::ToggleWbPicker
                | Message::ToggleLevelTool
                | Message::SelectMask(_)
                | Message::ToggleMaskPlacement(_)
        )
}

/// Messages emitted by the Develop sidebar's sliders.
fn is_slider_change(message: &Message) -> bool {
    matches!(
        message,
        Message::ExposureChanged(_)
            | Message::ContrastChanged(_)
            | Message::HighlightsChanged(_)
            | Message::ShadowsChanged(_)
            | Message::WhitesChanged(_)
            | Message::BlacksChanged(_)
            | Message::VibranceChanged(_)
            | Message::ProfileCurveChanged(_)
            | Message::GtParamChanged(..)
            | Message::SaturationChanged(_)
            | Message::TemperatureChanged(_)
            | Message::TintChanged(_)
            | Message::TextureChanged(_)
            | Message::ClarityChanged(_)
            | Message::DehazeChanged(_)
            | Message::LumaNoiseChanged(_)
            | Message::LumaDetailChanged(_)
            | Message::ColorNoiseChanged(_)
            | Message::SharpeningChanged(_)
            | Message::SharpenMaskingChanged(_)
            | Message::RotationChanged(_)
            | Message::MaskAdjustmentChanged(..)
            | Message::LensParamChanged(..)
            | Message::DefringeChanged(..)
    )
}

fn dispatch(editor: &mut RawEditor, message: Message) -> Task<Message> {
    match message {
        // Library
        Message::DatabaseLoaded(res) => handlers::library::handle_database_loaded(editor, res),
        Message::ImportFolder => handlers::library::handle_import_folder(editor),
        Message::ImportFromCamera => handlers::library::handle_import_from_camera(editor),
        Message::ImportComplete(res) => handlers::library::handle_import_complete(editor, res),
        Message::CacheProcessed(res) => handlers::library::handle_cache_processed(editor, res),
        Message::SetRating(r) => handlers::library::handle_set_rating(editor, r),
        Message::SetFlag(f) => handlers::library::handle_set_flag(editor, f),
        Message::ToggleAutoAdvance => handlers::library::handle_toggle_auto_advance(editor),

        // Navigation
        Message::ImageSelected(id) => handlers::navigation::handle_image_selected(editor, id),
        Message::TabChanged(tab) => handlers::navigation::handle_tab_changed(editor, tab),
        Message::SelectNextImage => handlers::navigation::handle_select_next_image(editor),
        Message::SelectPreviousImage => handlers::navigation::handle_select_previous_image(editor),
        Message::Zoom(d, p) => handlers::navigation::handle_zoom(editor, d, p),
        Message::Pan(d) => handlers::navigation::handle_pan(editor, d),
        Message::ResetView => handlers::navigation::handle_reset_view(editor),
        Message::MousePressed => handlers::navigation::handle_mouse_pressed(editor),
        Message::MouseReleased => handlers::navigation::handle_mouse_released(editor),
        Message::MouseMoved(p) => handlers::navigation::handle_mouse_moved(editor, p),
        Message::WorkingPreviewReady(id, h, p, d) => {
            handlers::navigation::handle_working_preview_ready(editor, id, h, p, d)
        }
        Message::PreviewCached(id, res) => {
            handlers::navigation::handle_preview_cached(editor, id, res)
        }

        // Develop
        Message::ExposureChanged(v) => handlers::develop::handle_exposure_changed(editor, v),
        Message::ContrastChanged(v) => handlers::develop::handle_contrast_changed(editor, v),
        Message::HighlightsChanged(v) => handlers::develop::handle_highlights_changed(editor, v),
        Message::ShadowsChanged(v) => handlers::develop::handle_shadows_changed(editor, v),
        Message::WhitesChanged(v) => handlers::develop::handle_whites_changed(editor, v),
        Message::BlacksChanged(v) => handlers::develop::handle_blacks_changed(editor, v),
        Message::BlackOffsetChanged(i, v) => {
            handlers::develop::handle_black_offset_changed(editor, i, v)
        }
        Message::BlackPhaseChanged(y, v) => {
            handlers::develop::handle_black_phase_changed(editor, y, v)
        }
        Message::VibranceChanged(v) => handlers::develop::handle_vibrance_changed(editor, v),
        Message::ProfileCurveChanged(v) => {
            handlers::develop::handle_profile_curve_changed(editor, v)
        }
        Message::CurveChanged(ch, curve, commit) => {
            handlers::develop::handle_curve_changed(editor, ch, curve, commit)
        }
        Message::SetCurveChannel(ch) => {
            editor.curve_channel = ch;
            Task::none()
        }
        Message::ResetCurve => {
            let ch = editor.curve_channel;
            handlers::develop::handle_curve_changed(editor, ch, Default::default(), true)
        }
        Message::LensParamChanged(which, v) => {
            handlers::lens::handle_lens_param_changed(editor, which, v)
        }
        Message::LensToggled(which, on) => handlers::lens::handle_lens_toggled(editor, which, on),
        Message::DefringeChanged(which, v) => {
            handlers::lens::handle_defringe_changed(editor, which, v)
        }
        Message::ToggleSection(section) => {
            if !editor.collapsed_sections.remove(&section) {
                editor.collapsed_sections.insert(section);
            }
            // Opening the loupe renders it (it does nothing while folded).
            if section == crate::app::message::SidebarSection::DetailPreview {
                handlers::loupe::request(editor)
            } else {
                Task::none()
            }
        }
        Message::ToneMapperChanged(m) => {
            handlers::develop::handle_tone_mapper_changed(editor, m)
        }
        Message::GtParamChanged(p, v) => handlers::develop::handle_gt_param_changed(editor, p, v),
        Message::ToggleWbPicker => handlers::develop::handle_toggle_wb_picker(editor),
        Message::WbPicked(u, v) => handlers::develop::handle_wb_picked(editor, u, v),
        Message::SaturationChanged(v) => handlers::develop::handle_saturation_changed(editor, v),
        Message::TemperatureChanged(v) => handlers::develop::handle_temperature_changed(editor, v),
        Message::TintChanged(v) => handlers::develop::handle_tint_changed(editor, v),
        Message::TextureChanged(v) => handlers::develop::handle_texture_changed(editor, v),
        Message::ClarityChanged(v) => handlers::develop::handle_clarity_changed(editor, v),
        Message::DehazeChanged(v) => handlers::develop::handle_dehaze_changed(editor, v),
        Message::LumaNoiseChanged(v) => {
            handlers::develop::handle_luma_noise_changed(editor, v)
        }
        Message::LumaDetailChanged(v) => {
            handlers::develop::handle_luma_detail_changed(editor, v)
        }
        Message::ColorNoiseChanged(v) => {
            handlers::develop::handle_color_noise_changed(editor, v)
        }
        Message::SharpeningChanged(v) => handlers::develop::handle_sharpening_changed(editor, v),
        Message::SharpenMaskingChanged(v) => {
            handlers::develop::handle_sharpen_masking_changed(editor, v)
        }
        Message::RotationChanged(v) => handlers::develop::handle_rotation_changed(editor, v),
        Message::TurnImage { clockwise } => handlers::develop::handle_turn_image(editor, clockwise),
        Message::ToggleLevelTool => handlers::develop::handle_toggle_level_tool(editor),
        Message::LevelLineDrawn(dx, dy) => handlers::develop::handle_level_line(editor, dx, dy),
        Message::SetCrop(crop) => handlers::develop::handle_set_crop(editor, crop),
        Message::ToggleCrop => handlers::develop::handle_toggle_crop(editor),
        Message::CropHandleGrabbed(h, b) => {
            handlers::develop::handle_crop_handle_grabbed(editor, h, b)
        }
        Message::ToggleMaskPlacement(t) => {
            handlers::masks::handle_toggle_mask_placement(editor, t)
        }
        Message::SelectMask(i) => handlers::masks::handle_select_mask(editor, i),
        Message::DeleteMask(i) => handlers::masks::handle_delete_mask(editor, i),
        Message::ToggleMaskEnabled(i, e) => {
            handlers::masks::handle_toggle_mask_enabled(editor, i, e)
        }
        Message::ToggleMaskInvert(inv) => handlers::masks::handle_toggle_mask_invert(editor, inv),
        Message::MaskAdjustmentChanged(which, v) => {
            handlers::masks::handle_mask_adjustment_changed(editor, which, v)
        }
        Message::MaskPlacementStarted(u, v) => {
            handlers::masks::handle_mask_placement_started(editor, u, v)
        }
        Message::MaskHandleGrabbed(h, b) => {
            handlers::masks::handle_mask_handle_grabbed(editor, h, b)
        }
        Message::ToggleBrushArmed => handlers::masks::handle_toggle_brush_armed(editor),
        Message::SetBrushErase(erase) => {
            editor.brush_settings.erase = erase;
            Task::none()
        }
        Message::BrushSettingChanged(which, v) => {
            let b = &mut editor.brush_settings;
            match which {
                crate::app::message::BrushSetting::Size => b.size = v.clamp(0.002, 0.5),
                crate::app::message::BrushSetting::Feather => b.feather = v.clamp(0.0, 1.0),
                crate::app::message::BrushSetting::Flow => b.flow = v.clamp(0.01, 1.0),
            }
            editor.canvas_cache.clear();
            Task::none()
        }
        Message::BrushStroke(ev) => handlers::masks::handle_brush_stroke(editor, ev),
        Message::ClearMaskBrush => handlers::masks::handle_clear_mask_brush(editor),
        Message::ToggleMaskOverlay(on) => {
            editor.show_mask_overlay = on;
            crate::app::handlers::develop::update_pipeline(editor)
        }
        Message::ToggleMaskLumaRange(on) => handlers::masks::handle_toggle_luma_range(editor, on),
        Message::ToggleMaskColorRange(on) => handlers::masks::handle_toggle_color_range(editor, on),
        Message::PickMaskColor => {
            editor.color_picking_mask = editor.selected_mask;
            editor.status = "Color range: click the colour to select".to_string();
            Task::none()
        }
        Message::MaskColorPicked(u, v) => handlers::masks::handle_mask_color_picked(editor, u, v),
        Message::CommitEdit => handlers::develop::handle_commit_edit(editor),
        Message::SliderEntryStart { key, text } => {
            handlers::develop::handle_slider_entry_start(editor, key, text)
        }
        Message::SliderEntryChanged(text) => {
            if let Some(entry) = &mut editor.slider_entry {
                entry.text = text;
            }
            Task::none()
        }
        Message::SliderEntryCancel => {
            editor.slider_entry = None;
            Task::none()
        }
        Message::SliderSet { change, commit } => {
            editor.slider_entry = None;
            let task = dispatch(editor, *change);
            if commit {
                // What the release of a drag would have done.
                Task::batch([task, handlers::develop::handle_commit_edit(editor)])
            } else {
                task
            }
        }
        Message::Undo => handlers::develop::handle_undo(editor),
        Message::Redo => handlers::develop::handle_redo(editor),
        Message::ResetEdits => handlers::develop::handle_reset_edits(editor),
        Message::CopySettings => handlers::develop::handle_copy_settings(editor),
        Message::CopyAllSettings => handlers::develop::handle_copy_all_settings(editor),
        Message::CopySettingsConfirmed => {
            handlers::develop::handle_copy_settings_confirmed(editor)
        }
        Message::ToggleCopyCategory(c, v) => {
            handlers::develop::handle_toggle_copy_category(editor, c, v)
        }
        Message::SetAllCopyCategories(v) => {
            handlers::develop::handle_set_all_copy_categories(editor, v)
        }
        Message::PasteSettings => handlers::develop::handle_paste_settings(editor),
        Message::SetBeforePeek(active) => {
            handlers::develop::handle_set_before_peek(editor, active)
        }
        Message::ModifiersChanged(m) => {
            editor.last_modifiers = m;
            Task::none()
        }
        Message::SetMinRating(r) => {
            editor.min_filter_rating = r;
            Task::none()
        }
        Message::SetLibraryFolderFilter(folder_filter) => {
            handlers::library::handle_set_library_folder_filter(editor, folder_filter)
        }
        Message::DeleteImage => handlers::delete::handle_delete_image_requested(editor),
        Message::MarkForRemovalConfirmed => handlers::delete::handle_mark_for_removal_confirmed(editor),
        Message::DeleteFromDiskConfirmed => handlers::delete::handle_delete_from_disk_confirmed(editor),
        Message::HardDeleteComplete(outcome) => handlers::delete::handle_hard_delete_complete(editor, outcome),

        // Export
        Message::SetExportFormat(f) => handlers::export::handle_set_export_format(editor, f),
        Message::SetExportQuality(q) => handlers::export::handle_set_export_quality(editor, q),
        Message::ToggleExportResize(b) => handlers::export::handle_toggle_export_resize(editor, b),
        Message::SetExportWidth(w) => handlers::export::handle_set_export_width(editor, w),
        Message::SetExportSubfolder(s) => handlers::export::handle_set_export_subfolder(editor, s),
        Message::PickExportBasePath => handlers::export::handle_pick_export_base_path(editor),
        Message::SetExportBasePath(p) => handlers::export::handle_set_export_base_path(editor, p),
        Message::OpenExportModal => handlers::export::handle_open_export_modal(editor),
        Message::CancelExport => handlers::export::handle_cancel_export(editor),
        Message::ExportConfirmed => handlers::export::handle_export_confirmed(editor),
        Message::ProcessNextExport => handlers::export::handle_process_next_export(editor),
        Message::ExportRawLoaded(id, params, res) => {
            handlers::export::handle_export_raw_loaded(editor, id, *params, res)
        }
        Message::ExportPipelineReady(id, params, res) => {
            handlers::export::handle_export_pipeline_ready(editor, id, *params, res)
        }
        Message::ExportSaveComplete(id, res) => {
            handlers::export::handle_export_save_complete(editor, id, res)
        }

        // Window
        Message::MinimizeWindow => handlers::window::handle_minimize_window(),
        Message::MaximizeWindow => handlers::window::handle_maximize_window(),
        Message::CloseWindow => handlers::window::handle_close_window(editor),
        Message::DragWindow => handlers::window::handle_drag_window(editor),
        Message::OpenModal(m) => handlers::window::handle_open_modal(editor, m),
        Message::CloseModal => handlers::window::handle_close_modal(editor),
        Message::ModalExitFinished(token) => {
            if editor.modal_exit.is_some_and(|e| e.token == token) {
                editor.modal_exit = None;
            }
            Task::none()
        }
        Message::ToggleReduceMotion => handlers::window::handle_toggle_reduce_motion(editor),
        Message::ModalNoOp => Task::none(),
        Message::MenuItem(inner) => {
            editor.menu_generation = editor.menu_generation.wrapping_add(1);
            dispatch(editor, *inner)
        }
        Message::Escape => handlers::window::handle_escape(editor),
        Message::ModalConfirm => handlers::window::handle_modal_confirm(editor),
        Message::SelectAll => handlers::navigation::handle_select_all(editor),
        Message::RemoveFolderRequested(f) => {
            handlers::library::handle_remove_folder_requested(editor, f)
        }
        Message::RemoveFolderConfirmed => {
            handlers::library::handle_remove_folder_confirmed(editor)
        }
        Message::ImportFolderPicked(r) => handlers::library::handle_import_folder_picked(editor, r),
        Message::CheckForUpdates => handlers::window::handle_check_for_updates(editor),
        Message::OpenRepository => handlers::window::handle_open_repository(editor),
        Message::LoupeReady(id, r) => handlers::loupe::handle_ready(editor, id, r),
        Message::LoupeRendered(id, r) => handlers::loupe::handle_rendered(editor, id, r),
        Message::LoupePan(dx, dy) => handlers::loupe::handle_pan(editor, dx, dy),
        Message::ToggleLoupeTarget => handlers::loupe::handle_toggle_target(editor),
        Message::LoupeTargetPicked(u, v) => handlers::loupe::handle_target_picked(editor, u, v),
        Message::NoiseSettled(token) => handlers::loupe::handle_noise_settled(editor, token),
        Message::LensfunCheck => handlers::lensfun::handle_check(editor),
        Message::LensfunChecked(r) => handlers::lensfun::handle_checked(editor, r),
        Message::LensfunDownload => handlers::lensfun::handle_download(editor),
        Message::LensfunInstalled(r) => handlers::lensfun::handle_installed(editor, r),
        Message::LensfunRescan => handlers::lensfun::handle_rescan(editor),
        Message::LensDbLoaded(changed) => handlers::lensfun::handle_db_loaded(editor, changed),
        Message::CameraProfileSelected(c) => handlers::camera_profiles::handle_selected(editor, c),
        Message::SetCameraProfileDefault(camera, value) => {
            handlers::camera_profiles::handle_set_default(editor, camera, value)
        }
        Message::DownloadCameraProfile => handlers::camera_profiles::handle_download(editor),
        Message::CameraProfileDownloaded(r) => handlers::camera_profiles::handle_downloaded(editor, r),
        Message::ImportCameraProfiles => handlers::camera_profiles::handle_import(editor),
        Message::CameraProfilesPicked(files) => handlers::camera_profiles::handle_picked(editor, files),
        Message::CameraProfilesImported(ok, failed) => {
            handlers::camera_profiles::handle_imported(editor, ok, failed)
        }
        Message::UpdateCameraProfiles => handlers::camera_profiles::handle_update(editor),
        Message::CameraProfilesUpdated(r) => handlers::camera_profiles::handle_updated(editor, r),
        Message::RescanCameraProfiles => handlers::camera_profiles::handle_rescan(editor),
        Message::CameraProfilesScanned(changed) => {
            handlers::camera_profiles::handle_scanned(editor, changed)
        }
        Message::OpenLink(url) => handlers::window::handle_open_link(editor, url),
        Message::SetPreferencesSection(section) => {
            handlers::window::handle_set_preferences_section(editor, section)
        }
        Message::OpenAppFolder(folder) => handlers::window::handle_open_app_folder(editor, folder),
        Message::ToggleInfoHud => handlers::window::handle_toggle_info_hud(editor),
        Message::SetThumbnailSize(s) => handlers::window::handle_set_thumbnail_size(editor, s),
        Message::SetCacheCapacity(c) => handlers::window::handle_set_cache_capacity(editor, c),
        Message::SetRawPreloadBudget(v) => handlers::window::handle_set_raw_preload_budget(editor, v),
        Message::SetPreviewPreloadBehind(v) => {
            handlers::window::handle_set_preview_preload_behind(editor, v)
        }
        Message::SetPreviewPreloadAhead(v) => {
            handlers::window::handle_set_preview_preload_ahead(editor, v)
        }
        Message::SetRawPreloadBehind(v) => {
            handlers::window::handle_set_raw_preload_behind(editor, v)
        }
        Message::SetRawPreloadAhead(v) => {
            handlers::window::handle_set_raw_preload_ahead(editor, v)
        }
        Message::SetMinFreeRam(v) => handlers::window::handle_set_min_free_ram(editor, v),
        Message::SavePreferences => handlers::window::handle_save_preferences(editor),
        Message::ResetPreferences => handlers::window::handle_reset_preferences(editor),
        Message::HistogramToggled(b) => handlers::window::handle_histogram_toggled(editor, b),
        Message::ToggleProfiler => {
            editor.show_profiler = !editor.show_profiler;
            Task::none()
        }
        Message::RenderFinished(output) => handlers::develop::handle_render_finished(editor, *output),
        Message::EditedProxySaved(id, generation, result) => {
            handlers::proxy::handle_saved(editor, id, generation, result);
            Task::none()
        }
        Message::EditedProxyLoaded(id, result) => handlers::proxy::handle_loaded(editor, id, result),
        Message::ViewportResized(w, h) => {
            let (old_w, old_h) = editor.viewport_size;
            editor.viewport_size = (w, h);

            // Re-render on a size change: a different visible window, and at
            // zoom the buffer only covers the window the old size implied.
            let resized = (w - old_w).abs() > 0.5 || (h - old_h).abs() > 0.5;
            if resized {
                handlers::navigation::throttled_render(editor)
            } else {
                Task::none()
            }
        }
        Message::ScaleFactorChanged(s) => handlers::window::handle_scale_factor_changed(editor, s),

        // Loading
        Message::RawDataLoaded(id, res) => handlers::loading::handle_raw_data_loaded(editor, id, res),
        Message::RawPreloaded(id, res) => handlers::loading::handle_raw_preloaded(editor, id, res),
        Message::ImageResourcesReady(id, res) => {
            handlers::loading::handle_image_resources_ready(editor, id, res)
        }
        Message::FullResReady(id, res) => handlers::loading::handle_full_res_ready(editor, id, res),

        // Scroll boost + filmstrip momentum
        Message::WheelScrolled(delta) => handlers::scroll::handle_wheel(editor, delta),
        Message::GridHovered(over) => {
            editor.cursor_over_grid = over;
            Task::none()
        }
        Message::GridScrolled(y, width, height) => {
            editor.grid_scroll_y = y;
            editor.grid_viewport = (width, height);
            Task::none()
        }
        Message::FilmstripScrolled(x, width) => {
            editor.filmstrip_scroll_x = x;
            editor.filmstrip_view_w = width;
            Task::none()
        }
        Message::WindowResized(size) => {
            editor.window_size = size;
            handlers::window::query_scale_factor()
        }
        Message::KineticTick(now) => handlers::scroll::handle_kinetic_tick(editor, now),
        Message::NavigationKeyReleased => handlers::scroll::handle_navigation_released(editor),
        Message::FilmstripWheel(delta) => handlers::scroll::handle_filmstrip_wheel(editor, delta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> RawEditor {
        RawEditor::new().0
    }

    /// Every message is as big as the largest variant, and debug builds give
    /// each temporary its own stack slot: with an `EditParams` (~20 KB)
    /// inline, the Develop view overflowed the 2 MB test-thread stack. Big
    /// payloads go in a `Box`.
    #[test]
    fn message_stays_small() {
        let size = std::mem::size_of::<Message>();
        assert!(size <= 512, "Message is {size} bytes — box the large variant");
    }

    fn history_len(e: &mut RawEditor) -> usize {
        e.get_current_history().map(|(stack, _)| stack.len()).unwrap_or(0)
    }

    fn type_into(e: &mut RawEditor, key: &str) {
        let _ = update(e, Message::SliderEntryStart { key: key.into(), text: "0.00".into() });
        assert!(e.slider_entry.is_some());
    }

    /// Enter on a typed value sets the slider and records ONE undo step, the
    /// way a drag and its release would.
    #[test]
    fn a_typed_slider_value_is_applied_as_one_undo_step() {
        let mut e = editor();
        e.selected_image_id = Some(1);
        let base = history_len(&mut e);
        type_into(&mut e, "ExposureChanged(0.0)");
        let _ = update(&mut e, Message::SliderSet { change: Box::new(Message::ExposureChanged(1.5)), commit: true });
        assert_eq!(e.current_edit_params.exposure, 1.5);
        assert!(e.slider_entry.is_none(), "the field closes");
        assert_eq!(history_len(&mut e), base + 1);
        let _ = update(&mut e, Message::Undo);
        assert_eq!(e.current_edit_params.exposure, 0.0);
    }

    /// A tool setting (brush size) is set, but is not an edit to undo.
    #[test]
    fn a_typed_tool_setting_adds_no_undo_step() {
        let mut e = editor();
        e.selected_image_id = Some(1);
        let base = history_len(&mut e);
        let size = crate::app::message::BrushSetting::Size;
        let _ = update(&mut e, Message::SliderSet { change: Box::new(Message::BrushSettingChanged(size, 0.1)), commit: false });
        assert_eq!(e.brush_settings.size, 0.1);
        assert_eq!(history_len(&mut e), base);
    }

    /// Escape gives the number back — whether the field still has focus
    /// (it swallows the key, and the subscription sends SliderEntryCancel)
    /// or not (a plain Escape) — and does nothing else on the way.
    #[test]
    fn escape_leaves_a_typed_value_without_applying_it() {
        let mut e = editor();
        type_into(&mut e, "ContrastChanged(0.0)");
        let _ = update(&mut e, Message::SliderEntryCancel);
        assert!(e.slider_entry.is_none());

        type_into(&mut e, "ContrastChanged(0.0)");
        e.is_wb_picking = true;
        let _ = update(&mut e, Message::Escape);
        assert!(e.slider_entry.is_none());
        assert!(e.is_wb_picking, "the first Escape only closes the field");
        assert_eq!(e.current_edit_params.contrast, 0.0);
    }

    /// iced's text field has no blur event: moving on to anything else
    /// abandons the typed value.
    #[test]
    fn moving_on_abandons_a_typed_value() {
        let mut e = editor();
        type_into(&mut e, "ContrastChanged(0.0)");
        let _ = update(&mut e, Message::ExposureChanged(0.3));
        assert!(e.slider_entry.is_none());
        type_into(&mut e, "ContrastChanged(0.0)");
        let _ = update(&mut e, Message::SliderEntryChanged("1.2".into()));
        assert_eq!(e.slider_entry.as_ref().map(|s| s.text.as_str()), Some("1.2"), "typing keeps it");
    }

    /// The level tool is exclusive with the other click tools, and Escape
    /// disarms it.
    #[test]
    fn the_level_tool_takes_turns_with_the_other_tools() {
        let mut e = editor();
        e.current_tab = crate::app::message::AppTab::Develop;
        let _ = update(&mut e, Message::ToggleWbPicker);
        let _ = update(&mut e, Message::ToggleLevelTool);
        assert!(e.is_leveling && !e.is_wb_picking);
        let _ = update(&mut e, Message::ToggleCrop);
        assert!(e.is_cropping && !e.is_leveling);
        let _ = update(&mut e, Message::ToggleLevelTool);
        assert!(e.is_leveling && !e.is_cropping);
        let _ = update(&mut e, Message::Escape);
        assert!(!e.is_leveling);
    }

    /// A drawn line straightens and disarms the tool; a click that is not a
    /// line keeps it armed and changes nothing.
    #[test]
    fn a_level_line_sets_straighten_once() {
        let mut e = editor();
        e.selected_image_id = Some(1);
        e.current_tab = crate::app::message::AppTab::Develop;
        let _ = update(&mut e, Message::ToggleLevelTool);
        let _ = update(&mut e, Message::LevelLineDrawn(2.0, 1.0));
        assert!(e.is_leveling);
        assert_eq!(e.current_edit_params.rotation, 0.0);
        let _ = update(&mut e, Message::LevelLineDrawn(400.0, 20.0));
        assert!(!e.is_leveling);
        let want = (20.0f32 / 400.0).atan().to_degrees();
        assert!((e.current_edit_params.rotation - want).abs() < 1e-4);
    }

    #[test]
    fn closing_a_modal_keeps_it_mounted_for_its_exit() {
        let mut e = editor();
        let _ = update(&mut e, Message::OpenModal(Modal::Help));
        assert_eq!(e.modal_exit, None);
        let _ = update(&mut e, Message::Escape);
        assert_eq!(e.active_modal, Modal::None);
        let exit = e.modal_exit.expect("exit should be playing");
        assert_eq!(exit.modal, Modal::Help);
        let _ = update(&mut e, Message::ModalExitFinished(exit.token));
        assert_eq!(e.modal_exit, None);
    }

    /// Close → reopen → close inside one exit: the first close's timer must
    /// not cut the second exit short.
    #[test]
    fn a_stale_exit_timer_cannot_end_a_later_exit() {
        let mut e = editor();
        let _ = update(&mut e, Message::OpenModal(Modal::Help));
        let _ = update(&mut e, Message::CloseModal);
        let first = e.modal_exit.unwrap().token;
        let _ = update(&mut e, Message::OpenModal(Modal::Help));
        assert_eq!(e.modal_exit, None, "re-opening supersedes the exit");
        let _ = update(&mut e, Message::CloseModal);
        let _ = update(&mut e, Message::ModalExitFinished(first));
        assert!(e.modal_exit.is_some(), "stale timer ended the new exit");
    }

    #[test]
    fn choosing_a_menu_entry_closes_the_menu_and_runs_the_entry() {
        let mut e = editor();
        let generation = e.menu_generation;
        let _ = update(&mut e, Message::MenuItem(Box::new(Message::OpenModal(Modal::About))));
        assert_eq!(e.active_modal, Modal::About);
        assert_ne!(e.menu_generation, generation);
    }
}
