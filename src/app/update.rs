use crate::app::handlers;
use crate::app::message::Message;
use crate::app::state::RawEditor;
use iced::Task;

pub fn update(editor: &mut RawEditor, message: Message) -> Task<Message> {
    match message {
        // Library
        Message::DatabaseLoaded(res) => handlers::library::handle_database_loaded(editor, res),
        Message::ImportFolder => handlers::library::handle_import_folder(editor),
        Message::ImportComplete(res) => handlers::library::handle_import_complete(editor, res),
        Message::CacheProcessed(res) => handlers::library::handle_cache_processed(editor, res),
        Message::ThumbnailGenerated(_) => Task::none(),
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
        Message::PreloadPreview(_) => Task::none(),

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
        Message::SaturationChanged(v) => handlers::develop::handle_saturation_changed(editor, v),
        Message::TemperatureChanged(v) => handlers::develop::handle_temperature_changed(editor, v),
        Message::TintChanged(v) => handlers::develop::handle_tint_changed(editor, v),
        Message::NoiseReductionChanged(v) => {
            handlers::develop::handle_noise_reduction_changed(editor, v)
        }
        Message::SharpeningChanged(v) => handlers::develop::handle_sharpening_changed(editor, v),
        Message::SharpenMaskingChanged(v) => {
            handlers::develop::handle_sharpen_masking_changed(editor, v)
        }
        Message::RotationChanged(v) => handlers::develop::handle_rotation_changed(editor, v),
        Message::SetCrop(crop) => handlers::develop::handle_set_crop(editor, crop),
        Message::ToggleCrop => handlers::develop::handle_toggle_crop(editor),
        Message::CropHandleGrabbed(h, b) => {
            handlers::develop::handle_crop_handle_grabbed(editor, h, b)
        }
        Message::CommitEdit => handlers::develop::handle_commit_edit(editor),
        Message::Undo => handlers::develop::handle_undo(editor),
        Message::Redo => handlers::develop::handle_redo(editor),
        Message::ResetEdits => handlers::develop::handle_reset_edits(editor),
        Message::CopySettings => handlers::develop::handle_copy_settings(editor),
        Message::PasteSettings => handlers::develop::handle_paste_settings(editor),
        Message::ToggleBeforeAfter => {
            editor.show_before = !editor.show_before;
            editor.histogram_cache.clear();
            Task::none()
        }
        Message::ModifiersChanged(m) => {
            editor.last_modifiers = m;
            Task::none()
        }
        Message::SetMinRating(r) => {
            editor.min_filter_rating = r;
            Task::none()
        }
        Message::DeleteImage => {
            tracing::warn!("Delete image requested (not implemented yet)");
            Task::none()
        }

        // Export
        Message::SetExportFormat(f) => handlers::export::handle_set_export_format(editor, f),
        Message::SetExportQuality(q) => handlers::export::handle_set_export_quality(editor, q),
        Message::ToggleExportResize(b) => handlers::export::handle_toggle_export_resize(editor, b),
        Message::SetExportWidth(w) => handlers::export::handle_set_export_width(editor, w),
        Message::SetExportSubfolder(s) => handlers::export::handle_set_export_subfolder(editor, s),
        Message::PickExportBasePath => handlers::export::handle_pick_export_base_path(editor),
        Message::SetExportBasePath(p) => handlers::export::handle_set_export_base_path(editor, p),
        Message::OpenExportModal => handlers::export::handle_open_export_modal(editor),
        Message::ExportConfirmed => handlers::export::handle_export_confirmed(editor),
        Message::ProcessNextExport => handlers::export::handle_process_next_export(editor),
        Message::ExportRawLoaded(id, res) => {
            handlers::export::handle_export_raw_loaded(editor, id, res)
        }
        Message::ExportPipelineReady(id, res) => {
            handlers::export::handle_export_pipeline_ready(editor, id, res)
        }
        Message::ExportSaveComplete(id, res) => {
            handlers::export::handle_export_save_complete(editor, id, res)
        }

        // Window
        Message::MinimizeWindow => handlers::window::handle_minimize_window(),
        Message::MaximizeWindow => handlers::window::handle_maximize_window(),
        Message::CloseWindow => handlers::window::handle_close_window(),
        Message::DragWindow => handlers::window::handle_drag_window(),
        Message::OpenModal(m) => handlers::window::handle_open_modal(editor, m),
        Message::CloseModal => handlers::window::handle_close_modal(editor),
        Message::ModalNoOp => Task::none(),
        Message::Escape => handlers::window::handle_escape(editor),
        Message::ToggleInfoHud => handlers::window::handle_toggle_info_hud(editor),
        Message::SetThumbnailSize(s) => handlers::window::handle_set_thumbnail_size(editor, s),
        Message::SetCacheCapacity(c) => handlers::window::handle_set_cache_capacity(editor, c),
        Message::HistogramToggled(b) => handlers::window::handle_histogram_toggled(editor, b),
        Message::ToggleProfiler => {
            editor.show_profiler = !editor.show_profiler;
            Task::none()
        }
        Message::RenderFinished(h, bytes, dims, d, u, r, cpu) => {
            handlers::develop::handle_render_finished(editor, h, bytes, dims, d, u, r, cpu)
        }

        // Loading
        Message::RawDataLoaded(res) => handlers::loading::handle_raw_data_loaded(editor, res),
        Message::ImageResourcesReady(id, res) => {
            handlers::loading::handle_image_resources_ready(editor, id, res)
        }
        Message::PreviewGenerated(_) => Task::none(),
    }
}
