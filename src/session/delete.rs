//! Delete Image: two modes reached from the same Delete/Backspace key.
//!
//! - Mark for removal (soft): reuses the existing `rating: u8` column with a
//!   offset (MARKED_BASE + the stars, see database::models) instead of a new DB
//!   column — deliberately, to avoid a schema migration. Hidden from Develop,
//!   still visible (with a badge) in Library/Cull.
//! - Delete from disk (hard): moves the RAW file + cache tiers to the OS
//!   trash and removes the DB row, off the UI thread.
//!
//! Delete applies to the whole multi-selection when non-empty, falling back
//! to the single selected image — the same pattern as rate_targets/
//! flag_targets. Pressing Delete again on an already-fully-marked
//! selection instantly toggles the mark off (no modal, since un-marking
//! isn't destructive); otherwise it opens the confirmation modal.

use std::collections::HashSet;
use std::path::PathBuf;

use super::{Effects, Moved, Session, SessionEvent};
use crate::app::state::EditorReadiness;
use crate::database::models::{is_marked, with_mark};

#[derive(Debug, Clone)]
pub struct HardDeleteOutcome {
    pub deleted_ids: Vec<i64>,
    pub failed: Vec<(i64, String)>,
}

struct DeleteTarget {
    id: i64,
    path: String,
    cache_thumb: Option<String>,
    cache_instant: Option<String>,
    cache_working: Option<String>,
    cache_edited: Option<String>,
}

/// What Delete does with the action targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteRequest {
    /// No targets, or un-marking failed (the status says why).
    Nothing,
    /// They were all marked: the mark is off again, no questions asked.
    Unmarked,
    /// Ask how to delete them (`pending_delete_ids` holds them).
    Confirm,
}

impl Session {
    /// Delete/Backspace: instant-unmark when every target is already marked,
    /// otherwise ask.
    pub fn request_delete(&mut self) -> DeleteRequest {
        let ids = self.action_target_ids();
        if ids.is_empty() {
            return DeleteRequest::Nothing;
        }

        let marked: HashSet<i64> = self.images.iter().filter(|i| is_marked(i.rating)).map(|i| i.id).collect();
        if !ids.iter().all(|id| marked.contains(id)) {
            self.pending_delete_ids = ids;
            return DeleteRequest::Confirm;
        }
        if let Some(lib) = &self.library {
            if let Err(e) = lib.set_marked(&ids, false) {
                tracing::error!("Failed to unmark {} image(s): {e}", ids.len());
                self.status = format!("Could not unmark: {e}");
                return DeleteRequest::Nothing;
            }
            self.apply_to_images(&ids, |img| img.rating = with_mark(img.rating, false));
        }
        self.status = format!("Unmarked {} image(s)", ids.len());
        DeleteRequest::Unmarked
    }

    /// Mark the pending targets for removal. `true` when they were marked.
    pub fn mark_pending_for_removal(&mut self) -> bool {
        let ids = std::mem::take(&mut self.pending_delete_ids);
        if ids.is_empty() {
            return false;
        }
        if let Some(lib) = &self.library {
            if let Err(e) = lib.set_marked(&ids, true) {
                tracing::error!("Failed to mark {} image(s) for removal: {e}", ids.len());
                self.status = format!("Could not mark for removal: {e}");
                return false;
            }
            self.apply_to_images(&ids, |img| img.rating = with_mark(img.rating, true));
        }
        self.status = format!("Marked {} image(s) for removal", ids.len());
        true
    }

    /// Move the pending targets' files to the trash and drop their rows.
    pub fn delete_pending_from_disk(&mut self) -> Effects {
        if self.is_deleting {
            return Effects::none();
        }
        let ids = std::mem::take(&mut self.pending_delete_ids);
        let targets: Vec<DeleteTarget> = ids
            .iter()
            .filter_map(|id| {
                self.images.iter().find(|i| i.id == *id).map(|img| DeleteTarget {
                    id: img.id,
                    path: img.path.clone(),
                    cache_thumb: img.cache_path_thumb.clone(),
                    cache_instant: img.cache_path_instant.clone(),
                    cache_working: img.cache_path_working.clone(),
                    cache_edited: img.cache_path_edited.clone(),
                })
            })
            .collect();
        let Some(db_path) = self.library.as_ref().map(|l| l.path().clone()) else {
            return Effects::none();
        };
        if targets.is_empty() {
            return Effects::none();
        }
        self.is_deleting = true;
        self.status = format!("Deleting {} image(s)…", targets.len());
        Effects::perform("hard delete", async move {
            SessionEvent::HardDeleteComplete(hard_delete_async(targets, db_path).await)
        })
    }

    pub fn hard_delete_complete(&mut self, outcome: HardDeleteOutcome) -> Moved {
        self.is_deleting = false;
        let deleted: std::collections::HashSet<i64> = outcome.deleted_ids.iter().copied().collect();

        // Compute the fallback selection before mutating self.images: find how
        // many deleted images preceded the current selection, so the adjusted
        // index lands roughly where the user was.
        let mut fallback: Option<i64> = None;
        if let Some(sel) = self.selected_image_id {
            if deleted.contains(&sel) {
                if let Some(old_idx) = self.images.iter().position(|i| i.id == sel) {
                    let remaining: Vec<i64> = self
                        .images
                        .iter()
                        .filter(|i| !deleted.contains(&i.id))
                        .map(|i| i.id)
                        .collect();
                    if !remaining.is_empty() {
                        let deleted_before = self.images[..old_idx]
                            .iter()
                            .filter(|i| deleted.contains(&i.id))
                            .count();
                        let new_idx = (old_idx - deleted_before).min(remaining.len() - 1);
                        fallback = Some(remaining[new_idx]);
                    }
                }
            }
        }

        self.images.retain(|img| !deleted.contains(&img.id));
        self.multi_selection.retain(|id| !deleted.contains(id));
        self.pending_delete_ids.clear();

        for id in &outcome.deleted_ids {
            self.preview_cache.pop(id);
            self.edited_proxies.pop(id);
            self.remove_from_raw_cache(*id);
            self.history_map.remove(id);
            self.pending_loads.remove(id);
            self.pending_raw_loads.remove(id);
        }
        self.queued_loads.retain(|(id, _)| !deleted.contains(id));
        self.queued_raw_loads.retain(|(id, _)| !deleted.contains(id));

        let mut after = Moved::Stay;
        if let Some(sel) = self.selected_image_id {
            if deleted.contains(&sel) {
                // The open image is gone: whatever it had unsaved has nowhere to
                // go. `handle_image_selected` flushes first, and with the
                // selection already moved it would write these params onto the
                // fallback image.
                self.edits_dirty = false;
                self.working_preview_bytes = None;
                self.rendered_preview_bytes = None;
                self.image_resources = None;
                self.preview_resources = None;
                self.selected_image_id = fallback;
                if let Some(next_id) = fallback {
                    after = Moved::To(next_id);
                } else {
                    self.editor_readiness = EditorReadiness::NoSelection;
                    self.camera_preview = Some(super::PreviewHandle::Clear);
                    after = Moved::Cleared;
                }
            }
        }

        self.status = if outcome.failed.is_empty() {
            format!("Deleted {} image(s).", outcome.deleted_ids.len())
        } else {
            for (id, err) in &outcome.failed {
                tracing::error!("Failed to delete image {}: {}", id, err);
            }
            format!(
                "Deleted {} image(s), {} failed — see log.",
                outcome.deleted_ids.len(),
                outcome.failed.len()
            )
        };

        after
    }
}

async fn hard_delete_async(targets: Vec<DeleteTarget>, db_path: PathBuf) -> HardDeleteOutcome {
    tokio::task::spawn_blocking(move || {
        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => {
                crate::database::library::apply_pragmas(&c);
                c
            }
            Err(e) => {
                tracing::error!("delete: failed to open DB: {}", e);
                return HardDeleteOutcome {
                    deleted_ids: vec![],
                    failed: targets
                        .into_iter()
                        .map(|t| (t.id, format!("DB open failed: {e}")))
                        .collect(),
                };
            }
        };

        let mut deleted_ids = Vec::new();
        let mut failed = Vec::new();

        for t in targets {
            // Cache tiers are regenerable — a failure here is non-fatal and
            // doesn't block the DB row delete.
            for cache in [&t.cache_thumb, &t.cache_instant, &t.cache_working, &t.cache_edited]
                .into_iter()
                .flatten()
            {
                if let Err(e) = trash_or_missing(cache) {
                    tracing::warn!(
                        "delete: cache tier trash failed for image {} ({}): {}",
                        t.id,
                        cache,
                        e
                    );
                }
            }

            match trash_or_missing(&t.path) {
                // Edits go explicitly: `ON DELETE CASCADE` is inert because
                // foreign keys are never enabled (see `apply_pragmas`), so
                // this used to orphan an `edits` row per deleted image.
                Ok(()) => match conn
                    .execute("DELETE FROM edits WHERE image_id = ?1", [t.id])
                    .and_then(|_| conn.execute("DELETE FROM images WHERE id = ?1", [t.id]))
                {
                    Ok(_) => deleted_ids.push(t.id),
                    Err(e) => {
                        tracing::error!("delete: DB row delete failed for {}: {}", t.id, e);
                        failed.push((t.id, format!("DB delete failed: {e}")));
                    }
                },
                Err(e) => {
                    tracing::error!("delete: trash failed for image {} ({}): {}", t.id, t.path, e);
                    failed.push((t.id, format!("Trash failed: {e}")));
                }
            }
        }
        HardDeleteOutcome { deleted_ids, failed }
    })
    .await
    .unwrap_or_else(|e| {
        tracing::error!("delete: task join failed: {}", e);
        HardDeleteOutcome { deleted_ids: vec![], failed: vec![] }
    })
}

/// Treat "file already gone" as success — the goal state (file not present)
/// is already achieved, so this isn't an error worth blocking the DB delete on.
fn trash_or_missing(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return Ok(());
    }
    trash::delete(p).map_err(|e| e.to_string())
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Not run in normal `cargo test` (needs a real desktop trash location,
    /// which CI/sandboxed environments may lack) — run explicitly with
    /// `cargo test --lib trash_crate_moves_file -- --ignored` to verify the
    /// `trash` crate actually works on a given machine before relying on it.
    #[test]
    #[ignore]
    fn trash_crate_moves_file() {
        let dir = std::env::temp_dir().join("raw-editor-trash-smoke-test");
        std::fs::create_dir_all(&dir).unwrap();
        let scratch = dir.join("scratch.txt");
        std::fs::write(&scratch, b"smoke test").unwrap();
        assert!(scratch.exists());

        trash_or_missing(scratch.to_str().unwrap()).expect("trash::delete failed");
        assert!(!scratch.exists(), "file should be gone from its original location");

        // Missing files are treated as already-succeeded, not an error.
        trash_or_missing(scratch.to_str().unwrap()).expect("missing file should not error");
    }

    /// Delete on a selection that is all marked un-marks it without asking;
    /// anything unmarked asks first.
    #[test]
    fn delete_unmarks_or_asks() {
        let mut s = Session::default();
        assert_eq!(s.request_delete(), DeleteRequest::Nothing, "nothing selected");
        s.images = vec![crate::database::models::Image {
            id: 1,
            filename: "a.nef".into(),
            path: "/x/a.nef".into(),
            cache_path_thumb: None,
            cache_path_instant: None,
            cache_path_working: None,
            cache_path_edited: None,
            file_status: "exists".into(),
            rating: with_mark(0, true),
            flag: 0,
        }];
        s.selected_image_id = Some(1);
        assert_eq!(s.request_delete(), DeleteRequest::Unmarked);
        assert!(s.pending_delete_ids.is_empty(), "un-marking asked first");
        s.images[0].rating = with_mark(0, false);
        assert_eq!(s.request_delete(), DeleteRequest::Confirm);
        assert_eq!(s.pending_delete_ids, vec![1]);
    }
}
