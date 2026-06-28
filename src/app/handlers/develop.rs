use crate::app::message::Message;
use crate::app::state::{DragMode, RawEditor};
use crate::ui::preview_renderer::CropHandle;
use iced::Task;

pub fn handle_exposure_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.exposure = v;
    update_pipeline(editor)
}

pub fn handle_contrast_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.contrast = v;
    update_pipeline(editor)
}

pub fn handle_highlights_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.highlights = v;
    update_pipeline(editor)
}

pub fn handle_shadows_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.shadows = v;
    update_pipeline(editor)
}

pub fn handle_whites_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.whites = v;
    update_pipeline(editor)
}

pub fn handle_blacks_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.blacks = v;
    update_pipeline(editor)
}

pub fn handle_black_offset_changed(editor: &mut RawEditor, i: usize, v: f32) -> Task<Message> {
    if i < 4 {
        editor.current_edit_params.black_offsets[i] = v;
        update_pipeline(editor)
    } else {
        Task::none()
    }
}

pub fn handle_black_phase_changed(editor: &mut RawEditor, y: bool, v: u32) -> Task<Message> {
    if y {
        editor.current_edit_params.black_phase_y = v;
    } else {
        editor.current_edit_params.black_phase_x = v;
    }
    update_pipeline(editor)
}

pub fn handle_vibrance_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.vibrance = v;
    update_pipeline(editor)
}

pub fn handle_saturation_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.saturation = v;
    update_pipeline(editor)
}

pub fn handle_temperature_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.temperature = v;
    update_pipeline(editor)
}

pub fn handle_tint_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.tint = v;
    update_pipeline(editor)
}

pub fn handle_luma_noise_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.luma_noise = v;
    update_pipeline(editor)
}

pub fn handle_color_noise_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.color_noise = v;
    update_pipeline(editor)
}

pub fn handle_sharpening_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.sharpening = v;
    update_pipeline(editor)
}

pub fn handle_sharpen_masking_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.sharpen_masking = v;
    update_pipeline(editor)
}

pub fn handle_rotation_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.rotation = v;
    update_pipeline(editor)
}

pub fn handle_set_crop(editor: &mut RawEditor, crop: [f32; 4]) -> Task<Message> {
    editor.current_edit_params.crop = crop;
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

pub fn handle_toggle_crop(editor: &mut RawEditor) -> Task<Message> {
    editor.is_cropping = !editor.is_cropping;
    if editor.is_cropping {
        editor.drag_mode = DragMode::Crop;
    } else {
        editor.drag_mode = DragMode::None;
    }
    // Phase 135: Sync to edit params for GPU visualization
    editor.current_edit_params.is_cropping = if editor.is_cropping { 1 } else { 0 };
    update_pipeline(editor)
}

pub fn handle_crop_handle_grabbed(
    editor: &mut RawEditor,
    handle: CropHandle,
    bounds: iced::Rectangle,
) -> Task<Message> {
    editor.drag_mode = DragMode::CropHandle(handle);
    editor.is_dragging = true;
    editor.viewport_size = (bounds.width, bounds.height);
    Task::none()
}

pub fn handle_commit_edit(editor: &mut RawEditor) -> Task<Message> {
    editor.commit_current_state();
    Task::none()
}

pub fn handle_undo(editor: &mut RawEditor) -> Task<Message> {
    if let Some((stack, index)) = editor.get_current_history() {
        if *index > 0 {
            *index -= 1;
            editor.current_edit_params = stack[*index];
            update_pipeline(editor)
        } else {
            Task::none()
        }
    } else {
        Task::none()
    }
}

pub fn handle_redo(editor: &mut RawEditor) -> Task<Message> {
    if let Some((stack, index)) = editor.get_current_history() {
        if *index < stack.len() - 1 {
            *index += 1;
            editor.current_edit_params = stack[*index];
            update_pipeline(editor)
        } else {
            Task::none()
        }
    } else {
        Task::none()
    }
}

pub fn handle_reset_edits(editor: &mut RawEditor) -> Task<Message> {
    editor.current_edit_params.reset();
    if let Some(lib) = &editor.library {
        if let Some(id) = editor.selected_image_id {
            let _ = lib.delete_edits(id);
        }
    }
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

pub fn handle_copy_settings(editor: &mut RawEditor) -> Task<Message> {
    editor.edit_clipboard = Some(editor.current_edit_params);
    editor.status = "Settings copied!".to_string();
    Task::none()
}

pub fn handle_paste_settings(editor: &mut RawEditor) -> Task<Message> {
    if let Some(cb) = editor.edit_clipboard {
        editor.current_edit_params = cb;
        if let Some(lib) = &editor.library {
            for &id in &editor.multi_selection {
                if Some(id) != editor.selected_image_id {
                    let _ = lib.save_edit_params(id, &editor.current_edit_params);
                }
            }
            editor.status = "Settings pasted to selection".to_string();
        }
        let task = update_pipeline(editor);
        editor.commit_current_state();
        task
    } else {
        Task::none()
    }
}

// Fires immediately after GPU render — stores the new preview and releases the
// render throttle.  Histogram computation is spawned as a separate task so it
// never blocks the next slider event from starting its own render.
#[allow(clippy::too_many_arguments)]
pub fn handle_render_preview(
    editor: &mut RawEditor,
    handle: iced::widget::image::Handle,
    bytes: std::sync::Arc<[u8]>,
    dims: (u32, u32),
    upload_ms: f32,
    render_ms: f32,
    update_ms: f32,
) -> Task<Message> {
    editor.rendered_preview = Some(handle);
    editor.rendered_preview_bytes = Some(bytes.clone());
    editor.rendered_preview_dims = dims;
    editor.canvas_cache.clear();

    editor.profiler.push_frame(crate::core::profiler::ProfilerFrame {
        update_ms,
        upload_ms,
        render_ms,
        total_ms: update_ms + upload_ms + render_ms,
    });
    editor.profiler_cache.clear();

    // Release the render lock — next slider/zoom event can start immediately.
    editor.is_rendering = false;
    let render_task = if editor.pending_render {
        editor.is_rendering = true;
        editor.pending_render = false;
        trigger_async_render(editor)
    } else {
        Task::none()
    };

    // Histogram runs concurrently; result arrives shortly after via HistogramReady.
    let hist_task = Task::perform(
        async move {
            tokio::task::spawn_blocking(move || crate::core::histogram::calculate(&bytes))
                .await
                .ok()
        },
        |res| match res {
            Some(data) => Message::HistogramReady(data),
            None => Message::ModalNoOp,
        },
    );

    Task::batch(vec![render_task, hist_task])
}

pub fn handle_histogram_ready(
    editor: &mut RawEditor,
    data: crate::core::histogram::HistogramData,
) -> Task<Message> {
    *editor.histogram_data.borrow_mut() = data;
    editor.histogram_cache.clear();
    Task::none()
}

// Kept for backward-compat (used by RenderFinished variant which may come from
// older code paths or exports).
#[allow(clippy::too_many_arguments)]
pub fn handle_render_finished(
    editor: &mut RawEditor,
    handle: iced::widget::image::Handle,
    bytes: std::sync::Arc<[u8]>,
    dims: (u32, u32),
    data: crate::core::histogram::HistogramData,
    upload_ms: f32,
    render_ms: f32,
    update_ms: f32,
) -> Task<Message> {
    editor.rendered_preview = Some(handle);
    editor.rendered_preview_bytes = Some(bytes);
    editor.rendered_preview_dims = dims;
    *editor.histogram_data.borrow_mut() = data;
    editor.histogram_cache.clear();
    editor.canvas_cache.clear();

    editor.profiler.push_frame(crate::core::profiler::ProfilerFrame {
        update_ms,
        upload_ms,
        render_ms,
        total_ms: update_ms + upload_ms + render_ms,
    });
    editor.profiler_cache.clear();

    editor.is_rendering = false;
    if editor.pending_render {
        editor.is_rendering = true;
        editor.pending_render = false;
        return trigger_async_render(editor);
    }
    Task::none()
}

pub fn trigger_async_render(editor: &mut RawEditor) -> Task<Message> {
    let t_update_start = std::time::Instant::now();

    if let (Some(ctx), Some(resources)) = (&editor.gpu_context, &editor.image_resources) {
        let ctx = ctx.clone();
        let resources = resources.clone();

        let original_aspect = resources.width as f32 / resources.height as f32;
        let (vw, _) = editor.viewport_size;
        let viewport_px = (vw * editor.scale_factor).round() as u32;
        let zoomed_px = (viewport_px as f32 * editor.zoom).round() as u32;
        let max_size = zoomed_px.clamp(800, resources.width);

        let (target_w, target_h) = if original_aspect > 1.0 {
            let w = max_size;
            let h = (w as f32 / original_aspect).round() as u32;
            (w, h)
        } else {
            let h = max_size;
            let w = (h as f32 * original_aspect).round() as u32;
            (w, h)
        };
        let update_ms = t_update_start.elapsed().as_secs_f32() * 1000.0;

        return Task::perform(
            async move {
                let (bytes, upload_ms, render_ms) = crate::gpu::render_functions::render_to_bytes(
                    &ctx, &resources, target_w, target_h,
                )
                .await;

                // Only the two fast memory operations — no histogram here.
                // Arc::from copies once; Handle::from_rgba moves (no copy).
                let byte_arc: std::sync::Arc<[u8]> = std::sync::Arc::from(bytes.as_slice());
                let handle = iced::widget::image::Handle::from_rgba(target_w, target_h, bytes);
                Some((handle, byte_arc, target_w, target_h, upload_ms, render_ms, update_ms))
            },
            |res| match res {
                Some((handle, byte_arc, tw, th, upload_ms, render_ms, update_ms)) => {
                    Message::RenderPreview(handle, byte_arc, (tw, th), upload_ms, render_ms, update_ms)
                }
                None => Message::RenderFailed,
            },
        );
    }
    // GPU context not ready — release the lock so the next slider event can retry.
    editor.is_rendering = false;
    editor.pending_render = false;
    Task::none()
}

fn update_pipeline(editor: &mut RawEditor) -> Task<Message> {
    editor.save_current_edits();
    if let (Some(ctx), Some(resources)) = (&editor.gpu_context, &editor.image_resources) {
        // Phase 140: Interpolate DCP if available
        let interpolated = editor.current_dcp_profile.as_ref().map(|dcp| {
            crate::raw::dcp::interpolate_at_temperature(dcp, editor.current_edit_params.temperature_to_kelvin())
        });
        
        resources.update_uniforms(ctx, &editor.current_edit_params, interpolated.as_ref());
        editor.canvas_cache.clear();
    }
    
    // Phase 106: Throttling
    if editor.is_rendering {
        editor.pending_render = true;
        Task::none()
    } else {
        editor.is_rendering = true;
        editor.pending_render = false;
        trigger_async_render(editor)
    }
}
