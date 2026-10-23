use crate::app::message::Message;
use crate::app::state::{DragMode, RawEditor};
use crate::core::tonemap::ToneSettings;
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

pub fn handle_curve_changed(
    editor: &mut RawEditor,
    channel: crate::core::curves::CurveChannel,
    curve: crate::core::curves::Curve,
    commit: bool,
) -> Task<Message> {
    *editor.current_edit_params.curves.get_mut(channel) = curve;
    let task = update_pipeline(editor);
    if commit {
        editor.commit_current_state();
    }
    task
}

/// Switch the tone rendering operator.
///
/// Commits immediately rather than waiting for a `CommitEdit`: this is a
/// discrete choice from a button, not a drag, so there is no release event to
/// hang persistence off.
pub fn handle_tone_mapper_changed(
    editor: &mut RawEditor,
    mapper: crate::core::tonemap::ToneMapper,
) -> Task<Message> {
    if editor.current_edit_params.tone_mapper == mapper {
        return Task::none();
    }
    editor.current_edit_params.tone_mapper = mapper;
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

/// Adjust one GT curve shape parameter. Clamped to the same ranges
/// `GtParams::sanitised` enforces, so the stored value and the baked curve
/// never disagree about what the slider meant.
pub fn handle_gt_param_changed(
    editor: &mut RawEditor,
    param: crate::app::message::GtParam,
    v: f32,
) -> Task<Message> {
    use crate::app::message::GtParam;
    let gt = &mut editor.current_edit_params.gt;
    match param {
        GtParam::Contrast => gt.contrast = v,
        GtParam::LinearStart => gt.linear_start = v,
        GtParam::LinearLength => gt.linear_length = v,
        GtParam::BlackTightness => gt.black_tightness = v,
    }
    *gt = gt.sanitised();
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

pub fn handle_texture_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.texture = v;
    update_pipeline(editor)
}

pub fn handle_clarity_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.clarity = v;
    update_pipeline(editor)
}

pub fn handle_dehaze_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.dehaze = v;
    update_pipeline(editor)
}

// Noise reduction is held back on the main view while these move, and shown
// live in the detail loupe — see `handlers::loupe`.
pub fn handle_luma_noise_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    crate::app::handlers::loupe::handle_noise_changed(editor, |p| p.luma_noise = v)
}

pub fn handle_luma_detail_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    crate::app::handlers::loupe::handle_noise_changed(editor, |p| p.luma_detail = v)
}

pub fn handle_color_noise_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    crate::app::handlers::loupe::handle_noise_changed(editor, |p| p.color_noise = v)
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

/// ⟲ / ⟳: a quarter turn of the whole frame. The crop, masks and brush
/// strokes turn with it (`EditParams::turn_quarter`) and so does the detail
/// loupe's target, which is a point in the same (cropped) view; the view
/// returns to fit, since a pan offset is a fraction of the old shape.
pub fn handle_turn_image(editor: &mut RawEditor, clockwise: bool) -> Task<Message> {
    // The turn needs the open image's own shape; resources left over from
    // another image (Library with a different selection) would scale its
    // brush strokes by the wrong aspect.
    let Some(res) = editor
        .image_resources
        .as_ref()
        .filter(|r| Some(r.image_id) == editor.selected_image_id)
    else {
        return Task::none();
    };
    let (w, h) = res.oriented_dims(editor.current_edit_params.quarter_turns);
    editor.current_edit_params.turn_quarter(clockwise, w as f32 / h.max(1) as f32);
    // The crop turned rigidly with the frame, so a point in crop-local UV
    // turns the same way inside it.
    let [u, v] = editor.loupe.center;
    editor.loupe.center = if clockwise { [1.0 - v, u] } else { [v, 1.0 - u] };
    editor.zoom = 1.0;
    editor.pan_offset = cgmath::Vector2::new(0.0, 0.0);
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

/// A click on a slider's value: a text field takes its place, focused with
/// the value selected, so typing replaces it.
pub fn handle_slider_entry_start(editor: &mut RawEditor, key: String, text: String) -> Task<Message> {
    editor.slider_entry = Some(crate::app::state::SliderEntry { key, text });
    let id = crate::app::views::develop::slider_entry_id();
    Task::batch([
        iced::widget::text_input::focus(id.clone()),
        iced::widget::text_input::select_all(id),
    ])
}

/// Arm/disarm the level tool. Develop-only and exclusive with every other
/// tool, like the WB picker: a drag on the image can only mean one thing.
pub fn handle_toggle_level_tool(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None
        || editor.current_tab != crate::app::message::AppTab::Develop
    {
        return Task::none();
    }
    editor.is_leveling = !editor.is_leveling;
    if editor.is_leveling {
        if editor.is_cropping {
            editor.is_cropping = false;
            editor.drag_mode = DragMode::None;
        }
        editor.is_wb_picking = false;
        editor.mask_tool = crate::app::state::MaskTool::Inactive;
        editor.selected_mask = None;
        editor.brush_armed = false;
        editor.color_picking_mask = None;
        editor.loupe.targeting = false;
        editor.status = "Level: drag along a line that should be level or upright".to_string();
    } else {
        editor.status.clear();
    }
    update_pipeline(editor)
}

/// The level line was drawn: straighten so it shows level (or upright). A
/// line too short to aim with keeps the tool armed for another try.
pub fn handle_level_line(editor: &mut RawEditor, dx: f32, dy: f32) -> Task<Message> {
    let current = editor.current_edit_params.rotation;
    let Some(angle) = crate::core::viewport::straighten_from_line(current, dx, dy) else {
        editor.status = "Level: drag a longer line along the horizon or an upright".to_string();
        return Task::none();
    };
    editor.is_leveling = false;
    editor.status = format!("Straightened by {:+.1}°", angle - current);
    editor.current_edit_params.rotation = angle;
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

pub fn handle_set_crop(editor: &mut RawEditor, crop: [f32; 4]) -> Task<Message> {
    editor.current_edit_params.crop = crop;
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

pub fn handle_toggle_crop(editor: &mut RawEditor) -> Task<Message> {
    // Develop-only. Without the tab check, pressing C in Library flipped
    // is_cropping / drag_mode and ran a full update_pipeline with nothing on
    // screen to show for it — the user only met the consequences on their next
    // visit to Develop.
    if editor.active_modal != crate::app::state::Modal::None
        || editor.current_tab != crate::app::message::AppTab::Develop
    {
        return Task::none();
    }
    editor.is_wb_picking = false; // tools are mutually exclusive
    editor.is_leveling = false;
    editor.mask_tool = crate::app::state::MaskTool::Inactive;
    editor.selected_mask = None;
    editor.brush_armed = false;
    editor.color_picking_mask = None;
    editor.loupe.targeting = false;
    editor.is_cropping = !editor.is_cropping;
    if editor.is_cropping {
        editor.drag_mode = DragMode::Crop;
    } else {
        editor.drag_mode = DragMode::None;
    }
    // update_pipeline mirrors is_cropping into the params for the shader.
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
    finish_stage_drag(editor)
}

/// A noise-reduction drag just ended: apply what the main view held back
/// (`handlers::loupe`); or, if a mid-drag pause already did, from the
/// subsampled preview (`RawEditor::stage_drag`), render it at full resolution.
pub fn finish_stage_drag(editor: &mut RawEditor) -> Task<Message> {
    if editor.held_noise.is_some() {
        // `stage_drag` is already clear, so this render is full resolution.
        crate::app::handlers::loupe::release_noise(editor)
    } else if editor.display_from_drag_preview {
        crate::app::handlers::navigation::throttled_render(editor)
    } else {
        Task::none()
    }
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

/// Ctrl+C — open the category picker so the user chooses what to copy.
pub fn handle_copy_settings(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None
        || editor.selected_image_id.is_none()
    {
        return Task::none();
    }
    // Masks can't be copied if there are none — drop the tick rather than let
    // the user confirm a category that would silently do nothing.
    if editor.current_edit_params.mask_count == 0 {
        editor.copy_categories.masks = false;
    }
    editor.active_modal = crate::app::state::Modal::CopySettings;
    Task::none()
}

/// Ctrl+Shift+C — copy everything with no modal, so the common case keeps
/// zero friction.
pub fn handle_copy_all_settings(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None
        || editor.selected_image_id.is_none()
    {
        return Task::none();
    }
    store_clipboard(editor, crate::core::types::CopyCategories::all());
    Task::none()
}

/// Confirm the picker (its Copy button, or Enter via `ModalConfirm`).
pub fn handle_copy_settings_confirmed(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::CopySettings {
        return Task::none();
    }
    editor.active_modal = crate::app::state::Modal::None;
    if editor.copy_categories.is_empty() {
        return Task::none();
    }
    store_clipboard(editor, editor.copy_categories);
    Task::none()
}

/// Snapshot the current params plus the chosen categories.
///
/// Both halves are stored — see `EditClipboard`. Filtering the params here
/// would make "not copied" indistinguishable from "copied as the default
/// value" at paste time.
fn store_clipboard(editor: &mut RawEditor, categories: crate::core::types::CopyCategories) {
    editor.edit_clipboard = Some(crate::core::types::EditClipboard {
        params: editor.current_edit_params,
        categories,
    });
    editor.status = format!("Copied {}", describe_categories(&categories));
}

/// Human-readable summary of a selection, for the status line.
fn describe_categories(c: &crate::core::types::CopyCategories) -> String {
    let all = crate::core::types::CopyCategories::all();
    if *c == all {
        return "all settings".to_string();
    }
    let mut parts = Vec::new();
    if c.tone { parts.push("tone"); }
    if c.color { parts.push("color"); }
    if c.white_balance { parts.push("WB"); }
    if c.noise { parts.push("noise"); }
    if c.detail { parts.push("detail"); }
    if c.geometry { parts.push("crop"); }
    if c.profile { parts.push("profile"); }
    if c.masks { parts.push("masks"); }
    if c.lens { parts.push("lens"); }
    if c.black_levels { parts.push("black levels"); }
    if parts.is_empty() {
        "nothing".to_string()
    } else {
        parts.join(", ")
    }
}

pub fn handle_toggle_copy_category(
    editor: &mut RawEditor,
    category: crate::app::message::CopyCategory,
    value: bool,
) -> Task<Message> {
    use crate::app::message::CopyCategory;
    let c = &mut editor.copy_categories;
    match category {
        CopyCategory::Tone => c.tone = value,
        CopyCategory::Color => c.color = value,
        CopyCategory::WhiteBalance => c.white_balance = value,
        CopyCategory::Presence => c.presence = value,
        CopyCategory::Noise => c.noise = value,
        CopyCategory::Detail => c.detail = value,
        CopyCategory::Geometry => c.geometry = value,
        CopyCategory::Profile => c.profile = value,
        CopyCategory::Masks => c.masks = value,
        CopyCategory::Lens => c.lens = value,
        CopyCategory::BlackLevels => c.black_levels = value,
    }
    editor.save_preferences();
    Task::none()
}

pub fn handle_set_all_copy_categories(editor: &mut RawEditor, all: bool) -> Task<Message> {
    editor.copy_categories = if all {
        crate::core::types::CopyCategories::all()
    } else {
        crate::core::types::CopyCategories::none()
    };
    // Never leave Masks ticked on an image that has none.
    if editor.current_edit_params.mask_count == 0 {
        editor.copy_categories.masks = false;
    }
    editor.save_preferences();
    Task::none()
}

/// Ctrl+V — MERGE the clipboard's selected categories onto the target(s).
///
/// A merge, not a replace: only the categories the user ticked at copy time
/// are written, and everything else on the target survives untouched.
pub fn handle_paste_settings(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    let Some(clip) = editor.edit_clipboard else {
        return Task::none();
    };
    if clip.categories.is_empty() {
        return Task::none();
    }

    crate::core::types::apply_categories(
        &mut editor.current_edit_params,
        &clip.params,
        clip.categories,
    );

    // A paste that lowered mask_count can leave `selected_mask` pointing past
    // the end of the live masks, exactly as handle_reset_edits guards against.
    let live_masks = editor.current_edit_params.mask_count as usize;
    if editor.selected_mask.is_some_and(|i| i >= live_masks) {
        editor.selected_mask = None;
        editor.mask_tool = crate::app::state::MaskTool::Inactive;
    }

    // Other images in the selection: load each one's own params and merge into
    // them. Writing `current_edit_params` wholesale (as this used to) would
    // overwrite the categories the user chose NOT to copy.
    // Written in one transaction: a paste onto a few hundred images used to
    // commit once per image on the UI thread.
    let mut others = 0usize;
    if let Some(lib) = &editor.library {
        let merged: Vec<(i64, crate::core::types::EditParams)> = editor
            .multi_selection
            .iter()
            .filter(|&&id| Some(id) != editor.selected_image_id)
            .map(|&id| {
                let mut target = lib.load_edit_params(id).unwrap_or_default();
                crate::core::types::apply_categories(&mut target, &clip.params, clip.categories);
                (id, target)
            })
            .collect();
        match lib.save_edit_params_many(&merged) {
            Ok(n) => others = n,
            Err(e) => tracing::error!("Failed to paste onto {} image(s): {e}", merged.len()),
        }
        // Their edits changed unrendered: their edited thumbnails are stale.
        let pasted: Vec<i64> = merged.iter().map(|(id, _)| *id).collect();
        editor.invalidate_proxies(&pasted);
    }

    editor.status = if others > 0 {
        format!(
            "Pasted {} to {} images",
            describe_categories(&clip.categories),
            others + 1
        )
    } else {
        format!("Pasted {}", describe_categories(&clip.categories))
    };

    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}


// The single landing point for a completed develop render: stores the new
// preview, applies the histogram, keeps the overview as the image's edited
// proxy, and releases the render throttle.
//
// This used to be split into RenderPreview (release the throttle) then
// HistogramReady (the slow part), because the histogram scanned the entire
// display buffer on the CPU. The dedicated small overview pass made that scan
// trivial, so the split — and the extra message round-trip — no longer earn
// their keep.
pub fn handle_render_finished(
    editor: &mut RawEditor,
    out: crate::app::message::RenderOutput,
) -> Task<Message> {
    // A render of the image the user has since navigated away from: drop the
    // pixels, but still release the throttle below so the next render runs.
    let current = editor.image_resources.as_ref().map(|r| r.image_id);
    if current == Some(out.image_id) && !out.display.is_empty() {
        editor.rendered_preview_bytes = Some(out.display);
        editor.rendered_preview_dims = out.dims;
        // Buffer and window swap in the same frame — see the note in
        // trigger_async_render about why this is not published earlier.
        editor.rendered_view_rect = out.view_rect;
        editor.bump_preview_generation();
        *editor.histogram_data.borrow_mut() = out.histogram;
        editor.histogram_cache.clear();
        editor.canvas_cache.clear();
    }
    // The overview is a faithful render of `out.image_id` even when the user
    // has already moved on — which is exactly when `persist` may be waiting
    // for it (see `proxy::resolve_awaiting`).
    let awaited = editor.proxy_awaiting.contains_key(&out.image_id);
    if let Some(params) = out.edit.filter(|_| current == Some(out.image_id) || awaited) {
        crate::app::handlers::proxy::capture(
            editor,
            out.image_id,
            out.overview,
            out.overview_dims,
            params,
        );
    }
    let save = crate::app::handlers::proxy::resolve_awaiting(editor, out.image_id, out.edit);

    editor.profiler.push_frame(crate::core::profiler::ProfilerFrame {
        update_ms: out.update_ms,
        upload_ms: out.upload_ms,
        render_ms: out.render_ms,
        total_ms: out.update_ms + out.upload_ms + out.render_ms,
    });
    editor.profiler_cache.clear();

    // A new image, a full-resolution swap or a DPI change re-render the main
    // view without pushing params; the loupe catches up here.
    let loupe = crate::app::handlers::loupe::refresh_if_stale(editor);

    editor.is_rendering = false;
    if editor.pending_render {
        editor.is_rendering = true;
        editor.pending_render = false;
        return Task::batch([save, loupe, trigger_async_render(editor)]);
    }
    Task::batch([save, loupe])
}

/// What a develop render of the current view needs: window, target size,
/// and how far the screen magnifies the texture's source pixels. The source
/// is the develop texture itself (`image_display_dims`), so a subsampled
/// preview reports the magnification the full-resolution rebuild would fix.
pub fn render_target(editor: &RawEditor) -> crate::core::viewport::RenderTarget {
    let (disp_w, disp_h) = editor.image_display_dims();
    let (vw, vh) = editor.viewport_size;
    crate::core::viewport::render_target(
        disp_w,
        disp_h,
        vw,
        vh,
        editor.zoom,
        editor.pan_offset,
        editor.scale_factor,
        editor.is_cropping,
    )
}

pub fn trigger_async_render(editor: &mut RawEditor) -> Task<Message> {
    let t_update_start = std::time::Instant::now();

    // Oriented display dims of what is actually on screen — crop-aware, so the
    // render target takes the CROPPED aspect. Must be the same source the
    // viewport widget and the input math use, or the texture is drawn at a
    // different shape than it was rendered for.
    let (disp_w, disp_h) = editor.image_display_dims();

    if let (Some(ctx), Some(resources)) = (&editor.gpu_context, &editor.image_resources) {
        let ctx = ctx.clone();
        let resources = resources.clone();

        // Render only the slice of the image that is actually on screen, at
        // the resolution it is displayed at. Sizing the target to the whole
        // image at zoom-scaled resolution made its area grow as zoom^2 — at
        // 100% on a 24MP file that meant rendering, reading back and
        // re-uploading ~24M pixels to display ~2M. (Crop mode reveals the
        // full sensor and disables zoom/pan, so it renders the whole image.)
        let target = render_target(editor);
        let view_rect = target.view_rect;
        let (target_w, target_h) = (target.width, target.height);

        // Only the REQUESTED window moves here. `rendered_view_rect` — what the
        // viewport actually draws against — is published in
        // `handle_render_finished`, together with the bytes it describes.
        // Updating it now would place the still-current texture at the incoming
        // window's position for the length of the render, which looks like the
        // image jumping.
        editor.requested_view_rect = view_rect;
        tracing::debug!(
            "render window: zoom={:.2} view_rect=[{:.3},{:.3},{:.3},{:.3}] target={}x{}",
            editor.zoom,
            view_rect[0],
            view_rect[1],
            view_rect[2],
            view_rect[3],
            target_w,
            target_h,
        );
        let update_ms = t_update_start.elapsed().as_secs_f32() * 1000.0;

        let overview_dims = crate::core::viewport::overview_dims(disp_w, disp_h);
        let image_id = resources.image_id;
        // Only a render of the user's real edit may become the image's proxy:
        // the before/after peek shows other params, and crop mode shows the
        // whole sensor with the discarded area dimmed. The uniforms hold
        // `current_edit_params` whenever neither is active — every change to
        // them is pushed to the GPU as it happens.
        // The mask overlay tints the render, so it disqualifies too.
        // So does one whose noise reduction is held back mid-drag.
        let edit = (!editor.before_peek
            && !editor.is_cropping
            && editor.current_edit_params.mask_overlay == 0
            && editor.held_noise.is_none())
            .then_some(editor.current_edit_params);

        // A subsampled texture shown larger than its own pixels is soft
        // however it is rendered: fetch full resolution. Asked here, by the
        // render itself, rather than by the zoom handler, so it also covers
        // a view that is already zoomed in when an image loads (zoom is kept
        // across images and tabs), a window resize, and HiDPI screens.
        let upgrade = if target.magnification > crate::core::viewport::FULL_RES_MAGNIFICATION
            && editor.current_tab == crate::app::message::AppTab::Develop
        {
            crate::app::handlers::loading::trigger_full_res_upgrade(editor)
        } else {
            Task::none()
        };

        // After a full-resolution upgrade the subsampled preview is kept
        // (`preview_resources`). Render from it whenever full resolution adds
        // nothing: the overview always (384px), and the display while the
        // view does not magnify the preview — or while a noise-reduction
        // slider is dragged, whose stage re-runs over every SOURCE pixel per
        // tick. The release re-renders at full resolution (`finish_stage_drag`).
        let preview = editor
            .preview_resources
            .clone()
            .filter(|p| p.image_id == resources.image_id);
        let (display_res, overview_res) = match preview {
            Some(preview) => {
                // Same state as the full-resolution set: its uniforms are the
                // ones every edit path pushes to.
                let params = resources.pushed_params();
                let (interpolated, wb_override) = resolve_wb_and_dcp(editor, &params);
                preview.update_uniforms(&ctx, &params, interpolated.as_deref(), wb_override);
                let preview_magnification =
                    target.magnification * resources.width as f32 / preview.width.max(1) as f32;
                let fits = preview_magnification <= crate::core::viewport::FULL_RES_MAGNIFICATION;
                editor.display_from_drag_preview = !fits && editor.stage_drag;
                let display = if fits || editor.stage_drag { preview.clone() } else { resources };
                (display, preview)
            }
            None => {
                editor.display_from_drag_preview = false;
                (resources.clone(), resources)
            }
        };

        let render = Task::perform(
            async move {
                // Display window and the whole-image overview share one GPU
                // wait; the overview is ~100k pixels, so there is no reason to
                // report it separately from the render.
                crate::gpu::render_functions::render_display_and_overview(
                    &ctx,
                    &display_res,
                    &overview_res,
                    target_w,
                    target_h,
                    view_rect,
                    overview_dims,
                )
                .await
            },
            move |render| {
                Message::RenderFinished(Box::new(crate::app::message::RenderOutput {
                    image_id,
                    display: render.display,
                    dims: (target_w, target_h),
                    view_rect,
                    overview: render.overview,
                    overview_dims,
                    histogram: render.histogram,
                    edit,
                    upload_ms: render.encode_ms,
                    render_ms: render.gpu_ms,
                    update_ms,
                }))
            },
        );
        return Task::batch([render, upgrade]);
    }
    // GPU context not ready — release the lock so the next slider event can retry.
    editor.is_rendering = false;
    editor.pending_render = false;
    Task::none()
}

pub fn handle_toggle_wb_picker(editor: &mut RawEditor) -> Task<Message> {
    // Develop-only, same reasoning as handle_toggle_crop.
    if editor.active_modal != crate::app::state::Modal::None
        || editor.current_tab != crate::app::message::AppTab::Develop
    {
        return Task::none();
    }
    editor.is_wb_picking = !editor.is_wb_picking;
    if editor.is_wb_picking {
        // Tools are mutually exclusive; update_pipeline below clears the
        // shader's crop flag to match.
        if editor.is_cropping {
            editor.is_cropping = false;
            editor.drag_mode = DragMode::None;
        }
        editor.mask_tool = crate::app::state::MaskTool::Inactive;
        editor.selected_mask = None;
        editor.brush_armed = false;
        editor.color_picking_mask = None;
        editor.loupe.targeting = false;
        editor.is_leveling = false;
        editor.status = "WB picker: click a neutral grey/white area".to_string();
    } else {
        editor.status.clear();
    }
    update_pipeline(editor)
}

/// Why a raw sample could not be taken.
pub(crate) enum RawSampleError {
    /// No usable raw data (not in memory, or too small).
    Missing(&'static str),
}

/// Mean black-subtracted camera (R, G, B) of the raw mosaic in a 16×16
/// window at `(u, v)` — display-space UV within the rendered (cropped) image,
/// as the overlay's eyedroppers report it — plus whether any site clipped.
///
/// Shared by the WB picker and the colour-range mask picker. The UV goes
/// through the same crop remap and EXIF orientation as the fragment shader.
pub(crate) fn sample_raw_rgb(
    editor: &RawEditor,
    image_id: i64,
    u: f32,
    v: f32,
) -> Result<([f32; 3], bool, std::sync::Arc<crate::raw::loader::RawDataResult>), RawSampleError> {
    let Some(raw) = editor.raw_cache.peek(&image_id).cloned() else {
        return Err(RawSampleError::Missing("raw data not in memory (reopen the image)"));
    };
    // The 16×16 sample window below needs room on both sides; `clamp(8, w-9)`
    // panics outright on anything narrower.
    if raw.data.is_empty() || raw.width < 17 || raw.height < 17 {
        return Err(RawSampleError::Missing("no raw pixels available"));
    }

    // Display UV → sensor UV: undo the crop remap (the vertex shader shows the
    // crop sub-rect when not in crop mode), then the orientation mapping —
    // EXIF's plus the user's quarter turns, the same transform the fragment
    // shader applies.
    let crop = editor.current_edit_params.crop;
    let cu = crop[0] + u.clamp(0.0, 1.0) * crop[2];
    let cv = crop[1] + v.clamp(0.0, 1.0) * crop[3];
    let orientation =
        crate::color::compose_orientation(raw.orientation, editor.current_edit_params.quarter_turns);
    let (su, sv) = crate::color::display_to_sensor_uv(cu, cv, orientation);

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
    let mut clipped = false;
    for y in (cy - 8)..(cy + 8) {
        for x in (cx - 8)..(cx + 8) {
            let value = raw.data[(y * w + x) as usize];
            clipped |= value >= clip_threshold;
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
    let mean = |c: usize| (sums[c] / counts[c].max(1) as f64) as f32;
    Ok(([mean(0), mean(1), mean(2)], clipped, raw))
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
    let (rgb, clipped, raw) = match sample_raw_rgb(editor, image_id, u, v) {
        Ok(sample) => sample,
        Err(RawSampleError::Missing(msg)) => {
            editor.is_wb_picking = false;
            editor.status = format!("WB picker: {msg}");
            return Task::none();
        }
    };
    if clipped {
        editor.status = "WB picker: area is clipped — pick a darker neutral".to_string();
        return Task::none();
    }
    let [r, g, b] = rgb;
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

/// Tolerance for reusing a memoized DCP interpolation. `kelvin` comes out of
/// `solve_wb`, which is deterministic, so identical slider state reproduces it
/// bit-for-bit; the epsilon only absorbs noise, it does not coarsen the slider.
const DCP_MEMO_KELVIN_EPS: f32 = 0.01;

/// True when a memo recorded at `(memo_kelvin, memo_tone)` is still valid for
/// `(kelvin, tone)`.
///
/// Tone settings compare exactly rather than by epsilon: they come straight
/// off the UI as the same f32s each time, and an epsilon on a *set* of shape
/// parameters is a good way to miss a change to one of them.
fn dcp_memo_is_valid(
    memo_kelvin: f32,
    memo_tone: ToneSettings,
    kelvin: f32,
    tone: ToneSettings,
) -> bool {
    (memo_kelvin - kelvin).abs() < DCP_MEMO_KELVIN_EPS && memo_tone == tone
}

/// Resolve the DCP interpolation and WB override for the editor's current
/// slider state. Shared by every path that refreshes uniforms (slider edits,
/// image-ready, preview upgrades).
///
/// The interpolation depends only on `(kelvin, ToneSettings)`, but this is
/// called from `update_pipeline` on every slider tick — including sliders that
/// cannot affect it. It is memoized in `editor.dcp_memo` because rebuilding the
/// HueSatMap LUT and re-baking the tone curve is a multi-allocation,
/// `powf`-heavy job that `update_uniforms` would then throw away via its own
/// dirty-check.
///
/// The two dirty-checks are independent and cannot disagree harmfully: both are
/// functions of the same slider state, so a miss on either side merely repeats
/// work that produces the same result.
///
/// `params` is passed in rather than read from `editor` because the
/// before/after peek renders a DIFFERENT set of params than the ones the user
/// is editing. Reading `editor.current_edit_params` here would give the peek
/// default exposure with slider-derived white balance and tone curve — a
/// hybrid of neither state.
pub fn resolve_wb_and_dcp(
    editor: &RawEditor,
    params: &crate::core::types::EditParams,
) -> (Option<std::sync::Arc<crate::raw::dcp::InterpolatedProfile>>, Option<[f32; 4]>) {
    let as_shot = editor.as_shot_wb.unwrap_or((5000.0, 0.0));
    let fallback_matrix = editor
        .current_metadata
        .as_ref()
        .map(|m| m.color_matrix)
        .unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    let (kelvin, _tint, wb_override) = crate::color::solve_wb(
        params,
        as_shot,
        editor.current_dcp_profile.as_deref(),
        fallback_matrix,
    );

    let Some(dcp) = editor.current_dcp_profile.as_ref() else {
        return (None, wb_override);
    };
    let tone = ToneSettings::from_params(params);

    if let Ok(memo) = editor.dcp_memo.try_borrow() {
        if let Some((k, t, profile)) = memo.as_ref() {
            if dcp_memo_is_valid(*k, *t, kelvin, tone) {
                return (Some(profile.clone()), wb_override);
            }
        }
    }

    let interpolated = std::sync::Arc::new(crate::raw::dcp::interpolate_at_temperature(
        dcp, kelvin, tone,
    ));
    if let Ok(mut memo) = editor.dcp_memo.try_borrow_mut() {
        *memo = Some((kelvin, tone, interpolated.clone()));
    }

    (Some(interpolated), wb_override)
}

/// Push `params` to the GPU and render with them.
///
/// Deliberately does NOT mark edits dirty — the caller decides whether these
/// params represent the user's real state. The before/after peek renders
/// params that must never be persisted.
pub(crate) fn push_uniforms_and_render(
    editor: &mut RawEditor,
    params: &crate::core::types::EditParams,
) -> Task<Message> {
    // `params` belong to the SELECTED image, but the loaded GPU resources can
    // still be another one's — Library and Cull change the selection without
    // loading. Rendering them together (an undo or reset pressed in Library)
    // drew the previous Develop image with this image's edits, and Develop
    // then showed that frame when the user went back to it.
    if editor.image_resources.as_ref().map(|r| r.image_id) != editor.selected_image_id {
        return Task::none();
    }
    // A different camera profile can't be pushed as uniforms (its LUT sizes
    // a texture): undo, paste or reset changed it — rebuild instead.
    if crate::app::handlers::camera_profiles::resources_stale(editor) {
        return crate::app::handlers::camera_profiles::rebuild(editor);
    }
    if let (Some(ctx), Some(resources)) = (&editor.gpu_context, &editor.image_resources) {
        // Phase 140: Interpolate DCP + resolve slider WB
        let (interpolated, wb_override) = resolve_wb_and_dcp(editor, params);
        resources.update_uniforms(ctx, params, interpolated.as_deref(), wb_override);
        editor.canvas_cache.clear();
    }

    // The detail loupe follows every edit (it costs ~0.3 ms when the noise
    // settings haven't changed).
    let loupe = crate::app::handlers::loupe::request(editor);

    // Phase 106: Throttling
    let render = if editor.is_rendering {
        editor.pending_render = true;
        Task::none()
    } else {
        editor.is_rendering = true;
        editor.pending_render = false;
        trigger_async_render(editor)
    };
    Task::batch([render, loupe])
}

pub(crate) fn update_pipeline(editor: &mut RawEditor) -> Task<Message> {
    // Deferred, not written here: this runs once per mouse-move during a
    // slider or mask drag, and a synchronous SQLite write on that path cost
    // an fsync per event on the UI thread. `flush_edits` at the commit points
    // (CommitEdit, drag release, image/tab switch, close) does the write.
    editor.mark_edits_dirty();
    editor.sync_tool_flags();
    // EditParams is Copy, so this sidesteps the borrow conflict for free.
    let params = editor.view_params();
    push_uniforms_and_render(editor, &params)
}

/// Render the params the viewport should currently be showing — the
/// before-reference while peeking, the user's real edits otherwise.
///
/// Every path that needs to *leave* the peek goes through here rather than
/// poking `before_peek` directly, so the flag and the uploaded uniforms can
/// never disagree.
pub(crate) fn refresh_for_peek_state(editor: &mut RawEditor) -> Task<Message> {
    let params = if editor.before_peek {
        editor.current_edit_params.before_reference()
    } else {
        editor.view_params()
    };
    push_uniforms_and_render(editor, &params)
}

/// Leave the before/after peek if it is active, re-rendering the real edit.
///
/// Called from anywhere the peek could otherwise get stranded — tab change,
/// image change, modal open, window focus loss. Returns `Task::none()` when
/// there was nothing to do, so it is safe to call unconditionally.
pub fn clear_before_peek(editor: &mut RawEditor) -> Task<Message> {
    if !editor.before_peek {
        return Task::none();
    }
    editor.before_peek = false;
    refresh_for_peek_state(editor)
}

/// Hold-to-peek at the unedited original (Space).
///
/// Hold rather than toggle: releasing the key always returns you to your edit,
/// so you cannot get stranded looking at the original wondering why the
/// sliders do nothing.
pub fn handle_set_before_peek(editor: &mut RawEditor, active: bool) -> Task<Message> {
    // Key auto-repeat delivers KeyPressed continuously while Space is held.
    // Without this guard every repeat would queue another GPU render.
    if editor.before_peek == active {
        return Task::none();
    }

    if active {
        // Space is bound globally and the subscription closure has no access
        // to editor state, so all gating lives here — the same pattern the
        // Delete modal's Enter/D bindings use.
        if editor.active_modal != crate::app::state::Modal::None
            || editor.current_tab != crate::app::message::AppTab::Develop
            || !matches!(
                editor.editor_readiness,
                crate::app::state::EditorReadiness::Ready(_)
            )
        {
            return Task::none();
        }
    }

    editor.before_peek = active;
    refresh_for_peek_state(editor)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::core::tonemap::{GtParams, ToneMapper};

    fn tone(curve: f32) -> ToneSettings {
        ToneSettings { mapper: ToneMapper::Camera, profile_curve: curve, gt: GtParams::default() }
    }

    #[test]
    fn dcp_memo_hits_on_identical_state() {
        assert!(dcp_memo_is_valid(5500.0, tone(0.33), 5500.0, tone(0.33)));
    }

    #[test]
    fn dcp_memo_misses_when_temperature_moves() {
        // One Kelvin is far below what any slider step produces, and must
        // still invalidate — the memo must never coarsen the temperature.
        assert!(!dcp_memo_is_valid(5500.0, tone(0.33), 5501.0, tone(0.33)));
    }

    #[test]
    fn dcp_memo_misses_when_profile_curve_moves() {
        assert!(!dcp_memo_is_valid(5500.0, tone(0.33), 5500.0, tone(0.34)));
    }

    #[test]
    fn dcp_memo_absorbs_kelvin_noise_only() {
        // Recomputing solve_wb from identical inputs is deterministic, so the
        // epsilon exists purely to tolerate last-bit drift, not real changes.
        assert!(dcp_memo_is_valid(5500.0, tone(0.33), 5500.0 + 1e-4, tone(0.33)));
    }

    /// The memo caches a baked tone LUT, so switching operators MUST miss —
    /// otherwise the new selection would not reach the GPU until something
    /// else happened to move the temperature.
    #[test]
    fn dcp_memo_misses_when_the_tone_operator_changes() {
        let mut gt = tone(0.33);
        gt.mapper = ToneMapper::Gt;
        assert!(!dcp_memo_is_valid(5500.0, tone(0.33), 5500.0, gt));
    }

    /// Same for the GT shape sliders — they re-bake the curve, and nothing
    /// else in the key moves when the user drags one.
    #[test]
    fn dcp_memo_misses_when_gt_shape_changes() {
        let mut a = tone(0.33);
        a.mapper = ToneMapper::Gt;
        let mut b = a;
        b.gt.contrast = 1.4;
        assert!(!dcp_memo_is_valid(5500.0, a, 5500.0, b));
    }
}
