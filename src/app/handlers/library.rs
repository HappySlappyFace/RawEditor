use iced::Task;
use rusqlite::{Connection, OptionalExtension};
use std::path::PathBuf;
use rfd::FileDialog;
use walkdir::WalkDir;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app::state::RawEditor;
use crate::app::message::{Message, ImportResult};
use crate::database;
use crate::raw;
use crate::ui;

pub fn handle_database_loaded(editor: &mut RawEditor, result: Result<Vec<crate::database::models::Image>, String>) -> Task<Message> {
    match result {
        Ok(images) => {
            match database::library::Library::new() {
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

pub fn handle_import_folder(editor: &mut RawEditor) -> Task<Message> {
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

pub fn handle_import_from_camera(editor: &mut RawEditor) -> Task<Message> {
    editor.status = "Camera import is not implemented yet.".to_string();
    Task::none()
}

pub fn handle_import_complete(editor: &mut RawEditor, result: ImportResult) -> Task<Message> {
    if let Some(library) = &editor.library {
        editor.images = library.get_all_images().unwrap_or_default();
        editor.status = format!("Import complete! Added {}, skipped {}.", result.imported_count, result.skipped_count);
        let db_path = library.path().clone();
        return Task::perform(process_cache_async(db_path), Message::CacheProcessed);
    }
    Task::none()
}

pub fn handle_cache_processed(editor: &mut RawEditor, result: Result<(i64, String, String, String), (i64, String)>) -> Task<Message> {
    if let Some(library) = &editor.library {
        match result {
            Ok((image_id, thumb, instant, working)) => {
                let _ = library.set_image_cache_paths(image_id, &thumb, &instant, &working);
            },
            Err((image_id, ref err)) if image_id != 0 => {
                if let Err(db_err) = library.conn().execute("UPDATE images SET cache_status = 'failed' WHERE id = ?1", [image_id]) {
                    tracing::error!("Failed to mark image {} as failed: {}", image_id, db_err);
                }
                tracing::warn!("Cache processing failed for image {}: {}", image_id, err);
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

pub fn handle_set_rating(editor: &mut RawEditor, rating: u8) -> Task<Message> {
    let ids: Vec<i64> = if !editor.multi_selection.is_empty() { 
        editor.multi_selection.iter().copied().collect() 
    } else if let Some(id) = editor.selected_image_id { 
        vec![id] 
    } else { 
        vec![] 
    };
    
    if let Some(lib) = &editor.library {
        for &id in &ids {
            let _ = lib.set_image_rating(id, rating);
            if let Some(img) = editor.images.iter_mut().find(|i| i.id == id) { img.rating = rating; }
        }
    }
    editor.status = format!("Rated {} image(s)", ids.len());
    if editor.auto_advance {
        return Task::done(Message::SelectNextImage);
    }
    Task::none()
}

pub fn handle_set_flag(editor: &mut RawEditor, flag: i8) -> Task<Message> {
    let ids: Vec<i64> = if !editor.multi_selection.is_empty() { 
        editor.multi_selection.iter().copied().collect() 
    } else if let Some(id) = editor.selected_image_id { 
        vec![id] 
    } else { 
        vec![] 
    };
    
    if let Some(lib) = &editor.library {
        for &id in &ids {
            let _ = lib.set_image_flag(id, flag);
            if let Some(img) = editor.images.iter_mut().find(|i| i.id == id) { img.flag = flag; }
        }
    }
    editor.status = format!("Flagged {} image(s)", ids.len());
    if editor.auto_advance {
        return Task::done(Message::SelectNextImage);
    }
    Task::none()
}

pub fn handle_toggle_auto_advance(editor: &mut RawEditor) -> Task<Message> {
    editor.auto_advance = !editor.auto_advance;
    editor.save_preferences();
    Task::none()
}

pub fn handle_set_library_folder_filter(
    editor: &mut RawEditor,
    folder_filter: Option<String>,
) -> Task<Message> {
    editor.library_folder_filter = folder_filter;
    Task::none()
}

// Helpers

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
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as i64;
                            conn.execute("INSERT INTO images (path, filename, imported_at, cache_status) VALUES (?1, ?2, ?3, 'pending')", (&path_str, &filename, &now)).expect("Failed to insert image");
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
