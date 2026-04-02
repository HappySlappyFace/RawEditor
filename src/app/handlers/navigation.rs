use iced::{Task, Point};
use iced::widget::image::Handle;
use crate::app::state::{RawEditor, EditorReadiness, DragMode};
use crate::app::message::{Message, AppTab};
use crate::raw;
use crate::ui::preview_renderer::CropHandle;

pub fn handle_image_selected(editor: &mut RawEditor, image_id: i64) -> Task<Message> {
    if editor.last_modifiers.command() {
        if !editor.multi_selection.remove(&image_id) { editor.multi_selection.insert(image_id); }
    } else {
        editor.multi_selection.clear();
        editor.multi_selection.insert(image_id);
    }
    editor.selected_image_id = Some(image_id);
    editor.canvas_cache.clear();
    
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
                if let Some(img) = editor.images.iter().find(|i| i.id == image_id) {
                    editor.editor_readiness = EditorReadiness::Loading(image_id);
                    return Task::perform(raw::loader::load_raw_data(img.path.clone()), Message::RawDataLoaded);
                }
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
        let iw = resources.preview_width as f32;
        let ih = resources.preview_height as f32;
        let (vw, vh) = editor.viewport_size;
        let xo = (vw - iw) / 2.0;
        let yo = (vh - ih) / 2.0;
        let icx = (p.x - xo).clamp(0.0, iw);
        let icy = (p.y - yo).clamp(0.0, ih);
        
        let new_zoom = if d > 0.0 { old_zoom * (1.0 + d * 0.8) } else { old_zoom / (1.0 + (-d * 0.8)) }.clamp(0.1, 10.0);
        editor.zoom = new_zoom;
        
        let nx = icx / iw;
        let ny = icy / ih;
        let tx = ((nx - 0.5) / old_zoom - editor.pan_offset.x) + 0.5;
        let ty = ((ny - 0.5) / old_zoom - editor.pan_offset.y) + 0.5;
        editor.pan_offset.x = (nx - 0.5) / editor.zoom - tx + 0.5;
        editor.pan_offset.y = (ny - 0.5) / editor.zoom - ty + 0.5;
    } else {
        editor.zoom = if d > 0.0 { editor.zoom * (1.0 + d * 0.8) } else { editor.zoom / (1.0 + (-d * 0.8)) }.clamp(0.1, 10.0);
    }
    editor.canvas_cache.clear();
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
    if !editor.is_cropping { editor.is_dragging = true; editor.drag_mode = DragMode::Pan; }
    Task::none()
}

pub fn handle_mouse_released(editor: &mut RawEditor) -> Task<Message> {
    if editor.is_dragging {
        if let DragMode::CropHandle(_) = editor.drag_mode {
            editor.save_current_edits();
            editor.commit_current_state();
            if let (Some(ctx), Some(res)) = (&editor.gpu_context, &editor.image_resources) { res.update_uniforms(ctx, &editor.current_edit_params); }
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
             // Phase 116: Normalized delta based on 1000px virtual width for consistent feel.
             let sx = 1.0 / 1000.0;
             editor.last_cursor_position = Some(pos);
             return Task::done(Message::Pan(cgmath::Vector2::new(delta.x * sx, delta.y * sx)));
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
    let delta = pos - last;
    let (bw, bh) = editor.viewport_size;
    let dx = delta.x / bw;
    let dy = delta.y / bh;
    let c = editor.current_edit_params.crop;
    let (mut l, mut t, mut r, mut b) = (c[0], c[1], c[0]+c[2], c[1]+c[3]);
    
    match h {
        CropHandle::TopLeft => { l += dx; t += dy; }
        CropHandle::TopRight => { t += dy; r += dx; }
        CropHandle::BottomLeft => { l += dx; b += dy; }
        CropHandle::BottomRight => { r += dx; b += dy; }
        #[allow(clippy::possible_missing_else)]
        CropHandle::Body => { l += dx; t += dy; r += dx; b += dy; if l < 0.0 { r -= l; l = 0.0; } if r > 1.0 { l -= r - 1.0; r = 1.0; } if t < 0.0 { b -= t; t = 0.0; } if b > 1.0 { t -= b - 1.0; b = 1.0; } }
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
    let w = (r - l).max(0.0);
    let h = (b - t).max(0.0);
    editor.current_edit_params.crop = [l, t, w, h];
    if let (Some(ctx), Some(res)) = (&editor.gpu_context, &editor.image_resources) { res.update_uniforms(ctx, &editor.current_edit_params); }
}

fn trigger_image_load(editor: &mut RawEditor, image_id: i64) -> Task<Message> {
    editor.working_preview = None;
    if let Some(img) = editor.images.iter().find(|i| i.id == image_id) {
        // Phase 73: Check RAM cache first
        if let Some(handle) = editor.preview_cache.get(&image_id) {
            editor.working_preview = Some(handle.clone());
        } else if let Some(path) = &img.cache_path_working {
            editor.working_preview = Some(Handle::from_path(path.clone()));
        }
        
        editor.editor_readiness = EditorReadiness::Loading(image_id);
        let mut tasks = Vec::new();
        if let Some(path) = &img.cache_path_working {
            tasks.push(Task::perform(
                load_image_handle(image_id, path.clone()),
                |(id, h, p, d)| Message::WorkingPreviewReady(id, h, p, d),
            ));
        }
        
        // Only load full RAW data if we are in Develop mode
        if editor.current_tab == AppTab::Develop {
            tasks.push(Task::perform(raw::loader::load_raw_data(img.path.clone()), Message::RawDataLoaded));
        }
        
        // Schedule preloads for adjacent images
        tasks.push(schedule_preloads(editor));
        
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

fn identify_missing_preloads(editor: &RawEditor) -> Vec<(i64, String)> {
    let mut missing = Vec::new();
    if let Some(current_id) = editor.selected_image_id {
        if let Some(current_idx) = editor.images.iter().position(|i| i.id == current_id) {
            let total = editor.images.len() as isize;
            let behind = crate::app::state::PRELOAD_BEHIND as isize;
            let ahead = crate::app::state::PRELOAD_AHEAD as isize;

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
