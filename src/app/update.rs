use std::path::PathBuf;
use std::sync::Arc;
use iced::{Task, Point};
use iced::widget::image::Handle;
use rusqlite::{Connection, OptionalExtension};
use walkdir::WalkDir;
use chrono::Utc;
use rfd::FileDialog;
use iced_wgpu::wgpu;

use crate::state;
use crate::gpu;
use crate::raw;
use crate::ui;
use crate::app::message::{Message, AppTab, ImportResult};
use crate::app::state::{RawEditor, EditorStatus, DragMode};
use crate::ui::preview_renderer::CropHandle;

pub fn update(editor: &mut RawEditor, message: Message) -> Task<Message> {
    match message {
        Message::DatabaseLoaded(result) => {
            match result {
                Ok(images) => {
                    match state::library::Library::new() {
                        Ok(library) => {
                            let image_count = images.len();
                            editor.library = Some(library);
                            editor.images = images;
                            editor.status = format!("Loaded {} images.", image_count);
                            
                            use iced::window;
                            let maximize_window = window::get_latest().and_then(|id| window::maximize(id, true));

                            if let Some(lib) = &editor.library {
                                let db_path = lib.path().clone();
                                return Task::batch(vec![
                                    maximize_window,
                                    Task::perform(process_cache_async(db_path), Message::CacheProcessed),
                                ]);
                            }
                            return maximize_window;
                        }
                        Err(e) => editor.status = format!("Failed to create library: {:?}", e),
                    }
                }
                Err(e) => editor.status = format!("Failed to load database: {}", e),
            }
            Task::none()
        }
        Message::ImportFolder => {
            if let Some(library) = &editor.library {
                let folder = FileDialog::new().set_title("Select Folder").pick_folder();
                if let Some(folder_path) = folder {
                    editor.status = format!("Importing from {}...", folder_path.display());
                    let db_path = library.path().clone();
                    return Task::perform(import_folder_async(folder_path, db_path), Message::ImportComplete);
                }
            }
            Task::none()
        }
        Message::ImportComplete(result) => {
            if let Some(library) = &editor.library {
                editor.images = library.get_all_images().unwrap_or_default();
                editor.status = format!("Import complete! Added {}, skipped {}.", result.imported_count, result.skipped_count);
                let db_path = library.path().clone();
                return Task::perform(process_cache_async(db_path), Message::CacheProcessed);
            }
            Task::none()
        }
        Message::ThumbnailGenerated(_) => Task::none(),
        Message::CacheProcessed(result) => {
            if let Some(library) = &editor.library {
                match result {
                    Ok((image_id, thumb, instant, working)) => {
                        let _ = library.set_image_cache_paths(image_id, &thumb, &instant, &working);
                    },
                    Err((image_id, _)) if image_id != 0 => {
                        let _ = library.conn().execute("UPDATE images SET cache_status = 'failed' WHERE id = ?1", [image_id]);
                    },
                    _ => {}
                }
                editor.images = library.get_all_images().unwrap_or_default();
                
                let pending_count: i64 = library.conn().query_row("SELECT COUNT(*) FROM images WHERE cache_status = 'pending'", [], |row| row.get(0)).unwrap_or(0);
                
                if pending_count > 0 {
                    let total = editor.images.len() as i64;
                    editor.status = format!("Caching: {}/{} - {} remaining", total - pending_count, total, pending_count);
                    let db_path = library.path().clone();
                    return Task::perform(process_cache_async(db_path), Message::CacheProcessed);
                } else {
                    editor.status = format!("{} All cache tiers generated!", ui::icons::CHECK);
                }
            }
            Task::none()
        }
        Message::ImageSelected(image_id) => {
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
                editor.history_map.entry(image_id).or_insert_with(|| (vec![editor.current_edit_params.clone()], 0));
            }
            
            if editor.current_tab == AppTab::Develop || editor.current_tab == AppTab::Cull {
                let needs_load = match &editor.editor_status {
                    EditorStatus::Ready(p) => p.image_id != image_id,
                    EditorStatus::Loading(id) => *id != image_id,
                    _ => true,
                };
                
                if needs_load {
                    editor.working_preview = None;
                    if let Some(img) = editor.images.iter().find(|i| i.id == image_id) {
                        // Phase 73: Check RAM cache first
                        if let Some(handle) = editor.preview_cache.get(&image_id) {
                            editor.working_preview = Some(handle.clone());
                        } else if let Some(path) = &img.cache_path_working {
                            editor.working_preview = Some(Handle::from_path(path.clone()));
                        }
                        
                        editor.editor_status = EditorStatus::Loading(image_id);
                        let mut tasks = Vec::new();
                        if let Some(path) = &img.cache_path_working {
                            tasks.push(Task::perform(load_image_handle(image_id, path.clone()), |(id, h)| Message::WorkingPreviewReady(id, h)));
                        }
                        
                        // Only load full RAW data if we are in Develop mode
                        if editor.current_tab == AppTab::Develop {
                            tasks.push(Task::perform(raw::loader::load_raw_data(img.path.clone()), Message::RawDataLoaded));
                        }
                        
                        // Schedule preloads for adjacent images
                        tasks.push(schedule_preloads(editor));
                        
                        return Task::batch(tasks);
                    }
                }
            }
            Task::none()
        }
        Message::PreviewGenerated(_) => Task::none(),
        Message::TabChanged(tab) => {
            editor.current_tab = tab;
            if tab == AppTab::Develop {
                if let Some(image_id) = editor.selected_image_id {
                    let needs_load = match &editor.editor_status {
                        EditorStatus::Ready(p) => p.image_id != image_id,
                        EditorStatus::Loading(id) => *id != image_id,
                        _ => true,
                    };
                    if needs_load {
                        if let Some(img) = editor.images.iter().find(|i| i.id == image_id) {
                            editor.editor_status = EditorStatus::Loading(image_id);
                            return Task::perform(raw::loader::load_raw_data(img.path.clone()), Message::RawDataLoaded);
                        }
                    }
                }
            }
            Task::none()
        }
        Message::ExposureChanged(v) => { editor.current_edit_params.exposure = v; update_pipeline(editor); Task::none() }
        Message::ContrastChanged(v) => { editor.current_edit_params.contrast = v; update_pipeline(editor); Task::none() }
        Message::HighlightsChanged(v) => { editor.current_edit_params.highlights = v; update_pipeline(editor); Task::none() }
        Message::ShadowsChanged(v) => { editor.current_edit_params.shadows = v; update_pipeline(editor); Task::none() }
        Message::WhitesChanged(v) => { editor.current_edit_params.whites = v; update_pipeline(editor); Task::none() }
        Message::BlacksChanged(v) => { editor.current_edit_params.blacks = v; update_pipeline(editor); Task::none() }
        Message::BlackOffsetChanged(i, v) => { if i < 4 { editor.current_edit_params.black_offsets[i] = v; update_pipeline(editor); } Task::none() }
        Message::BlackPhaseChanged(y, v) => { if y { editor.current_edit_params.black_phase_y = v; } else { editor.current_edit_params.black_phase_x = v; } update_pipeline(editor); Task::none() }
        Message::VibranceChanged(v) => { editor.current_edit_params.vibrance = v; update_pipeline(editor); Task::none() }
        Message::SaturationChanged(v) => { editor.current_edit_params.saturation = v; update_pipeline(editor); Task::none() }
        Message::TemperatureChanged(v) => { editor.current_edit_params.temperature = v; update_pipeline(editor); Task::none() }
        Message::TintChanged(v) => { editor.current_edit_params.tint = v; update_pipeline(editor); Task::none() }
        Message::NoiseReductionChanged(v) => { editor.current_edit_params.noise_reduction = v; update_pipeline(editor); Task::none() }
        Message::SharpeningChanged(v) => { editor.current_edit_params.sharpening = v; update_pipeline(editor); Task::none() }
        Message::SharpenMaskingChanged(v) => { editor.current_edit_params.sharpen_masking = v; update_pipeline(editor); Task::none() }
        Message::RotationChanged(v) => { editor.current_edit_params.rotation = v; update_pipeline(editor); Task::none() }
        Message::SetCrop(crop) => {
            editor.current_edit_params.crop = crop;
            update_pipeline(editor);
            editor.commit_current_state();
            Task::none()
        }
        Message::ToggleCropMode => {
            editor.is_cropping = !editor.is_cropping;
            editor.drag_mode = DragMode::None;
            editor.canvas_cache.clear();
            Task::none()
        }
        Message::CropHandleGrabbed(handle, bounds) => {
            editor.drag_mode = DragMode::CropHandle(handle);
            editor.is_dragging = true;
            editor.viewport_size = (bounds.width, bounds.height);
            Task::none()
        }
        Message::CommitEdit => { editor.commit_current_state(); Task::none() }
        Message::Undo => {
            if let Some((stack, index)) = editor.get_current_history() {
                if *index > 0 {
                    *index -= 1;
                    editor.current_edit_params = stack[*index].clone();
                    update_pipeline(editor);
                }
            }
            Task::none()
        }
        Message::Redo => {
            if let Some((stack, index)) = editor.get_current_history() {
                if *index < stack.len() - 1 {
                    *index += 1;
                    editor.current_edit_params = stack[*index].clone();
                    update_pipeline(editor);
                }
            }
            Task::none()
        }
        Message::CopyEdits => { editor.edit_clipboard = Some(editor.current_edit_params.clone()); Task::none() }
        Message::PasteEdits => {
            if let Some(cb) = &editor.edit_clipboard {
                editor.current_edit_params = cb.clone();
                update_pipeline(editor);
                editor.commit_current_state();
            }
            Task::none()
        }
        Message::ResetEdits => {
            editor.current_edit_params.reset();
            if let Some(lib) = &editor.library { if let Some(id) = editor.selected_image_id { let _ = lib.delete_edits(id); } }
            update_pipeline(editor);
            editor.commit_current_state();
            Task::none()
        }
        Message::CopySettings => { editor.edit_clipboard = Some(editor.current_edit_params); editor.status = "Settings copied!".to_string(); Task::none() }
        Message::PasteSettings => {
            if let Some(cb) = editor.edit_clipboard {
                editor.current_edit_params = cb;
                if let Some(lib) = &editor.library {
                    for &id in &editor.multi_selection {
                        if Some(id) != editor.selected_image_id { let _ = lib.save_edit_params(id, &editor.current_edit_params); }
                    }
                    editor.status = "Settings pasted to selection".to_string();
                }
                update_pipeline(editor);
                editor.commit_current_state();
            }
            Task::none()
        }
        Message::ModifiersChanged(m) => { editor.last_modifiers = m; Task::none() }
        Message::SetMinRating(r) => { editor.min_filter_rating = r; Task::none() }
        Message::ToggleInfoHud => { editor.info_overlay = editor.info_overlay.next(); Task::none() }
        Message::SetRating(r) => {
            let ids: Vec<i64> = if !editor.multi_selection.is_empty() { editor.multi_selection.iter().copied().collect() } else if let Some(id) = editor.selected_image_id { vec![id] } else { vec![] };
            if let Some(lib) = &editor.library {
                for &id in &ids {
                    let _ = lib.set_image_rating(id, r);
                    if let Some(img) = editor.images.iter_mut().find(|i| i.id == id) { img.rating = r; }
                }
            }
            editor.status = format!("Rated {} image(s)", ids.len());
            Task::none()
        }
        Message::ToggleBeforeAfter => { editor.show_before = !editor.show_before; editor.histogram_cache.clear(); Task::none() }
        Message::SelectNextImage => {
            if let Some(id) = editor.selected_image_id {
                if let Some(idx) = editor.images.iter().position(|i| i.id == id) {
                    let next = editor.images[(idx + 1) % editor.images.len()].id;
                    return update(editor, Message::ImageSelected(next));
                }
            }
            Task::none()
        }
        Message::SelectPreviousImage => {
            if let Some(id) = editor.selected_image_id {
                if let Some(idx) = editor.images.iter().position(|i| i.id == id) {
                    let prev = editor.images[if idx == 0 { editor.images.len() - 1 } else { idx - 1 }].id;
                    return update(editor, Message::ImageSelected(prev));
                }
            }
            Task::none()
        }
        Message::Zoom(d, mut p) => {
            if editor.is_cropping { return Task::none(); }
            if p.x < 0.0 { p = editor.last_cursor_position.unwrap_or(Point::ORIGIN); }
            
            if let EditorStatus::Ready(pipe) = &editor.editor_status {
                let old_zoom = editor.zoom;
                let iw = pipe.preview_width as f32;
                let ih = pipe.preview_height as f32;
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
        Message::ResetView => { editor.zoom = 1.0; editor.pan_offset = cgmath::Vector2::new(0.0, 0.0); editor.canvas_cache.clear(); Task::none() }
        Message::Pan(d) => {
            if editor.is_cropping { return Task::none(); }
            let s = 1.0 / editor.zoom;
            editor.pan_offset.x += d.x * s;
            editor.pan_offset.y += d.y * s;
            editor.canvas_cache.clear();
            Task::none()
        }
        Message::MousePressed => {
            let now = std::time::Instant::now();
            let double = editor.last_click_time.map(|t| now.duration_since(t).as_millis() < 300).unwrap_or(false);
            editor.last_click_time = Some(now);
            if double { return update(editor, Message::ResetView); }
            if !editor.is_cropping { editor.is_dragging = true; editor.drag_mode = DragMode::Pan; }
            Task::none()
        }
        Message::MouseReleased => {
            if editor.is_dragging {
                if let DragMode::CropHandle(_) = editor.drag_mode {
                    editor.save_current_edits();
                    editor.commit_current_state();
                    if let EditorStatus::Ready(p) = &editor.editor_status { p.update_uniforms(&editor.current_edit_params); }
                }
            }
            editor.is_dragging = false;
            editor.drag_mode = DragMode::None;
            editor.last_cursor_position = None;
            Task::none()
        }
        Message::MouseMoved(pos) => {
            let nw = (pos.x * 1.01).max(editor.viewport_size.0);
            let nh = (pos.y * 1.01).max(editor.viewport_size.1);
            if (nw - editor.viewport_size.0).abs() > 10.0 { editor.viewport_size.0 = nw; }
            if (nh - editor.viewport_size.1).abs() > 10.0 { editor.viewport_size.1 = nh; }
            
            if editor.is_dragging {
                if let Some(last) = editor.last_cursor_position {
                    let delta = pos - last;
                    match editor.drag_mode {
                        DragMode::Pan => {
                            let (sx, sy) = if let EditorStatus::Ready(p) = &editor.editor_status { (1.0/p.preview_width as f32, 1.0/p.preview_height as f32) } else { (0.001, 0.001) };
                            editor.last_cursor_position = Some(pos);
                            return update(editor, Message::Pan(cgmath::Vector2::new(delta.x * sx, delta.y * sy)));
                        }
                        DragMode::CropHandle(h) => {
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
                            
                            editor.current_edit_params.crop = [l, t, r - l, b - t];
                            if let EditorStatus::Ready(p) = &mut editor.editor_status { p.update_uniforms(&editor.current_edit_params); }
                            editor.last_cursor_position = Some(pos);
                            editor.canvas_cache.clear();
                        }
                        _ => {}
                    }
                }
            } else {
                editor.last_cursor_position = Some(pos);
            }
            Task::none()
        }
        Message::RawDataLoaded(res) => {
            match res {
                Ok(raw) => {
                    editor.current_metadata = Some(raw.clone());
                    let image_id = editor.selected_image_id.unwrap_or(0);
                    let params = editor.current_edit_params.clone();
                    
                    // Phase 15: Calculate proper cam-to-sRGB color matrix
                    let xyz_to_cam = raw.color_matrix;
                    let cam_to_srgb = crate::color::calculate_cam_to_srgb(xyz_to_cam);

                    return Task::perform(
                        async move {
                            gpu::RenderPipeline::new(
                                image_id,
                                raw.data,
                                raw.width,
                                raw.height,
                                &params,
                                raw.wb_multipliers,
                                cam_to_srgb,
                                raw.cfa_pattern,
                                raw.black_levels,
                                raw.white_level
                            ).await.map(Arc::new)
                        },
                        Message::GpuPipelineReady
                    );
                }
                Err(e) => { editor.status = format!("Failed to load RAW: {}", e); editor.editor_status = EditorStatus::Failed(0, e); }
            }
            Task::none()
        }
        Message::GpuPipelineReady(res) => {
            match res {
                Ok(p) => {
                    p.update_uniforms(&editor.current_edit_params);
                    editor.editor_status = EditorStatus::Ready(p);
                    editor.working_preview = None;
                    editor.canvas_cache.clear();
                    editor.histogram_cache.clear();
                }
                Err(e) => { editor.status = format!("GPU Init Failed: {}", e); editor.editor_status = EditorStatus::Failed(0, e); }
            }
            Task::none()
        }
        Message::ExportImage => {
            if let EditorStatus::Ready(p) = &editor.editor_status {
                if let Some(id) = editor.selected_image_id {
                    let fname = editor.images.iter().find(|i| i.id == id).map(|i| i.filename.clone()).unwrap_or("export.jpg".to_string());
                    if let Some(path) = FileDialog::new().set_file_name(&format!("edited_{}", fname)).save_file() {
                        editor.status = "Exporting...".to_string();
                        return Task::perform(export_image_async(p.clone(), path, editor.current_edit_params.crop), Message::ExportComplete);
                    }
                }
            }
            Task::none()
        }
        Message::ExportComplete(res) => {
            match res {
                Ok(p) => editor.status = format!("Export saved to {}", p.display()),
                Err(e) => editor.status = format!("Export failed: {}", e),
            }
            Task::none()
        }
        Message::MinimizeWindow => { use iced::window; window::get_latest().and_then(|id| window::minimize(id, true)) }
        Message::MaximizeWindow => { use iced::window; window::get_latest().and_then(|id| window::maximize(id, true)) }
        Message::CloseWindow => { use iced::window; window::get_latest().and_then(window::close) }
        Message::DragWindow => { use iced::window; window::get_latest().and_then(window::drag) }
        Message::HistogramToggled(e) => { editor.histogram_enabled = e; Task::none() }
        Message::WorkingPreviewReady(id, h) => {
            if Some(id) == editor.selected_image_id {
                editor.working_preview = Some(h);
            }
            Task::none()
        }
        Message::PreloadPreview(_) => Task::none(), // Handled by schedule_preloads batching
        Message::PreviewCached(id, res) => {
            // Phase 78: Cleanup pending load
            editor.pending_loads.remove(&id);
            
            if let Ok((width, height, pixels)) = res {
                let handle = iced::widget::image::Handle::from_rgba(width, height, pixels);
                editor.preview_cache.put(id, handle);
            }
            Task::none()
        }
    }
}

fn update_pipeline(editor: &mut RawEditor) {
    editor.save_current_edits();
    if let EditorStatus::Ready(p) = &editor.editor_status {
        p.update_uniforms(&editor.current_edit_params);
        editor.canvas_cache.clear();
    }
}

async fn import_folder_async(folder_path: PathBuf, db_path: PathBuf) -> ImportResult {
    tokio::task::spawn_blocking(move || {
        let mut imported_count = 0;
        let mut skipped_count = 0;
        let conn = Connection::open(&db_path).expect("Failed to open database");
        for entry in WalkDir::new(&folder_path).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if ["nef", "cr2", "arw", "dng", "orf", "rw2"].contains(&ext_lower.as_str()) {
                        let path_str = path.to_string_lossy().to_string();
                        let filename = path.file_name().unwrap().to_string_lossy().to_string();
                        let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM images WHERE path = ?1)", [&path_str], |row| row.get(0)).unwrap_or(false);
                        if !exists {
                            let now = Utc::now().to_rfc3339();
                            conn.execute("INSERT INTO images (path, filename, date_added, cache_status) VALUES (?1, ?2, ?3, 'pending')", (&path_str, &filename, &now)).expect("Failed to insert image");
                            imported_count += 1;
                        } else { skipped_count += 1; }
                    }
                }
            }
        }
        ImportResult { imported_count, skipped_count }
    }).await.expect("Import task failed")
}

async fn process_cache_async(db_path: PathBuf) -> Result<(i64, String, String, String), (i64, String)> {
    let conn = Connection::open(&db_path).map_err(|e| (0, format!("DB Error: {}", e)))?;
    let pending: Option<(i64, String)> = conn.query_row("SELECT id, path FROM images WHERE cache_status = 'pending' LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(|e| (0, format!("Query Error: {}", e)))?;
    
    if let Some((id, path)) = pending {
        let _ = conn.execute("UPDATE images SET cache_status = 'processing' WHERE id = ?1", [id]);
        drop(conn);
        let result = tokio::task::spawn_blocking(move || {
            let cache_dir = raw::preview::get_preview_cache_dir();
            raw::processor::process_image(std::path::Path::new(&path), id, &cache_dir)
        }).await.map_err(|e| (id, format!("Join Error: {}", e)))?;
        match result { Ok(res) => Ok((id, res.0, res.1, res.2)), Err(e) => Err((id, e)) }
    } else { Err((0, "No pending images".to_string())) }
}

async fn load_preview_pixels(path: String) -> Result<(u32, u32, Vec<u8>), String> {
    tokio::task::spawn_blocking(move || {
        let img = image::open(&path).map_err(|e| e.to_string())?;
        let rgba = img.to_rgba8();
        Ok((rgba.width(), rgba.height(), rgba.into_raw()))
    }).await.map_err(|e| e.to_string())?
}

async fn load_image_handle(id: i64, path: String) -> (i64, iced::widget::image::Handle) {
    match load_preview_pixels(path.clone()).await {
        Ok((w, h, pixels)) => (id, iced::widget::image::Handle::from_rgba(w, h, pixels)),
        Err(_) => (id, iced::widget::image::Handle::from_path(path)),
    }
}

async fn export_image_async(pipeline: Arc<gpu::RenderPipeline>, save_path: std::path::PathBuf, crop: [f32; 4]) -> Result<std::path::PathBuf, String> {
    tokio::task::spawn_blocking(move || {
        let rgba_bytes = pipeline.render_full_res_to_bytes(wgpu::TextureFormat::Rgba8Unorm, crop);
        let result = {
            let width = (pipeline.width as f32 * crop[2]) as u32;
            let height = (pipeline.height as f32 * crop[3]) as u32;
            let bytes = rgba_bytes.map_err(|e| format!("Export failed: {}", e))?;
            image::save_buffer(&save_path, &bytes, width, height, image::ColorType::Rgba8)
        };
        result.map(|_| save_path.clone()).map_err(|e| format!("Save failed: {}", e))
    }).await.map_err(|e| format!("Task failed: {}", e))?
}

// Phase 73: The Look-Ahead Cache
// Identify adjacent images (current - 2 to current + 10) and preload their working previews
fn schedule_preloads(editor: &mut RawEditor) -> Task<Message> {
    if let Some(current_id) = editor.selected_image_id {
        if let Some(current_idx) = editor.images.iter().position(|i| i.id == current_id) {
            let total = editor.images.len() as isize;
            let mut tasks = Vec::new();
            
            // Phase 80: Configurable Cache Size
            let behind = crate::app::state::PRELOAD_BEHIND as isize;
            let ahead = crate::app::state::PRELOAD_AHEAD as isize;

            // Range: -BEHIND to +AHEAD
            for offset in -behind..=ahead {
                if offset == 0 { continue; } // Skip current image (already handled)
                
                let mut target_idx = current_idx as isize + offset;
                
                // Handle wrapping
                if target_idx < 0 { target_idx += total; }
                if target_idx >= total { target_idx -= total; }
                
                let target_idx = target_idx as usize;
                if target_idx < editor.images.len() {
                    let img = &editor.images[target_idx];
                    
                    // Phase 78: Async Task Deduplication
                    // Check if already cached OR already loading
                    if editor.preview_cache.contains(&img.id) || editor.pending_loads.contains(&img.id) {
                        continue;
                    }

                    // Not in RAM cache and not loading, check if we have a working preview on disk
                    if let Some(path) = &img.cache_path_working {
                        let path_clone = path.clone();
                        let id = img.id;
                        
                        // Mark as loading
                        editor.pending_loads.insert(id);
                        
                        // Spawn load task
                        tasks.push(Task::perform(
                            load_preview_pixels(path_clone),
                            move |res| Message::PreviewCached(id, res)
                        ));
                    }
                }
            }
            
            if !tasks.is_empty() {
                return Task::batch(tasks);
            }
        }
    }
    Task::none()
}
