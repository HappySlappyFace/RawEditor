// Delete Image: two modes reached from the same Delete/Backspace key.
//
// - Mark for removal (soft): reuses the existing `rating: u8` column with a
//   reserved sentinel (MARKED_FOR_REMOVAL_RATING) instead of a new DB
//   column — deliberately, to avoid a schema migration. Hidden from Develop,
//   still visible (with a badge) in Library/Cull.
// - Delete from disk (hard): moves the RAW file + cache tiers to the OS
//   trash and removes the DB row, off the UI thread.
//
// Delete applies to the whole multi-selection when non-empty, falling back
// to the single selected image — the same pattern as handle_set_flag/
// handle_set_rating. Pressing Delete again on an already-fully-marked
// selection instantly toggles the mark off (no modal, since un-marking
// isn't destructive); otherwise it opens the confirmation modal.

use crate::app::handlers::navigation;
use crate::app::message::Message;
use crate::app::state::{EditorReadiness, Modal, RawEditor};
use crate::database::models::MARKED_FOR_REMOVAL_RATING;
use iced::Task;
use std::path::PathBuf;

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
}

fn target_ids(editor: &RawEditor) -> Vec<i64> {
    if !editor.multi_selection.is_empty() {
        editor.multi_selection.iter().copied().collect()
    } else if let Some(id) = editor.selected_image_id {
        vec![id]
    } else {
        vec![]
    }
}

/// Dispatcher for Message::DeleteImage: instant-unmark when every target is
/// already marked, otherwise opens the confirmation modal.
pub fn handle_delete_image_requested(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    let ids = target_ids(editor);
    if ids.is_empty() {
        return Task::none();
    }

    let all_marked = ids.iter().all(|id| {
        editor
            .images
            .iter()
            .find(|i| i.id == *id)
            .map(|i| i.rating == MARKED_FOR_REMOVAL_RATING)
            .unwrap_or(false)
    });

    if all_marked {
        if let Some(lib) = &editor.library {
            for &id in &ids {
                let _ = lib.set_image_rating(id, 0);
                if let Some(img) = editor.images.iter_mut().find(|i| i.id == id) {
                    img.rating = 0;
                }
            }
        }
        editor.status = format!("Unmarked {} image(s)", ids.len());
        Task::none()
    } else {
        editor.pending_delete_ids = ids;
        editor.active_modal = Modal::Delete;
        Task::none()
    }
}

pub fn handle_mark_for_removal_confirmed(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::Delete {
        return Task::none();
    }
    let ids = std::mem::take(&mut editor.pending_delete_ids);
    editor.active_modal = Modal::None;
    if ids.is_empty() {
        return Task::none();
    }

    if let Some(lib) = &editor.library {
        for &id in &ids {
            let _ = lib.set_image_rating(id, MARKED_FOR_REMOVAL_RATING);
            if let Some(img) = editor.images.iter_mut().find(|i| i.id == id) {
                img.rating = MARKED_FOR_REMOVAL_RATING;
            }
        }
    }
    editor.status = format!("Marked {} image(s) for removal", ids.len());

    navigation::ensure_develop_selection_not_marked(editor)
}

pub fn handle_delete_from_disk_confirmed(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::Delete || editor.is_deleting {
        return Task::none();
    }
    let ids = std::mem::take(&mut editor.pending_delete_ids);
    editor.active_modal = Modal::None;
    if ids.is_empty() {
        return Task::none();
    }

    let targets: Vec<DeleteTarget> = ids
        .iter()
        .filter_map(|id| {
            editor.images.iter().find(|i| i.id == *id).map(|img| DeleteTarget {
                id: img.id,
                path: img.path.clone(),
                cache_thumb: img.cache_path_thumb.clone(),
                cache_instant: img.cache_path_instant.clone(),
                cache_working: img.cache_path_working.clone(),
            })
        })
        .collect();

    let Some(db_path) = editor.library.as_ref().map(|l| l.path().clone()) else {
        return Task::none();
    };
    if targets.is_empty() {
        return Task::none();
    }

    editor.is_deleting = true;
    editor.status = format!("Deleting {} image(s)...", targets.len());
    Task::perform(hard_delete_async(targets, db_path), Message::HardDeleteComplete)
}

pub fn handle_hard_delete_complete(editor: &mut RawEditor, outcome: HardDeleteOutcome) -> Task<Message> {
    editor.is_deleting = false;
    let deleted: std::collections::HashSet<i64> = outcome.deleted_ids.iter().copied().collect();

    // Compute the fallback selection before mutating editor.images: find how
    // many deleted images preceded the current selection, so the adjusted
    // index lands roughly where the user was.
    let mut fallback: Option<i64> = None;
    if let Some(sel) = editor.selected_image_id {
        if deleted.contains(&sel) {
            if let Some(old_idx) = editor.images.iter().position(|i| i.id == sel) {
                let remaining: Vec<i64> = editor
                    .images
                    .iter()
                    .filter(|i| !deleted.contains(&i.id))
                    .map(|i| i.id)
                    .collect();
                if !remaining.is_empty() {
                    let deleted_before = editor.images[..old_idx]
                        .iter()
                        .filter(|i| deleted.contains(&i.id))
                        .count();
                    let new_idx = (old_idx - deleted_before).min(remaining.len() - 1);
                    fallback = Some(remaining[new_idx]);
                }
            }
        }
    }

    editor.images.retain(|img| !deleted.contains(&img.id));
    editor.multi_selection.retain(|id| !deleted.contains(id));
    editor.pending_delete_ids.clear();

    for id in &outcome.deleted_ids {
        editor.preview_cache.pop(id);
        editor.remove_from_raw_cache(*id);
        editor.history_map.remove(id);
        editor.pending_loads.remove(id);
        editor.pending_raw_loads.remove(id);
    }
    editor.queued_loads.retain(|(id, _)| !deleted.contains(id));
    editor.queued_raw_loads.retain(|(id, _)| !deleted.contains(id));

    let mut nav_task = Task::none();
    if let Some(sel) = editor.selected_image_id {
        if deleted.contains(&sel) {
            editor.selected_image_id = fallback;
            if let Some(next_id) = fallback {
                nav_task = Task::done(Message::ImageSelected(next_id));
            } else {
                editor.editor_readiness = EditorReadiness::NoSelection;
                editor.working_preview = None;
                editor.rendered_preview = None;
            }
        }
    }

    editor.status = if outcome.failed.is_empty() {
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

    nav_task
}

// ── async helpers ───────────────────────────────────────────────────────

async fn hard_delete_async(targets: Vec<DeleteTarget>, db_path: PathBuf) -> HardDeleteOutcome {
    tokio::task::spawn_blocking(move || {
        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
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
            for cache in [&t.cache_thumb, &t.cache_instant, &t.cache_working]
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
                Ok(()) => match conn.execute("DELETE FROM images WHERE id = ?1", [t.id]) {
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
}
