use iced::{Task, Point};
use iced::widget::image::Handle;
use crate::app::state::{RawEditor, EditorReadiness, DragMode};
use crate::app::message::{Message, AppTab};
use crate::raw;
use crate::ui::preview_renderer::{CropHandle, MaskHandle};

pub fn handle_image_selected(editor: &mut RawEditor, image_id: i64) -> Task<Message> {
    if editor.last_modifiers.command() {
        if !editor.multi_selection.remove(&image_id) { editor.multi_selection.insert(image_id); }
    } else {
        editor.multi_selection.clear();
        editor.multi_selection.insert(image_id);
    }
    editor.selected_image_id = Some(image_id);
    editor.canvas_cache.clear();
    // Mask selection is an index into the previous image's mask list — clear
    // both it and any pending placement mode when the image changes.
    editor.selected_mask = None;
    editor.mask_tool = crate::app::state::MaskTool::Inactive;

    if let Some(library) = &editor.library {
        editor.current_edit_params = library.load_edit_params(image_id).unwrap_or_default();
        editor.history_map.entry(image_id).or_insert_with(|| (vec![editor.current_edit_params], 0));
    }
    
    if editor.current_tab == AppTab::Develop || editor.current_tab == AppTab::Cull {
        let needs_load = match &editor.editor_readiness {
            EditorReadiness::Ready(id) => *id != image_id,
            EditorReadiness::Loading(id) => *id != image_id,
            _ => true,
        };
        
        if needs_load {
            return trigger_image_load(editor, image_id);
        }
    }
    Task::none()
}

pub fn handle_tab_changed(editor: &mut RawEditor, tab: AppTab) -> Task<Message> {
    editor.current_tab = tab;
    if tab == AppTab::Develop {
        if let Some(image_id) = editor.selected_image_id {
            let needs_load = match &editor.editor_readiness {
                EditorReadiness::Ready(id) => *id != image_id,
                EditorReadiness::Loading(id) => *id != image_id,
                _ => true,
            };
            if needs_load {
                return trigger_image_load(editor, image_id);
            }
        }
    }
    Task::none()
}

pub fn handle_select_next_image(editor: &mut RawEditor) -> Task<Message> {
    if let Some(id) = editor.selected_image_id {
        if let Some(idx) = editor.images.iter().position(|i| i.id == id) {
            let next = editor.images[(idx + 1) % editor.images.len()].id;
            return Task::done(Message::ImageSelected(next));
        }
    }
    Task::none()
}

pub fn handle_select_previous_image(editor: &mut RawEditor) -> Task<Message> {
    if let Some(id) = editor.selected_image_id {
        if let Some(idx) = editor.images.iter().position(|i| i.id == id) {
            let prev = editor.images[if idx == 0 { editor.images.len() - 1 } else { idx - 1 }].id;
            return Task::done(Message::ImageSelected(prev));
        }
    }
    Task::none()
}

pub fn handle_zoom(editor: &mut RawEditor, d: f32, mut p: Point) -> Task<Message> {
    if editor.is_cropping { return Task::none(); }
    if p.x < 0.0 { p = editor.last_cursor_position.unwrap_or(Point::ORIGIN); }

    if let Some(resources) = &editor.image_resources {
        let old_zoom = editor.zoom;
        let iw = resources.width as f32;
        let ih = resources.height as f32;
        let img_aspect = iw / ih;
        let (vw, vh) = editor.viewport_size;
        let vp_aspect = vw / vh;

        let (fw, fh) = if img_aspect > vp_aspect {
            (vw, vw / img_aspect)
        } else {
            (vh * img_aspect, vh)
        };

        let xo = (vw - fw) / 2.0;
        let yo = (vh - fh) / 2.0;

        // Cursor position in fitted-image space [0..1]
        let icx = (p.x - xo).clamp(0.0, fw);
        let icy = (p.y - yo).clamp(0.0, fh);
        let nx = icx / fw;
        let ny = icy / fh;

        // pan_x/pan_y are in UV space (what the shader sees).
        // ViewportProgram converts: pan_x = offset.x * (fw/vw), pan_y = offset.y * (fh/vh)
        let scale_x = if vw > 0.0 { fw / vw } else { 1.0 };
        let scale_y = if vh > 0.0 { fh / vh } else { 1.0 };
        let pan_x = editor.pan_offset.x * scale_x;
        let pan_y = editor.pan_offset.y * scale_y;

        let new_zoom = if d > 0.0 { old_zoom * (1.0 + d * 0.8) } else { old_zoom / (1.0 + (-d * 0.8)) }.clamp(0.1, 10.0);
        editor.zoom = new_zoom;

        // Image UV under cursor must stay fixed: img_uv = (n - 0.5)/zoom - pan + 0.5
        let img_uv_x = (nx - 0.5) / old_zoom - pan_x + 0.5;
        let img_uv_y = (ny - 0.5) / old_zoom - pan_y + 0.5;

        // Solve for new offset: new_pan = (n-0.5)/new_zoom - img_uv + 0.5
        let new_pan_x = (nx - 0.5) / editor.zoom - img_uv_x + 0.5;
        let new_pan_y = (ny - 0.5) / editor.zoom - img_uv_y + 0.5;
        editor.pan_offset.x = if scale_x > 0.0 { new_pan_x / scale_x } else { 0.0 };
        editor.pan_offset.y = if scale_y > 0.0 { new_pan_y / scale_y } else { 0.0 };
    } else {
        editor.zoom = if d > 0.0 { editor.zoom * (1.0 + d * 0.8) } else { editor.zoom / (1.0 + (-d * 0.8)) }.clamp(0.1, 10.0);
    }
    editor.canvas_cache.clear();

    // Re-render at zoom-appropriate resolution, and upgrade to full-res debayer
    // when the user zooms in past the subsampled texture's useful range.
    use crate::app::state::EditorReadiness;
    if editor.current_tab == AppTab::Develop
        && matches!(editor.editor_readiness, EditorReadiness::Ready(_))
    {
        // Apply the same is_rendering/pending_render throttle as update_pipeline.
        // Without this, each scroll event spawns a new GPU task (dozens of concurrent
        // readback buffers → CPU/RAM spike). With throttling: at most one in-flight
        // render + one pending; the pending render fires after the current one finishes,
        // picking up the latest zoom at that point.
        let render_task = if editor.is_rendering {
            editor.pending_render = true;
            Task::none()
        } else {
            editor.is_rendering = true;
            editor.pending_render = false;
            crate::app::handlers::develop::trigger_async_render(editor)
        };

        let upgrade_task = if editor.zoom > 1.5 {
            crate::app::handlers::loading::trigger_full_res_upgrade(editor)
        } else {
            Task::none()
        };

        return Task::batch(vec![render_task, upgrade_task]);
    }
    Task::none()
}

pub fn handle_pan(editor: &mut RawEditor, d: cgmath::Vector2<f32>) -> Task<Message> {
    if editor.is_cropping { return Task::none(); }
    // Phase 116: Remove 1/zoom scaling. Standardised to 1:1 mapping with screen pixels.
    editor.pan_offset.x += d.x;
    editor.pan_offset.y += d.y;
    editor.canvas_cache.clear();
    Task::none()
}

pub fn handle_reset_view(editor: &mut RawEditor) -> Task<Message> {
    editor.zoom = 1.0;
    editor.pan_offset = cgmath::Vector2::new(0.0, 0.0);
    editor.canvas_cache.clear();
    Task::none()
}

pub fn handle_mouse_pressed(editor: &mut RawEditor) -> Task<Message> {
    let now = std::time::Instant::now();
    let double = editor.last_click_time.map(|t| now.duration_since(t).as_millis() < 300).unwrap_or(false);
    editor.last_click_time = Some(now);
    if double { return Task::done(Message::ResetView); }
    if !editor.is_cropping && !editor.is_wb_picking && !editor.mask_overlay_active() {
        editor.is_dragging = true;
        editor.drag_mode = DragMode::Pan;
    }
    Task::none()
}

pub fn handle_mouse_released(editor: &mut RawEditor) -> Task<Message> {
    if editor.is_dragging
        && matches!(
            editor.drag_mode,
            DragMode::CropHandle(_) | DragMode::MaskHandle(_)
        )
    {
        editor.save_current_edits();
        editor.commit_current_state();
        if let (Some(ctx), Some(res)) = (&editor.gpu_context, &editor.image_resources) {
            let (interpolated, wb_override) =
                crate::app::handlers::develop::resolve_wb_and_dcp(editor);
            res.update_uniforms(ctx, &editor.current_edit_params, interpolated.as_ref(), wb_override);
        }
    }
    editor.is_dragging = false;
    editor.drag_mode = DragMode::None;
    editor.last_cursor_position = None;
    Task::none()
}

pub fn handle_mouse_moved(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    // Phase 116: Remove broken viewport_size auto-inflation loop.
    // viewport_size is now managed via Message::ViewportResized.
    
    if editor.is_cropping {
        handle_crop_interaction(editor, pos)
    } else if editor.mask_overlay_active()
        || matches!(editor.drag_mode, DragMode::MaskHandle(_))
    {
        handle_mask_interaction(editor, pos)
    } else {
        handle_pan_interaction(editor, pos)
    }
}

pub fn handle_working_preview_ready(
    editor: &mut RawEditor,
    id: i64,
    handle: Handle,
    bytes: Option<std::sync::Arc<[u8]>>,
    dims: (u32, u32),
) -> Task<Message> {
    if Some(id) == editor.selected_image_id {
        editor.working_preview = Some(handle);
        editor.working_preview_bytes = bytes;
        editor.working_preview_dims = dims;
    }
    Task::none()
}

pub fn handle_preview_cached(
    editor: &mut RawEditor,
    id: i64,
    result: Result<(u32, u32, std::sync::Arc<[u8]>), String>,
) -> Task<Message> {
    // Phase 78: Cleanup pending load
    editor.pending_loads.remove(&id);
    // Phase 81: Cleanup queued load
    editor.queued_loads.retain(|(i, _)| *i != id);
    
    if let Ok((width, height, pixels)) = result {
        let handle = iced::widget::image::Handle::from_rgba(width, height, pixels.to_vec());
        editor.preview_cache.put(id, handle);
    }
    Task::none()
}

// Helpers

fn handle_pan_interaction(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    if editor.is_dragging && editor.drag_mode == DragMode::Pan {
        if let Some(last) = editor.last_cursor_position {
            let delta = pos - last;
            editor.last_cursor_position = Some(pos);

            let (vw, vh) = editor.viewport_size;
            let zoom = editor.zoom;

            // Fitted image dimensions (same logic as ViewportProgram::draw and handle_zoom)
            let (fitted_w, fitted_h) = if let Some(res) = &editor.image_resources {
                let img_aspect = res.width as f32 / res.height as f32;
                let vp_aspect = if vh > 0.0 { vw / vh } else { 1.0 };
                if img_aspect > vp_aspect {
                    (vw, vw / img_aspect)
                } else {
                    (vh * img_aspect, vh)
                }
            } else {
                (vw.max(1.0), vh.max(1.0))
            };

            // For 1:1 cursor tracking, account for the quad-to-viewport scale factor.
            // The shader UV spans the quad's pixel width (fitted_w), not the full viewport (vw).
            // Δpan_x = Δx / (fitted_w * zoom) where Δx is in quad-pixels, but Δx is in viewport
            // pixels. So Δpan_x = Δx / (fit_w * vw * zoom) where fit_w = fitted_w / vw.
            // pan_x (UV) = offset.x * fit_w → Δoffset.x = Δpan_x / fit_w = Δx / (fit_w² * vw * zoom)
            //                                             = Δx * vw / (fitted_w² * zoom)
            let dx = if fitted_w > 0.0 { delta.x * vw / (fitted_w * fitted_w * zoom) } else { 0.0 };
            let dy = if fitted_h > 0.0 { delta.y * vh / (fitted_h * fitted_h * zoom) } else { 0.0 };
            return Task::done(Message::Pan(cgmath::Vector2::new(dx, dy)));
        }
    }
    editor.last_cursor_position = Some(pos);
    Task::none()
}

fn handle_crop_interaction(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    if editor.is_dragging {
        if let DragMode::CropHandle(h) = editor.drag_mode {
            if let Some(last) = editor.last_cursor_position {
                apply_crop_drag(editor, pos, last, h);
                editor.last_cursor_position = Some(pos);
                editor.canvas_cache.clear();
            }
        }
    } else {
        editor.last_cursor_position = Some(pos);
    }
    Task::none()
}

fn apply_crop_drag(editor: &mut RawEditor, pos: Point, last: Point, h: CropHandle) {
    let resources = match &editor.image_resources {
        Some(r) => r,
        None => return,
    };
    
    let img_aspect = resources.width as f32 / resources.height as f32;
    let (bw, bh) = editor.viewport_size;
    let vp_aspect = bw / bh;
    
    let (fw, fh) = if img_aspect > vp_aspect {
        (bw, bw / img_aspect)
    } else {
        (bh * img_aspect, bh)
    };
    
    let zw = fw * editor.zoom;
    let zh = fh * editor.zoom;
    
    // Scale delta from screen pixels to normalized image [0,1]
    let dx = (pos.x - last.x) / zw;
    let dy = (pos.y - last.y) / zh;
    
    let c = editor.current_edit_params.crop;
    let (mut l, mut t, mut r, mut b) = (c[0], c[1], c[0]+c[2], c[1]+c[3]);
    
    match h {
        CropHandle::TopLeft => { l += dx; t += dy; }
        CropHandle::TopRight => { t += dy; r += dx; }
        CropHandle::BottomLeft => { l += dx; b += dy; }
        CropHandle::BottomRight => { r += dx; b += dy; }
        CropHandle::Body => { 
            l += dx; t += dy; r += dx; b += dy; 
            if l < 0.0 { r -= l; l = 0.0; } 
            if r > 1.0 { l -= r - 1.0; r = 1.0; } 
            if t < 0.0 { b -= t; t = 0.0; } 
            if b > 1.0 { t -= b - 1.0; b = 1.0; } 
        }
    }
    
    if h != CropHandle::Body {
        let min = 0.01;
        match h {
            CropHandle::TopLeft => { l = l.min(r - min).max(0.0); t = t.min(b - min).max(0.0); }
            CropHandle::TopRight => { r = r.max(l + min).min(1.0); t = t.min(b - min).max(0.0); }
            CropHandle::BottomLeft => { l = l.min(r - min).max(0.0); b = b.max(t + min).min(1.0); }
            CropHandle::BottomRight => { r = r.max(l + min).min(1.0); b = b.max(t + min).min(1.0); }
            _ => {}
        }
    }
    
    let l = l.clamp(0.0, 1.0);
    let t = t.clamp(0.0, 1.0);
    let r = r.clamp(0.0, 1.0);
    let b = b.clamp(0.0, 1.0);
    
    editor.current_edit_params.crop = [l, t, (r - l).max(0.0), (b - t).max(0.0)];
    
    if let Some(ctx) = &editor.gpu_context {
        let (interpolated, wb_override) =
            crate::app::handlers::develop::resolve_wb_and_dcp(editor);
        resources.update_uniforms(ctx, &editor.current_edit_params, interpolated.as_ref(), wb_override);
    }
}

fn handle_mask_interaction(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    if editor.is_dragging {
        if let DragMode::MaskHandle(h) = editor.drag_mode {
            if let Some(last) = editor.last_cursor_position {
                apply_mask_drag(editor, pos, last, h);
                editor.last_cursor_position = Some(pos);
                editor.canvas_cache.clear();
                // Unlike crop (a canvas-only overlay), the mask's effect is
                // baked into the render — re-render live, throttled.
                return crate::app::handlers::develop::update_pipeline(editor);
            }
        }
    } else {
        editor.last_cursor_position = Some(pos);
    }
    Task::none()
}

fn apply_mask_drag(editor: &mut RawEditor, pos: Point, last: Point, h: MaskHandle) {
    let Some(index) = editor.selected_mask else {
        return;
    };
    if index >= editor.current_edit_params.mask_count as usize {
        return;
    }
    let Some(resources) = &editor.image_resources else {
        return;
    };

    // Same letterbox/zoom math as apply_crop_drag, but with the ORIENTED
    // dimensions — the displayed image is upright, portrait shots included.
    let (ow, oh) = resources.oriented_dims();
    let img_aspect = ow as f32 / oh.max(1) as f32;
    let (bw, bh) = editor.viewport_size;
    let vp_aspect = bw / bh;

    let (fw, fh) = if img_aspect > vp_aspect {
        (bw, bw / img_aspect)
    } else {
        (bh * img_aspect, bh)
    };

    let zw = fw * editor.zoom;
    let zh = fh * editor.zoom;

    // Screen deltas map to visible-area UV; mask geometry lives in full-image
    // UV (the crop sub-rect is what's displayed), so scale by the crop extent.
    let crop = editor.current_edit_params.crop;
    let du = (pos.x - last.x) / zw.max(1.0) * crop[2];
    let dv = (pos.y - last.y) / zh.max(1.0) * crop[3];

    let m = &mut editor.current_edit_params.masks[index];
    match h {
        MaskHandle::LinearStart => {
            m.ax += du;
            m.ay += dv;
        }
        MaskHandle::LinearEnd => {
            m.bx += du;
            m.by += dv;
        }
        MaskHandle::Center => {
            // Radial: move the center. Linear: move the whole gradient.
            m.ax += du;
            m.ay += dv;
            if m.mask_type == 0 {
                m.bx += du;
                m.by += dv;
            }
        }
        MaskHandle::RadiusX => m.bx = (m.bx + du).max(0.005),
        MaskHandle::RadiusY => m.by = (m.by + dv).max(0.005),
    }
}

fn trigger_image_load(editor: &mut RawEditor, image_id: i64) -> Task<Message> {
    editor.working_preview = None;
    editor.full_res_upgrading = false;
    if let Some((working_cache_path, raw_path)) = editor
        .images
        .iter()
        .find(|i| i.id == image_id)
        .map(|img| (img.cache_path_working.clone(), img.path.clone()))
    {
        // Phase 73: Check RAM cache first
        if let Some(handle) = editor.preview_cache.get(&image_id) {
            editor.working_preview = Some(handle.clone());
        } else if let Some(path) = &working_cache_path {
            editor.working_preview = Some(Handle::from_path(path.clone()));
        }
        
        editor.editor_readiness = EditorReadiness::Loading(image_id);
        let mut tasks = Vec::new();
        if let Some(path) = &working_cache_path {
            tasks.push(Task::perform(
                load_image_handle(image_id, path.clone()),
                |(id, h, p, d)| Message::WorkingPreviewReady(id, h, p, d),
            ));
        }
        
        // Only load full RAW data if we are in Develop mode
        if editor.current_tab == AppTab::Develop {
            let cached_raw = editor.raw_cache.get(&image_id).cloned();
            if let Some(cached_raw) = cached_raw {
                tasks.push(crate::app::handlers::loading::prepare_image_resources_from_raw(
                    editor,
                    image_id,
                    cached_raw.clone(),
                ));
            } else if !editor.pending_raw_loads.contains(&image_id) {
                tasks.push(Task::perform(
                    raw::loader::load_raw_data(raw_path),
                    move |res| Message::RawDataLoaded(image_id, res),
                ));
            } else if let Some(pos) = editor
                .queued_raw_loads
                .iter()
                .position(|(queued_id, _)| *queued_id == image_id)
            {
                let prioritized = editor.queued_raw_loads.remove(pos);
                editor.queued_raw_loads.insert(0, prioritized);
            }
        }
        
        // Schedule preloads for adjacent images
        tasks.push(schedule_preloads(editor));
        if editor.current_tab == AppTab::Develop {
            tasks.push(schedule_raw_preloads(editor));
        }
        
        return Task::batch(tasks);
    }
    Task::none()
}

fn schedule_preloads(editor: &mut RawEditor) -> Task<Message> {
    let missing = identify_missing_preloads(editor);
    for (id, path) in missing {
        editor.pending_loads.insert(id);
        editor.queued_loads.push((id, path));
    }
    Task::none()
}

fn schedule_raw_preloads(editor: &mut RawEditor) -> Task<Message> {
    if editor.raw_preload_budget_mb == 0 {
        return Task::none();
    }

    let missing = identify_missing_raw_preloads(editor);
    for (id, path) in missing {
        editor.pending_raw_loads.insert(id);
        editor.queued_raw_loads.push((id, path));
    }
    Task::none()
}

fn identify_missing_preloads(editor: &RawEditor) -> Vec<(i64, String)> {
    let mut missing = Vec::new();
    if let Some(current_id) = editor.selected_image_id {
        if let Some(current_idx) = editor.images.iter().position(|i| i.id == current_id) {
            let total = editor.images.len() as isize;
            let behind = editor.preview_preload_behind as isize;
            let ahead = editor.preview_preload_ahead as isize;

            for offset in -behind..=ahead {
                if offset == 0 { continue; }
                
                let mut target_idx = current_idx as isize + offset;
                
                if target_idx < 0 { target_idx += total; }
                if target_idx >= total { target_idx -= total; }
                
                let target_idx = target_idx as usize;
                if target_idx < editor.images.len() {
                    let img = &editor.images[target_idx];
                    
                    if editor.preview_cache.contains(&img.id) || editor.pending_loads.contains(&img.id) {
                        continue;
                    }

                    if let Some(path) = &img.cache_path_working {
                        missing.push((img.id, path.clone()));
                    }
                }
            }
        }
    }
    missing
}

fn identify_missing_raw_preloads(editor: &RawEditor) -> Vec<(i64, String)> {
    let mut missing = Vec::new();
    if let Some(current_id) = editor.selected_image_id {
        if let Some(current_idx) = editor.images.iter().position(|i| i.id == current_id) {
            let total = editor.images.len() as isize;
            let behind = editor.raw_preload_behind as isize;
            let ahead = editor.raw_preload_ahead as isize;

            for offset in -behind..=ahead {
                if offset == 0 {
                    continue;
                }

                let mut target_idx = current_idx as isize + offset;
                if target_idx < 0 {
                    target_idx += total;
                }
                if target_idx >= total {
                    target_idx -= total;
                }

                let target_idx = target_idx as usize;
                if target_idx < editor.images.len() {
                    let img = &editor.images[target_idx];
                    if editor.raw_cache.contains(&img.id) || editor.pending_raw_loads.contains(&img.id)
                    {
                        continue;
                    }
                    missing.push((img.id, img.path.clone()));
                }
            }
        }
    }
    missing
}

async fn load_preview_pixels(path: String) -> Result<(u32, u32, Vec<u8>), String> {
    tokio::task::spawn_blocking(move || {
        let img = image::open(&path).map_err(|e| e.to_string())?;
        let rgba = img.to_rgba8();
        Ok((rgba.width(), rgba.height(), rgba.into_raw()))
    }).await.map_err(|e| e.to_string())?
}

async fn load_image_handle(
    id: i64,
    path: String,
) -> (i64, iced::widget::image::Handle, Option<std::sync::Arc<[u8]>>, (u32, u32)) {
    match load_preview_pixels(path.clone()).await {
        Ok((w, h, pixels)) => {
            let buf: std::sync::Arc<[u8]> = pixels.clone().into();
            (
                id,
                iced::widget::image::Handle::from_rgba(w, h, pixels),
                Some(buf),
                (w, h),
            )
        }
        Err(_) => (id, iced::widget::image::Handle::from_path(path), None, (1280, 853)),
    }
}
