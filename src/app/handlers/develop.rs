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

pub fn handle_profile_curve_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.profile_curve = v;
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
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    editor.is_wb_picking = false; // tools are mutually exclusive
    editor.mask_tool = crate::app::state::MaskTool::Inactive;
    editor.selected_mask = None;
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
    _bounds: iced::Rectangle,
) -> Task<Message> {
    editor.drag_mode = DragMode::CropHandle(handle);
    editor.is_dragging = true;
    // See handle_mask_handle_grabbed: `_bounds` is the image rect, and
    // `viewport_size` must stay the canvas size. Overwriting it here made
    // apply_crop_drag re-apply zoom to already-zoomed dims (zoom² scaling),
    // so crop handles drifted from the cursor at any zoom != 1.
    Task::none()
}

pub fn handle_commit_edit(editor: &mut RawEditor) -> Task<Message> {
    // Every slider's on_release maps here, so this is the primary persistence
    // point now that update_pipeline only marks the state dirty.
    // commit_current_state flushes.
    editor.commit_current_state();
    Task::none()
}

pub fn handle_undo(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    if let Some((stack, index)) = editor.get_current_history() {
        if *index > 0 {
            *index -= 1;
            editor.current_edit_params = stack[*index];
            // Undo/redo don't commit to history (they move within it), so they
            // don't get commit_current_state's flush — persist explicitly.
            let task = update_pipeline(editor);
            editor.flush_edits();
            task
        } else {
            Task::none()
        }
    } else {
        Task::none()
    }
}

pub fn handle_redo(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    if let Some((stack, index)) = editor.get_current_history() {
        if *index < stack.len() - 1 {
            *index += 1;
            editor.current_edit_params = stack[*index];
            // See handle_undo.
            let task = update_pipeline(editor);
            editor.flush_edits();
            task
        } else {
            Task::none()
        }
    } else {
        Task::none()
    }
}

pub fn handle_reset_edits(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    editor.current_edit_params.reset();
    // The masks just got wiped; drop the selection/tool that pointed at them.
    editor.selected_mask = None;
    editor.mask_tool = crate::app::state::MaskTool::Inactive;
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
    bytes: std::sync::Arc<[u8]>,
    dims: (u32, u32),
    upload_ms: f32,
    render_ms: f32,
    update_ms: f32,
) -> Task<Message> {
    editor.rendered_preview_bytes = Some(bytes.clone());
    editor.rendered_preview_dims = dims;
    editor.bump_preview_generation();
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
    bytes: std::sync::Arc<[u8]>,
    dims: (u32, u32),
    data: crate::core::histogram::HistogramData,
    upload_ms: f32,
    render_ms: f32,
    update_ms: f32,
) -> Task<Message> {
    editor.rendered_preview_bytes = Some(bytes);
    editor.rendered_preview_dims = dims;
    editor.bump_preview_generation();
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

        // Oriented (display-space) dims: portrait images render portrait.
        let (disp_w, disp_h) = resources.oriented_dims();
        let original_aspect = disp_w as f32 / disp_h as f32;
        let (vw, _) = editor.viewport_size;
        let viewport_px = (vw * editor.scale_factor).round() as u32;
        let zoomed_px = (viewport_px as f32 * editor.zoom).round() as u32;
        let max_size = zoomed_px.clamp(800, disp_w.max(disp_h));

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

                // render_to_bytes already returns the shared buffer, so there
                // is nothing to copy or wrap here.
                Some((bytes, target_w, target_h, upload_ms, render_ms, update_ms))
            },
            |res| match res {
                Some((byte_arc, tw, th, upload_ms, render_ms, update_ms)) => {
                    Message::RenderPreview(byte_arc, (tw, th), upload_ms, render_ms, update_ms)
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

pub fn handle_toggle_wb_picker(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    editor.is_wb_picking = !editor.is_wb_picking;
    if editor.is_wb_picking {
        // Tools are mutually exclusive; leaving crop mode also clears its GPU flag.
        if editor.is_cropping {
            editor.is_cropping = false;
            editor.drag_mode = DragMode::None;
            editor.current_edit_params.is_cropping = 0;
        }
        editor.mask_tool = crate::app::state::MaskTool::Inactive;
        editor.selected_mask = None;
        editor.status = "WB picker: click a neutral grey/white area".to_string();
    } else {
        editor.status.clear();
    }
    update_pipeline(editor)
}

/// WB eyedropper: `(u, v)` is the display-space UV of the click within the
/// rendered image. Samples the raw Bayer mosaic around that point, derives the
/// neutral's WB multipliers, converts to Kelvin/tint, and sets the sliders.
pub fn handle_wb_picked(editor: &mut RawEditor, u: f32, v: f32) -> Task<Message> {
    // On a failed pick (clipped/dark area) the tool stays active so the user
    // can simply click elsewhere; it exits on success or on missing data.
    let Some(image_id) = editor.selected_image_id else {
        editor.is_wb_picking = false;
        return Task::none();
    };
    let Some(raw) = editor.raw_cache.get(&image_id).cloned() else {
        editor.is_wb_picking = false;
        editor.status = "WB picker: raw data not in memory (reopen the image)".to_string();
        return Task::none();
    };
    if raw.data.is_empty() || raw.width == 0 || raw.height == 0 {
        editor.is_wb_picking = false;
        editor.status = "WB picker: no raw pixels available".to_string();
        return Task::none();
    }

    // Display UV → sensor UV: undo the crop remap (the vertex shader shows the
    // crop sub-rect when not in crop mode), then the EXIF orientation mapping —
    // the same transform the fragment shader applies.
    let crop = editor.current_edit_params.crop;
    let cu = crop[0] + u.clamp(0.0, 1.0) * crop[2];
    let cv = crop[1] + v.clamp(0.0, 1.0) * crop[3];
    let (su, sv) = crate::color::display_to_sensor_uv(cu, cv, raw.orientation);

    // Sample a 16×16 sensor window centred on the pick, per CFA channel.
    let w = raw.width as i64;
    let h = raw.height as i64;
    let cx = ((su * w as f32) as i64).clamp(8, w - 9);
    let cy = ((sv * h as f32) as i64).clamp(8, h - 9);

    let mean_black =
        raw.black_levels.iter().sum::<u32>() as f32 / raw.black_levels.len() as f32;
    let clip_threshold = (raw.white_level as f32 * 0.98) as u16;

    let mut sums = [0.0f64; 3]; // R, G, B
    let mut counts = [0u32; 3];
    for y in (cy - 8)..(cy + 8) {
        for x in (cx - 8)..(cx + 8) {
            let value = raw.data[(y * w + x) as usize];
            if value >= clip_threshold {
                editor.status = "WB picker: area is clipped — pick a darker neutral".to_string();
                return Task::none();
            }
            // CFA channel for (x%2, y%2) per pattern: 0=RGGB 1=GRBG 2=GBRG 3=BGGR
            let (px, py) = ((x % 2) as u32, (y % 2) as u32);
            let channel = match (raw.cfa_pattern, py, px) {
                (0, 0, 0) | (1, 0, 1) | (2, 1, 0) | (3, 1, 1) => 0, // R
                (0, 1, 1) | (1, 1, 0) | (2, 0, 1) | (3, 0, 0) => 2, // B
                _ => 1,                                             // G (both sites)
            };
            sums[channel] += (value as f32 - mean_black).max(0.0) as f64;
            counts[channel] += 1;
        }
    }
    if counts.iter().any(|&c| c == 0) {
        return Task::none();
    }
    let r = (sums[0] / counts[0] as f64) as f32;
    let g = (sums[1] / counts[1] as f64) as f32;
    let b = (sums[2] / counts[2] as f64) as f32;
    if r < 1.0 || g < 1.0 || b < 1.0 {
        editor.status = "WB picker: area too dark for a reliable neutral".to_string();
        return Task::none();
    }

    // Neutral → multipliers → (Kelvin, tint) through the camera matrix.
    let wb_picked = [(g / r).clamp(0.05, 20.0), 1.0, (g / b).clamp(0.05, 20.0), 1.0];
    let matrices = match editor.current_dcp_profile.as_deref() {
        Some(p) => crate::color::CameraMatrices::Dcp(p),
        None => crate::color::CameraMatrices::Single(raw.color_matrix),
    };
    let (kelvin, tint) = crate::color::wb_to_kelvin_tint(wb_picked, &matrices);

    // Absolute Kelvin/tint → slider offsets around the as-shot anchor
    // (inverse of EditParams::kelvin_from_anchor / tint_from_anchor).
    let (anchor_kelvin, anchor_tint) = editor.as_shot_wb.unwrap_or((5000.0, 0.0));
    let anchor_mired = 1.0e6 / anchor_kelvin.clamp(1500.0, 50000.0);
    let temperature = ((anchor_mired - 1.0e6 / kelvin) / 120.0).clamp(-1.0, 1.0);
    let tint_offset = ((tint - anchor_tint) / 50.0).clamp(-1.0, 1.0);

    editor.is_wb_picking = false;
    editor.current_edit_params.temperature = temperature;
    editor.current_edit_params.tint = tint_offset;
    editor.status = format!("WB picked: {:.0}K, tint {:+.0}", kelvin, tint);

    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

/// Tolerances for reusing a memoized DCP interpolation. `kelvin` comes out of
/// `solve_wb`, which is deterministic, so identical slider state reproduces it
/// bit-for-bit; the epsilon only absorbs noise, it does not coarsen the slider.
const DCP_MEMO_KELVIN_EPS: f32 = 0.01;
const DCP_MEMO_CURVE_EPS: f32 = 0.0005;

/// True when a memo recorded at `(memo_kelvin, memo_curve)` is still valid for
/// `(kelvin, curve)`.
fn dcp_memo_is_valid(memo_kelvin: f32, memo_curve: f32, kelvin: f32, curve: f32) -> bool {
    (memo_kelvin - kelvin).abs() < DCP_MEMO_KELVIN_EPS
        && (memo_curve - curve).abs() < DCP_MEMO_CURVE_EPS
}

/// Resolve the DCP interpolation and WB override for the editor's current
/// slider state. Shared by every path that refreshes uniforms (slider edits,
/// image-ready, preview upgrades).
///
/// The interpolation depends only on `(kelvin, profile_curve)`, but this is
/// called from `update_pipeline` on every slider tick — including sliders that
/// cannot affect it. It is memoized in `editor.dcp_memo` because rebuilding the
/// HueSatMap LUT and re-baking the tone curve is a multi-allocation,
/// `powf`-heavy job that `update_uniforms` would then throw away via its own
/// dirty-check.
///
/// The two dirty-checks are independent and cannot disagree harmfully: both are
/// functions of the same slider state, so a miss on either side merely repeats
/// work that produces the same result.
pub fn resolve_wb_and_dcp(
    editor: &RawEditor,
) -> (Option<std::sync::Arc<crate::raw::dcp::InterpolatedProfile>>, Option<[f32; 4]>) {
    let as_shot = editor.as_shot_wb.unwrap_or((5000.0, 0.0));
    let fallback_matrix = editor
        .current_metadata
        .as_ref()
        .map(|m| m.color_matrix)
        .unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    let (kelvin, _tint, wb_override) = crate::color::solve_wb(
        &editor.current_edit_params,
        as_shot,
        editor.current_dcp_profile.as_deref(),
        fallback_matrix,
    );

    let Some(dcp) = editor.current_dcp_profile.as_ref() else {
        return (None, wb_override);
    };
    let curve = editor.current_edit_params.profile_curve;

    if let Ok(memo) = editor.dcp_memo.try_borrow() {
        if let Some((k, c, profile)) = memo.as_ref() {
            if dcp_memo_is_valid(*k, *c, kelvin, curve) {
                return (Some(profile.clone()), wb_override);
            }
        }
    }

    let interpolated = std::sync::Arc::new(crate::raw::dcp::interpolate_at_temperature(
        dcp, kelvin, curve,
    ));
    if let Ok(mut memo) = editor.dcp_memo.try_borrow_mut() {
        *memo = Some((kelvin, curve, interpolated.clone()));
    }

    (Some(interpolated), wb_override)
}

pub(crate) fn update_pipeline(editor: &mut RawEditor) -> Task<Message> {
    // Deferred, not written here: this runs once per mouse-move during a
    // slider or mask drag, and a synchronous SQLite write on that path cost
    // an fsync per event on the UI thread. `flush_edits` at the commit points
    // (CommitEdit, drag release, image/tab switch, close) does the write.
    editor.mark_edits_dirty();
    if let (Some(ctx), Some(resources)) = (&editor.gpu_context, &editor.image_resources) {
        // Phase 140: Interpolate DCP + resolve slider WB
        let (interpolated, wb_override) = resolve_wb_and_dcp(editor);
        resources.update_uniforms(ctx, &editor.current_edit_params, interpolated.as_deref(), wb_override);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dcp_memo_hits_on_identical_state() {
        assert!(dcp_memo_is_valid(5500.0, 0.33, 5500.0, 0.33));
    }

    #[test]
    fn dcp_memo_misses_when_temperature_moves() {
        // One Kelvin is far below what any slider step produces, and must
        // still invalidate — the memo must never coarsen the temperature.
        assert!(!dcp_memo_is_valid(5500.0, 0.33, 5501.0, 0.33));
    }

    #[test]
    fn dcp_memo_misses_when_profile_curve_moves() {
        assert!(!dcp_memo_is_valid(5500.0, 0.33, 5500.0, 0.34));
    }

    #[test]
    fn dcp_memo_absorbs_float_noise_only() {
        // Recomputing solve_wb from identical inputs is deterministic, so the
        // epsilon exists purely to tolerate last-bit drift, not real changes.
        assert!(dcp_memo_is_valid(5500.0, 0.33, 5500.0 + 1e-4, 0.33 + 1e-6));
    }
}
