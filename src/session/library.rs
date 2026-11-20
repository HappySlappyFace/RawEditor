//! The catalogue: import, the cache-tier workers, ratings and flags, folder
//! filters and forgetting a folder. (Pickers, the drop-import dialog and the
//! window are the app's.)

use rusqlite::{Connection, OptionalExtension};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

use super::{Effects, Session, SessionEvent};
use crate::app::state::EditorReadiness;
use crate::database;
use crate::raw;

/// Result of a folder import operation
#[derive(Debug, Clone)]
pub struct ImportResult {
    pub imported_count: usize,
    pub skipped_count: usize,
}

/// One cache job: the image and its thumb/instant/working paths, or the
/// image (0 when there was nothing left to claim) and why it failed.
pub type CacheResult = Result<(i64, String, String, String), (i64, String)>;

/// What one finished cache job means for the queue.
#[must_use]
pub enum CacheStep {
    /// More to do: the worker's replacement.
    Continue(Effects),
    /// This worker retired; others are still running.
    Retired,
    /// The queue is empty: every tier is generated.
    Finished,
}

/// Number of images to process in parallel. Each worker runs inside
/// `spawn_blocking` so they won't starve the async runtime.
/// Capped at 4 to keep memory usage predictable (each decode can hold ~100MB).
const CACHE_WORKERS: usize = 4;

impl Session {
    /// The catalogue was read at startup. `None` when the library could not
    /// be opened (the status says why); otherwise the cache workers to start.
    pub fn catalogue_loaded(&mut self, result: Result<Vec<database::models::Image>, String>) -> Option<Effects> {
        let images = match result {
            Ok(images) => images,
            Err(e) => {
                self.status = format!("Failed to load database: {}", e);
                return None;
            }
        };
        let library = match database::library::Library::new() {
            Ok(library) => library,
            Err(e) => {
                self.status = format!("Failed to create library: {:?}", e);
                return None;
            }
        };
        self.status = format!("Loaded {} images.", images.len());
        self.images = images;
        self.pending_cache_count = library.count_pending_cache().unwrap_or(0);
        let db_path = library.path().clone();
        self.library = Some(library);
        Some(self.spawn_cache_workers(db_path))
    }

    /// Import the RAWs under `folder`.
    pub fn import_folder(&mut self, folder: PathBuf) -> Effects {
        let Some(library) = &self.library else { return Effects::none() };
        self.status = format!("Importing {}…", folder.display());
        let db_path = library.path().clone();
        Effects::perform("import", async move {
            let result = tokio::task::spawn_blocking(move || import_folder_blocking(&folder, &db_path))
                .await
                .map_err(|e| format!("Import task failed: {e}"))
                .and_then(|r| r);
            SessionEvent::ImportComplete(result)
        })
    }

    /// Import several folders in one background job.
    pub fn import_folders(&mut self, folders: Vec<PathBuf>) -> Effects {
        let Some(library) = &self.library else { return Effects::none() };
        if folders.is_empty() {
            return Effects::none();
        }
        self.status = match folders.as_slice() {
            [one] => format!("Importing {}…", one.display()),
            many => format!("Importing {} folders…", many.len()),
        };
        let db_path = library.path().clone();
        Effects::perform("import", async move {
            let result = tokio::task::spawn_blocking(move || {
                let mut total = ImportResult { imported_count: 0, skipped_count: 0 };
                for folder in &folders {
                    let result = import_folder_blocking(folder, &db_path)?;
                    total.imported_count += result.imported_count;
                    total.skipped_count += result.skipped_count;
                }
                Ok(total)
            })
            .await
            .map_err(|e| format!("Import task failed: {e}"))
            .and_then(|r| r);
            SessionEvent::ImportComplete(result)
        })
    }

    pub fn import_complete(&mut self, result: Result<ImportResult, String>) -> Effects {
        let result = match result {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("Import failed: {e}");
                self.status = format!("Import failed: {e}");
                return Effects::none();
            }
        };
        let Some(library) = &self.library else { return Effects::none() };
        // Full reload is correct here: the import added rows this process has
        // never seen. It runs once per import, not once per cached image.
        self.images = library.get_all_images().unwrap_or_default();
        self.pending_cache_count = library.count_pending_cache().unwrap_or(0);
        self.status = format!("Import complete! Added {}, skipped {}.", result.imported_count, result.skipped_count);
        let db_path = library.path().clone();
        self.spawn_cache_workers(db_path)
    }

    pub fn cache_processed(&mut self, result: CacheResult) -> CacheStep {
        let Some(library) = &self.library else {
            return CacheStep::Retired;
        };

        // `image_id == 0` on the Err path means the worker found nothing left to
        // claim. Nothing was consumed from the queue, so don't decrement, and let
        // this worker retire instead of respawning — the others are still running
        // and will drive the count to zero.
        let mut claimed_one = true;

        match result {
            Ok((image_id, thumb, instant, working)) => {
                let _ = library.set_image_cache_paths(image_id, &thumb, &instant, &working);
                apply_cache_paths(&mut self.images, image_id, thumb, instant, working);
            }
            Err((image_id, ref err)) if image_id != 0 => {
                if let Err(db_err) = library.mark_cache_failed(image_id) {
                    tracing::error!("Failed to mark image {} as failed: {}", image_id, db_err);
                }
                tracing::warn!("Cache processing failed for image {}: {}", image_id, err);
            }
            _ => claimed_one = false,
        }

        if claimed_one {
            self.pending_cache_count = (self.pending_cache_count - 1).max(0);
        }

        if self.pending_cache_count > 0 && claimed_one {
            let total = self.images.len() as i64;
            let done = total - self.pending_cache_count;
            self.status = format!(
                "Caching: {}/{} - {} remaining",
                done, total, self.pending_cache_count
            );
            let db_path = library.path().clone();
            // One replacement worker per completion keeps concurrency steady
            return CacheStep::Continue(cache_worker(db_path));
        }

        // This worker retires: free its slot so a later import can top up.
        self.cache_workers_live = self.cache_workers_live.saturating_sub(1);
        if self.pending_cache_count == 0 {
            CacheStep::Finished
        } else {
            CacheStep::Retired
        }
    }

    /// Top the running cache workers up to `CACHE_WORKERS`. Each completion
    /// starts its own replacement, so starting a full set again while some are
    /// still running (a second import) would multiply the concurrency.
    pub fn spawn_cache_workers(&mut self, db_path: PathBuf) -> Effects {
        let missing = CACHE_WORKERS.saturating_sub(self.cache_workers_live);
        self.cache_workers_live += missing;
        Effects::batch((0..missing).map(|_| cache_worker(db_path.clone())))
    }

    /// Rate the action targets. `true` when it was saved.
    pub fn rate_targets(&mut self, rating: u8) -> bool {
        let ids = self.action_target_ids();
        let Some(lib) = &self.library else {
            return false;
        };
        // Memory follows the database: only touch the in-memory rows once the
        // write has landed, so a failed write can't show ratings that a restart
        // would silently take back.
        if let Err(e) = lib.set_ratings(&ids, rating) {
            tracing::error!("Failed to rate {} image(s): {e}", ids.len());
            self.status = format!("Could not save rating: {e}");
            return false;
        }
        self.apply_to_images(&ids, |img| img.rating = rating);
        self.status = format!("Rated {} image(s)", ids.len());
        true
    }

    /// Flag the action targets. `true` when it was saved.
    pub fn flag_targets(&mut self, flag: i8) -> bool {
        let ids = self.action_target_ids();
        let Some(lib) = &self.library else {
            return false;
        };
        if let Err(e) = lib.set_flags(&ids, flag) {
            tracing::error!("Failed to flag {} image(s): {e}", ids.len());
            self.status = format!("Could not save flag: {e}");
            return false;
        }
        self.apply_to_images(&ids, |img| img.flag = flag);
        self.status = format!("Flagged {} image(s)", ids.len());
        true
    }

    /// Show only `folder_filter` (a path prefix), or everything. Returns the
    /// images now displayed.
    pub fn set_library_folder_filter(&mut self, folder_filter: Option<String>) -> HashSet<i64> {
        self.library_folder_filter = folder_filter;
        // Drop anything the new filter hides. Otherwise a selection made under a
        // previous filter stays live but invisible, and Export or a bulk rating
        // would silently act on images the user can no longer see.
        let visible: HashSet<i64> = self.displayed_ids().into_iter().collect();
        self.multi_selection.retain(|id| visible.contains(id));
        visible
    }

    /// Images whose immediate parent directory is `folder`.
    ///
    /// Matches `views::library::collect_folder_stats`, which keys the sidebar on
    /// `Path::parent()`. Deliberately NOT the filter's `starts_with` prefix test:
    /// removing "the folder you clicked" must not also sweep up its subfolders,
    /// which appear as their own sidebar rows and are removable on their own.
    pub fn images_in_folder(&self, folder: &str) -> Vec<i64> {
        self.images
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
    pub fn folder_image_count(&self, folder: &str) -> usize {
        self.images_in_folder(folder).len()
    }

    /// Forget the folder: drop its catalogue rows and edits, delete its
    /// generated cache files, and leave every RAW file exactly where it is.
    /// Returns the images forgotten (empty when nothing was).
    pub fn remove_folder(&mut self, folder: &str) -> HashSet<i64> {
        let ids = self.images_in_folder(folder);
        if ids.is_empty() {
            return HashSet::new();
        }

        // Collect cache paths before the rows go away.
        let cache_files: Vec<String> = self
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

        let removed = match &mut self.library {
            Some(lib) => match lib.forget_images(&ids) {
                Ok(n) => n,
                Err(e) => {
                    tracing::error!("Failed to forget folder {folder}: {e}");
                    self.status = format!("Could not remove {folder}: {e}");
                    return HashSet::new();
                }
            },
            None => return HashSet::new(),
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
        let gone: HashSet<i64> = ids.iter().copied().collect();
        self.images.retain(|img| !gone.contains(&img.id));
        self.multi_selection.retain(|id| !gone.contains(id));
        self.history_map.retain(|id, _| !gone.contains(id));
        self.pending_loads.retain(|id| !gone.contains(id));
        self.queued_loads.retain(|(id, _)| !gone.contains(id));
        self.pending_raw_loads.retain(|id| !gone.contains(id));
        self.queued_raw_loads.retain(|(id, _)| !gone.contains(id));
        for id in &ids {
            self.preview_cache.pop(id);
            self.edited_proxies.pop(id);
            self.remove_from_raw_cache(*id);
        }
        if self.selected_image_id.is_some_and(|id| gone.contains(&id)) {
            self.selected_image_id = None;
            self.editor_readiness = EditorReadiness::NoSelection;
            self.working_preview_bytes = None;
            self.camera_preview = Some(super::PreviewHandle::Clear);
            self.rendered_preview_bytes = None;
            self.image_resources = None;
            self.preview_resources = None;
        }
        // The filter would otherwise point at a folder that no longer exists.
        if self.library_folder_filter.as_deref() == Some(folder) {
            self.library_folder_filter = None;
        }

        self.status = format!("Removed {removed} images from the catalogue (files kept)");
        gone
    }
}

/// One cache worker: claim a pending image and generate its tiers.
fn cache_worker(db_path: PathBuf) -> Effects {
    Effects::perform("cache tiers", async move { SessionEvent::CacheProcessed(process_cache_async(db_path).await) })
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
pub fn collect_raw_files(folder: &Path) -> Vec<PathBuf> {
    WalkDir::new(folder)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with("._"))
        .map(walkdir::DirEntry::into_path)
        .filter(|path| raw::is_supported_raw(path) && path.is_file())
        .collect()
}

async fn process_cache_async(db_path: PathBuf) -> CacheResult {
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

    fn folder_session() -> Session {
        let mut e = Session::default();
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
        let e = folder_session();
        assert_eq!(e.images_in_folder("/photos/2024"), vec![1, 2]);
        assert_eq!(e.images_in_folder("/photos/2024-old"), vec![3]);
    }

    /// Subfolders are their own sidebar rows, so removing the parent must not
    /// silently take them too.
    #[test]
    fn removing_a_folder_leaves_its_subfolders_alone() {
        let e = folder_session();
        assert!(!e.images_in_folder("/photos/2024").contains(&4));
        assert_eq!(e.images_in_folder("/photos/2024/nested"), vec![4]);
    }

    #[test]
    fn unknown_folder_matches_nothing() {
        let e = folder_session();
        assert!(e.images_in_folder("/photos/nope").is_empty());
        assert_eq!(e.folder_image_count("/photos/2024"), 2);
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

    /// Two imports in a row must not stack a second set of workers on the
    /// first (each completion starts its own replacement).
    #[test]
    fn cache_workers_are_topped_up_not_multiplied() {
        let dir = scratch_dir("workers");
        let mut e = Session::default();
        e.library = Some(crate::database::library::Library::open_at(&dir.join("c.db")).unwrap());
        let db = dir.join("c.db");
        assert_eq!(e.spawn_cache_workers(db.clone()).len(), CACHE_WORKERS);
        assert!(e.spawn_cache_workers(db.clone()).is_empty());
        assert_eq!(e.cache_workers_live, CACHE_WORKERS);
        // A worker that finds nothing left retires and frees its slot (and,
        // with nothing pending, reports the queue done).
        assert!(matches!(e.cache_processed(Err((0, "empty".into()))), CacheStep::Finished));
        assert_eq!(e.cache_workers_live, CACHE_WORKERS - 1);
        assert_eq!(e.spawn_cache_workers(db).len(), 1);
        assert_eq!(e.cache_workers_live, CACHE_WORKERS);
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
