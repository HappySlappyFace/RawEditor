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

pub fn handle_noise_reduction_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.noise_reduction = v;
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
    Task::none()
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
            editor.current_edit_params = stack[*index].clone();
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
            editor.current_edit_params = stack[*index].clone();
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

// Helper
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

    // Profiler
    editor
        .profiler
        .push_frame(crate::core::profiler::ProfilerFrame {
            update_ms,
            upload_ms,
            render_ms,
            total_ms: update_ms + upload_ms + render_ms,
        });
    // Phase 105: Invalidate profiler canvas so it redraws with fresh data
    editor.profiler_cache.clear();

    // Phase 106: Throttling
    editor.is_rendering = false;
    if editor.pending_render {
        editor.is_rendering = true;
        editor.pending_render = false;
        return trigger_async_render(editor);
    }

    Task::none()
}

pub fn trigger_async_render(editor: &mut RawEditor) -> Task<Message> {
    // Phase 105: Start the CPU timer — measures all synchronous work before we hand off to GPU
    let t_update_start = std::time::Instant::now();

    if let (Some(ctx), Some(resources)) = (&editor.gpu_context, &editor.image_resources) {
        let ctx = ctx.clone();
        let resources = resources.clone();

        // Calculate target preview size (max 1280px)
        let original_aspect = resources.width as f32 / resources.height as f32;
        const MAX_PREVIEW_SIZE: u32 = 1280;
        let (target_w, target_h) = if original_aspect > 1.0 {
            let w = MAX_PREVIEW_SIZE;
            (w, (w as f32 / original_aspect) as u32)
        } else {
            let h = MAX_PREVIEW_SIZE;
            ((h as f32 * original_aspect) as u32, h)
        };

        // Phase 105: CPU work is done — snapshot the elapsed time before the async boundary
        let update_ms = t_update_start.elapsed().as_secs_f32() * 1000.0;

        return Task::perform(
            async move {
                // 1. Render to bytes (async, on GPU)
                let (bytes, upload_ms, render_ms) = crate::gpu::render_functions::render_to_bytes(
                    &ctx, &resources, target_w, target_h,
                )
                .await;

                // 2. Calculate histogram & Create Handle (CPU, blocking but in async task)
                tokio::task::spawn_blocking(move || {
                    let histogram = crate::core::histogram::calculate(&bytes);
                    // Phase 115: Store byte arc BEFORE moving bytes into the Handle
                    let byte_arc: std::sync::Arc<[u8]> = std::sync::Arc::from(bytes.as_slice());
                    let handle = iced::widget::image::Handle::from_rgba(target_w, target_h, bytes);
                    (handle, byte_arc, histogram, upload_ms, render_ms, update_ms, target_w, target_h)
                })
                .await
                .ok()
            },
            |res| {
                if let Some((handle, byte_arc, histogram, upload_ms, render_ms, update_ms, tw, th)) = res {
                    Message::RenderFinished(handle, byte_arc, (tw, th), histogram, upload_ms, render_ms, update_ms)
                } else {
                    Message::ModalNoOp
                }
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
        resources.update_uniforms(ctx, &editor.current_edit_params);
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
