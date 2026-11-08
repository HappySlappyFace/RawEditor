use iced::Task;
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
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

                    let scale = crate::app::handlers::window::query_scale_factor();
                    // The first-run question, or the check the user agreed to.
                    let updates = crate::app::handlers::updates::on_startup(editor);
                    if let Some(lib) = &editor.library {
                        let db_path = lib.path().clone();
                        editor.pending_cache_count = lib.count_pending_cache().unwrap_or(0);
                        return Task::batch(vec![
                            maximize_window,
                            scale,
                            spawn_cache_workers(db_path),
                            updates,
                        ]);
                    }
                    return Task::batch(vec![maximize_window, scale, updates]);
                }
                Err(e) => editor.status = format!("Failed to create library: {:?}", e),
            }
        }
        Err(e) => editor.status = format!("Failed to load database: {}", e),
    }
    Task::none()
}

pub fn handle_import_folder(editor: &mut RawEditor) -> Task<Message> {
    if editor.library.is_none() {
        return Task::none();
    }
    // AsyncFileDialog, not the blocking FileDialog: the blocking one runs the
    // native picker's event loop inside `update`, freezing the whole window
    // for as long as the dialog is open. The export path already does it this
    // way (handlers::export::handle_pick_export_base_path).
    Task::perform(
        async {
            rfd::AsyncFileDialog::new()
                .set_title("Select Folder")
                .pick_folder()
                .await
                .map(|handle| handle.path().to_path_buf())
        },
        Message::ImportFolderPicked,
    )
}

/// Landing point for the async folder picker. `None` means the user cancelled.
///
/// The scan is a separate step from the picker so the status bar can say an
/// import is running — a large card used to show nothing at all until done.
pub fn handle_import_folder_picked(
    editor: &mut RawEditor,
    folder: Option<PathBuf>,
) -> Task<Message> {
    let (Some(folder), Some(library)) = (folder, &editor.library) else {
        return Task::none();
    };
    editor.status = format!("Importing {}…", folder.display());
    let db_path = library.path().clone();
    Task::perform(import_folder_async(folder, db_path), Message::ImportComplete)
}

/// A file or folder dropped onto the window. A folder is offered for import;
/// a RAW file stands for the folder it is in. Anything else is ignored.
///
/// The stat and the folder walk happen off the UI thread. A RAW is
/// recognised by its extension alone, so the fifty events of a fifty-file
/// drop collapse into one scan of their folder here, without touching disk.
pub fn handle_file_dropped(editor: &mut RawEditor, path: PathBuf) -> Task<Message> {
    use crate::app::state::Modal;
    if editor.library.is_none() || !matches!(editor.active_modal, Modal::None | Modal::ImportDropped) {
        return Task::none();
    }
    let key = if raw::is_supported_raw(&path) {
        path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.clone())
    } else {
        path.clone()
    };
    let drop = &mut editor.drop_import;
    if drop.scanning.contains(&key) || drop.folders.iter().any(|(f, _)| *f == key) {
        return Task::none();
    }
    drop.scanning.insert(key.clone());
    Task::perform(
        async move {
            let scan = tokio::task::spawn_blocking({
                let key = key.clone();
                move || key.is_dir().then(|| { let files = collect_raw_files(&key); (key, files) })
            })
            .await
            .ok()
            .flatten();
            (key, scan)
        },
        |(dropped, scan)| Message::DropScanned { dropped, scan },
    )
}

/// A dropped folder's scan is back: offer its new photos, say there are
/// none, or (when it is all catalogued already) show it.
pub fn handle_drop_scanned(
    editor: &mut RawEditor,
    dropped: PathBuf,
    scan: Option<(PathBuf, Vec<PathBuf>)>,
) -> Task<Message> {
    use crate::app::state::Modal;
    // Not ours any more: the dialog was cancelled while this scan ran.
    if !editor.drop_import.scanning.remove(&dropped) {
        return Task::none();
    }
    let Some((folder, files)) = scan else {
        return Task::none();
    };
    let catalogued: std::collections::HashSet<&str> = editor.images.iter().map(|img| img.path.as_str()).collect();
    let new: Vec<PathBuf> = files
        .iter()
        .filter(|f| !catalogued.contains(f.to_string_lossy().as_ref()))
        .cloned()
        .collect();
    let offering = !editor.drop_import.folders.is_empty();

    if new.is_empty() {
        if offering {
            return Task::none();
        }
        if files.is_empty() {
            editor.status = format!("No RAW files in {}.", folder.display());
            return Task::none();
        }
        // All catalogued: show it (the folder filter is a path prefix, so
        // this works for a folder whose photos sit in subfolders too).
        editor.status = format!("{} is already in the library.", folder.display());
        let key = folder.to_string_lossy().into_owned();
        if editor.active_modal == Modal::None {
            let tab = crate::app::handlers::navigation::handle_tab_changed(editor, crate::app::message::AppTab::Library);
            let filter = handle_set_library_folder_filter(editor, Some(key));
            return Task::batch([tab, filter]);
        }
        return Task::none();
    }

    let drop = &mut editor.drop_import;
    let before = drop.files.len();
    drop.files.extend(new);
    let added = drop.files.len() - before;
    if added > 0 {
        drop.folders.push((folder, added));
        // Scans land in any order; list the folders by path.
        drop.folders.sort();
    }
    if editor.active_modal == Modal::None && !editor.drop_import.folders.is_empty() {
        return crate::app::handlers::window::handle_open_modal(editor, Modal::ImportDropped);
    }
    Task::none()
}

/// Import every folder the dropped-import dialog lists, in one background job.
pub fn handle_drop_import_confirmed(editor: &mut RawEditor) -> Task<Message> {
    use crate::app::state::Modal;
    if editor.active_modal != Modal::ImportDropped {
        return Task::none();
    }
    editor.active_modal = Modal::None;
    let drop = std::mem::take(&mut editor.drop_import);
    let Some(library) = &editor.library else {
        return Task::none();
    };
    let folders: Vec<PathBuf> = drop.folders.into_iter().map(|(f, _)| f).collect();
    if folders.is_empty() {
        return Task::none();
    }
    editor.status = match folders.as_slice() {
        [one] => format!("Importing {}…", one.display()),
        many => format!("Importing {} folders…", many.len()),
    };
    let db_path = library.path().clone();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let mut total = ImportResult { imported_count: 0, skipped_count: 0 };
                for folder in &folders {
                    let result = import_folder_blocking(folder, &db_path)?;
                    total.imported_count += result.imported_count;
                    total.skipped_count += result.skipped_count;
                }
                Ok(total)
            })
            .await
            .map_err(|e| format!("Import task failed: {e}"))?
        },
        Message::ImportComplete,
    )
}

pub fn handle_import_from_camera(editor: &mut RawEditor) -> Task<Message> {
    editor.status = "Camera import is not implemented yet.".to_string();
    Task::none()
}

pub fn handle_import_complete(
    editor: &mut RawEditor,
    result: Result<ImportResult, String>,
) -> Task<Message> {
    let result = match result {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Import failed: {e}");
            editor.status = format!("Import failed: {e}");
            return Task::none();
        }
    };
    if let Some(library) = &editor.library {
        // Full reload is correct here: the import added rows this process has
        // never seen. It runs once per import, not once per cached image.
        editor.images = library.get_all_images().unwrap_or_default();
        editor.pending_cache_count = library.count_pending_cache().unwrap_or(0);
        editor.status = format!("Import complete! Added {}, skipped {}.", result.imported_count, result.skipped_count);
        let db_path = library.path().clone();
        return spawn_cache_workers(db_path);
    }
    Task::none()
}

pub fn handle_cache_processed(editor: &mut RawEditor, result: Result<(i64, String, String, String), (i64, String)>) -> Task<Message> {
    let Some(library) = &editor.library else {
        return Task::none();
    };

    // `image_id == 0` on the Err path means the worker found nothing left to
    // claim. Nothing was consumed from the queue, so don't decrement, and let
    // this worker retire instead of respawning — the others are still running
    // and will drive the count to zero.
    let mut claimed_one = true;

    match result {
        Ok((image_id, thumb, instant, working)) => {
            let _ = library.set_image_cache_paths(image_id, &thumb, &instant, &working);
            apply_cache_paths(&mut editor.images, image_id, thumb, instant, working);
        }
        Err((image_id, ref err)) if image_id != 0 => {
            if let Err(db_err) = library.conn().execute("UPDATE images SET cache_status = 'failed' WHERE id = ?1", [image_id]) {
                tracing::error!("Failed to mark image {} as failed: {}", image_id, db_err);
            }
            tracing::warn!("Cache processing failed for image {}: {}", image_id, err);
        }
        _ => claimed_one = false,
    }

    if claimed_one {
        editor.pending_cache_count = (editor.pending_cache_count - 1).max(0);
    }

    if editor.pending_cache_count > 0 && claimed_one {
        let total = editor.images.len() as i64;
        let done = total - editor.pending_cache_count;
        editor.status = format!(
            "Caching: {}/{} - {} remaining",
            done, total, editor.pending_cache_count
        );
        let db_path = library.path().clone();
        // One replacement worker per completion keeps concurrency steady
        return Task::perform(process_cache_async(db_path), Message::CacheProcessed);
    }

    if editor.pending_cache_count == 0 {
        editor.status = format!("{} All cache tiers generated!", ui::icons::CHECK);
    }
    Task::none()
}

pub fn handle_set_rating(editor: &mut RawEditor, rating: u8) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    let ids = editor.action_target_ids();
    let Some(lib) = &editor.library else {
        return Task::none();
    };
    // Memory follows the database: only touch the in-memory rows once the
    // write has landed, so a failed write can't show ratings that a restart
    // would silently take back.
    if let Err(e) = lib.set_ratings(&ids, rating) {
        tracing::error!("Failed to rate {} image(s): {e}", ids.len());
        editor.status = format!("Could not save rating: {e}");
        return Task::none();
    }
    editor.apply_to_images(&ids, |img| img.rating = rating);
    editor.status = format!("Rated {} image(s)", ids.len());
    if editor.auto_advance {
        return Task::done(Message::SelectNextImage);
    }
    Task::none()
}

pub fn handle_set_flag(editor: &mut RawEditor, flag: i8) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    let ids = editor.action_target_ids();
    let Some(lib) = &editor.library else {
        return Task::none();
    };
    if let Err(e) = lib.set_flags(&ids, flag) {
        tracing::error!("Failed to flag {} image(s): {e}", ids.len());
        editor.status = format!("Could not save flag: {e}");
        return Task::none();
    }
    editor.apply_to_images(&ids, |img| img.flag = flag);
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
    // Drop anything the new filter hides. Otherwise a selection made under a
    // previous filter stays live but invisible, and Export or a bulk rating
    // would silently act on images the user can no longer see.
    let visible: std::collections::HashSet<i64> = editor.displayed_ids().into_iter().collect();
    editor.multi_selection.retain(|id| visible.contains(id));
    if editor.selection_anchor.is_some_and(|a| !visible.contains(&a)) {
        editor.selection_anchor = None;
    }
    Task::none()
}

/// Images whose immediate parent directory is `folder`.
///
/// Matches `views::library::collect_folder_stats`, which keys the sidebar on
/// `Path::parent()`. Deliberately NOT the filter's `starts_with` prefix test:
/// removing "the folder you clicked" must not also sweep up its subfolders,
/// which appear as their own sidebar rows and are removable on their own.
fn images_in_folder(editor: &RawEditor, folder: &str) -> Vec<i64> {
    editor
        .images
        .iter()
        .filter(|img| {
            std::path::Path::new(&img.path)
                .parent()
                .map(|p| p.to_string_lossy() == folder)
                .unwrap_or(false)
        })
        .map(|img| img.id)
        .collect()
}

/// Count for the confirmation dialog.
pub fn folder_image_count(editor: &RawEditor, folder: &str) -> usize {
    images_in_folder(editor, folder).len()
}

/// Open the "forget this folder" confirmation.
pub fn handle_remove_folder_requested(editor: &mut RawEditor, folder: String) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    if images_in_folder(editor, &folder).is_empty() {
        return Task::none();
    }
    editor.pending_remove_folder = Some(folder);
    editor.active_modal = crate::app::state::Modal::RemoveFolder;
    Task::none()
}

/// Forget the folder: drop its catalogue rows and edits, delete its generated
/// cache files, and leave every RAW file exactly where it is.
pub fn handle_remove_folder_confirmed(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::RemoveFolder {
        return Task::none();
    }
    let Some(folder) = editor.pending_remove_folder.take() else {
        editor.active_modal = crate::app::state::Modal::None;
        return Task::none();
    };
    editor.active_modal = crate::app::state::Modal::None;

    let ids = images_in_folder(editor, &folder);
    if ids.is_empty() {
        return Task::none();
    }

    // Collect cache paths before the rows go away.
    let cache_files: Vec<String> = editor
        .images
        .iter()
        .filter(|img| ids.contains(&img.id))
        .flat_map(|img| {
            [
                img.cache_path_thumb.clone(),
                img.cache_path_instant.clone(),
                img.cache_path_working.clone(),
                img.cache_path_edited.clone(),
            ]
        })
        .flatten()
        .collect();

    let removed = match &mut editor.library {
        Some(lib) => match lib.forget_images(&ids) {
            Ok(n) => n,
            Err(e) => {
                tracing::error!("Failed to forget folder {folder}: {e}");
                editor.status = format!("Could not remove {folder}: {e}");
                return Task::none();
            }
        },
        None => return Task::none(),
    };

    // Regenerable, so plain removal rather than the trash — unlike RAW files,
    // which this operation never touches.
    for path in cache_files {
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("Could not delete cache file {path}: {e}");
            }
        }
    }

    // Same in-memory cleanup the hard-delete path does.
    let gone: std::collections::HashSet<i64> = ids.iter().copied().collect();
    editor.images.retain(|img| !gone.contains(&img.id));
    editor.multi_selection.retain(|id| !gone.contains(id));
    editor.history_map.retain(|id, _| !gone.contains(id));
    editor.pending_loads.retain(|id| !gone.contains(id));
    editor.queued_loads.retain(|(id, _)| !gone.contains(id));
    editor.pending_raw_loads.retain(|id| !gone.contains(id));
    editor.queued_raw_loads.retain(|(id, _)| !gone.contains(id));
    for id in &ids {
        editor.preview_cache.pop(id);
        editor.edited_proxies.pop(id);
        editor.remove_from_raw_cache(*id);
    }
    if editor.selection_anchor.is_some_and(|a| gone.contains(&a)) {
        editor.selection_anchor = None;
    }
    if editor.selected_image_id.is_some_and(|id| gone.contains(&id)) {
        editor.selected_image_id = None;
        editor.editor_readiness = crate::app::state::EditorReadiness::NoSelection;
        editor.working_preview = None;
        editor.working_preview_bytes = None;
        editor.rendered_preview_bytes = None;
        editor.image_resources = None;
        editor.preview_resources = None;
    }
    // The filter would otherwise point at a folder that no longer exists.
    if editor.library_folder_filter.as_deref() == Some(folder.as_str()) {
        editor.library_folder_filter = None;
    }

    editor.status = format!("Removed {removed} images from the catalogue (files kept)");
    Task::none()
}

// ── Cache concurrency ────────────────────────────────────────────────────────

/// Number of images to process in parallel. Each worker runs inside
/// `spawn_blocking` so they won't starve the async runtime.
/// Capped at 4 to keep memory usage predictable (each decode can hold ~100MB).
const CACHE_WORKERS: usize = 4;

/// Fire `CACHE_WORKERS` concurrent cache tasks at once.
fn spawn_cache_workers(db_path: PathBuf) -> Task<Message> {
    Task::batch(
        (0..CACHE_WORKERS).map(|_| {
            Task::perform(process_cache_async(db_path.clone()), Message::CacheProcessed)
        })
    )
}

// Helpers

async fn import_folder_async(
    folder_path: PathBuf,
    db_path: PathBuf,
) -> Result<ImportResult, String> {
    tokio::task::spawn_blocking(move || import_folder_blocking(&folder_path, &db_path))
        .await
        .map_err(|e| format!("Import task failed: {e}"))?
}

/// Add every supported RAW under `folder` to the catalogue.
///
/// One transaction for the whole folder: it used to commit (and query for
/// existence) once per file, and an error halfway through `expect`-panicked
/// the import thread, leaving a partial import behind. `INSERT OR IGNORE`
/// leans on the `path UNIQUE` constraint to skip files already catalogued.
fn import_folder_blocking(folder: &Path, db_path: &Path) -> Result<ImportResult, String> {
    let files = collect_raw_files(folder);
    let mut conn =
        Connection::open(db_path).map_err(|e| format!("Could not open the catalogue: {e}"))?;
    crate::database::library::apply_pragmas(&conn);

    // One timestamp for the batch, so its files sort as one group (by path)
    // instead of splitting wherever the wall clock ticked over a second.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let db_err = |e: rusqlite::Error| format!("Catalogue write failed: {e}");
    let tx = conn.transaction().map_err(db_err)?;
    let mut imported_count = 0;
    {
        let mut insert = tx
            .prepare(
                "INSERT OR IGNORE INTO images (path, filename, imported_at, cache_status)
                 VALUES (?1, ?2, ?3, 'pending')",
            )
            .map_err(db_err)?;
        for path in &files {
            let filename = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            imported_count += insert
                .execute(rusqlite::params![path.to_string_lossy(), filename, now])
                .map_err(db_err)?;
        }
    }
    tx.commit().map_err(db_err)?;

    Ok(ImportResult {
        imported_count,
        skipped_count: files.len() - imported_count,
    })
}

/// Supported RAW files under `folder`, recursively, in file-name order.
///
/// Skips macOS `._` AppleDouble sidecars: they carry the RAW's extension but
/// hold only Finder metadata, and copying a card through a Mac leaves one
/// beside every photo — each would otherwise import as a broken image.
fn collect_raw_files(folder: &Path) -> Vec<PathBuf> {
    WalkDir::new(folder)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with("._"))
        .map(walkdir::DirEntry::into_path)
        .filter(|path| raw::is_supported_raw(path) && path.is_file())
        .collect()
}

async fn process_cache_async(db_path: PathBuf) -> Result<(i64, String, String, String), (i64, String)> {
    // Atomically claim one pending image to avoid races with concurrent workers.
    // UPDATE ... RETURNING picks and marks in a single statement with no gap.
    let claimed = tokio::task::spawn_blocking({
        let db_path = db_path.clone();
        move || -> Result<Option<(i64, String)>, String> {
            let conn = Connection::open(&db_path).map_err(|e| format!("DB: {e}"))?;
            crate::database::library::apply_pragmas(&conn);
            let mut stmt = conn.prepare(
                "UPDATE images SET cache_status = 'processing'
                 WHERE id = (SELECT id FROM images WHERE cache_status = 'pending' LIMIT 1)
                 RETURNING id, path"
            ).map_err(|e| format!("Prepare: {e}"))?;
            let row = stmt.query_row([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .optional()
                .map_err(|e| format!("Query: {e}"))?;
            Ok(row)
        }
    }).await.map_err(|e| (0, format!("Join: {e}")))?
      .map_err(|e| (0, e))?;

    let Some((id, path)) = claimed else {
        return Err((0, "No pending images".to_string()));
    };

    let result = tokio::task::spawn_blocking(move || {
        raw::processor::process_image(std::path::Path::new(&path), id)
    }).await.map_err(|e| (id, format!("Join Error: {}", e)))?;

    match result {
        Ok(res) => Ok((id, res.0, res.1, res.2)),
        Err(e) => Err((id, e)),
    }
}

/// Write freshly generated cache-tier paths onto the in-memory catalog row.
///
/// Replaces a full `get_all_images()` reload that used to run once per cached
/// image, making a large import O(N^2) in database round-trips and allocations.
/// A missing id is not an error — the catalog can legitimately have moved on
/// (the image was deleted while its cache job was in flight).
fn apply_cache_paths(
    images: &mut [crate::database::models::Image],
    image_id: i64,
    thumb: String,
    instant: String,
    working: String,
) {
    if let Some(img) = images.iter_mut().find(|i| i.id == image_id) {
        img.cache_path_thumb = Some(thumb);
        img.cache_path_instant = Some(instant);
        img.cache_path_working = Some(working);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::Image;

    fn image(id: i64) -> Image {
        Image {
            id,
            filename: format!("IMG_{id}.NEF"),
            path: format!("/photos/IMG_{id}.NEF"),
            cache_path_thumb: None,
            cache_path_instant: None,
            cache_path_working: None,
            cache_path_edited: None,
            file_status: "exists".to_string(),
            rating: 0,
            flag: 0,
        }
    }

    #[test]
    fn apply_cache_paths_updates_only_the_target_row() {
        let mut images = vec![image(1), image(2), image(3)];
        apply_cache_paths(&mut images, 2, "t".into(), "i".into(), "w".into());

        assert_eq!(images[1].cache_path_thumb.as_deref(), Some("t"));
        assert_eq!(images[1].cache_path_instant.as_deref(), Some("i"));
        assert_eq!(images[1].cache_path_working.as_deref(), Some("w"));

        // Neighbours untouched — the old full-reload path rebuilt every row.
        assert!(images[0].cache_path_thumb.is_none());
        assert!(images[2].cache_path_thumb.is_none());
    }

    #[test]
    fn apply_cache_paths_ignores_an_unknown_id() {
        let mut images = vec![image(1)];
        apply_cache_paths(&mut images, 99, "t".into(), "i".into(), "w".into());
        assert!(images[0].cache_path_thumb.is_none());
    }

    #[test]
    fn apply_cache_paths_overwrites_a_stale_entry() {
        let mut images = vec![image(1)];
        images[0].cache_path_thumb = Some("old".into());
        apply_cache_paths(&mut images, 1, "new".into(), "i".into(), "w".into());
        assert_eq!(images[0].cache_path_thumb.as_deref(), Some("new"));
    }

    // ── folder removal targeting ─────────────────────────────────────────────

    fn folder_img(id: i64, path: &str) -> crate::database::models::Image {
        crate::database::models::Image {
            id,
            filename: format!("{id}.NEF"),
            path: path.to_string(),
            cache_path_thumb: None,
            cache_path_instant: None,
            cache_path_working: None,
            cache_path_edited: None,
            file_status: "exists".to_string(),
            rating: 0,
            flag: 0,
        }
    }

    fn folder_editor() -> RawEditor {
        let mut e = RawEditor::new().0;
        e.images = vec![
            folder_img(1, "/photos/2024/a.NEF"),
            folder_img(2, "/photos/2024/b.NEF"),
            folder_img(3, "/photos/2024-old/c.NEF"),
            folder_img(4, "/photos/2024/nested/d.NEF"),
            folder_img(5, "/photos/2025/e.NEF"),
        ];
        e
    }

    /// The trap: a bare `starts_with` prefix test would sweep up
    /// `/photos/2024-old` when removing `/photos/2024`. Matching on the exact
    /// parent directory is what the sidebar rows actually represent.
    #[test]
    fn removing_a_folder_does_not_catch_similarly_named_siblings() {
        let e = folder_editor();
        assert_eq!(images_in_folder(&e, "/photos/2024"), vec![1, 2]);
        assert_eq!(images_in_folder(&e, "/photos/2024-old"), vec![3]);
    }

    /// Subfolders are their own sidebar rows, so removing the parent must not
    /// silently take them too.
    #[test]
    fn removing_a_folder_leaves_its_subfolders_alone() {
        let e = folder_editor();
        assert!(!images_in_folder(&e, "/photos/2024").contains(&4));
        assert_eq!(images_in_folder(&e, "/photos/2024/nested"), vec![4]);
    }

    #[test]
    fn unknown_folder_matches_nothing() {
        let e = folder_editor();
        assert!(images_in_folder(&e, "/photos/nope").is_empty());
        assert_eq!(folder_image_count(&e, "/photos/2024"), 2);
    }

    // ── import ────────────────────────────────────────────────────────────────

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("raw-editor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        dir
    }

    #[test]
    fn collect_raw_files_is_sorted_and_skips_sidecars_and_other_files() {
        let dir = scratch_dir("collect");
        for name in ["b.NEF", "a.nef", "._a.nef", "notes.txt", "c.jpg", "sub/d.cr3"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let names: Vec<String> = collect_raw_files(&dir)
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.nef", "b.NEF", "sub/d.cr3"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Re-importing a folder adds only what is new, and reports the rest as
    /// skipped instead of failing on the UNIQUE path constraint.
    #[test]
    fn importing_twice_only_adds_new_files() {
        let dir = scratch_dir("import");
        let db = dir.join("catalogue.db");
        drop(crate::database::library::Library::open_at(&db).unwrap());
        std::fs::write(dir.join("a.nef"), b"x").unwrap();

        let first = import_folder_blocking(&dir, &db).unwrap();
        assert_eq!((first.imported_count, first.skipped_count), (1, 0));

        std::fs::write(dir.join("b.nef"), b"x").unwrap();
        let second = import_folder_blocking(&dir, &db).unwrap();
        assert_eq!((second.imported_count, second.skipped_count), (1, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_reports_an_unopenable_catalogue_instead_of_panicking() {
        let dir = scratch_dir("import-err");
        let db = dir.join("no-such-dir").join("catalogue.db");
        assert!(import_folder_blocking(&dir, &db).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
