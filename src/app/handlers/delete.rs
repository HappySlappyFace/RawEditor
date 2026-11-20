// Delete Image: two modes reached from the same Delete/Backspace key.
//
// - Mark for removal (soft): reuses the existing `rating: u8` column with a
//   offset (MARKED_BASE + the stars, see database::models) instead of a new DB
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
use crate::app::state::{Modal, RawEditor};
use crate::app::update::run;
use crate::session::{DeleteRequest, HardDeleteOutcome};
use iced::Task;

/// Dispatcher for Message::DeleteImage: instant-unmark when every target is
/// already marked, otherwise opens the confirmation modal.
pub fn handle_delete_image_requested(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    if editor.session.request_delete() == DeleteRequest::Confirm {
        editor.active_modal = Modal::Delete;
    }
    Task::none()
}

pub fn handle_mark_for_removal_confirmed(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::Delete {
        return Task::none();
    }
    editor.active_modal = Modal::None;
    if editor.session.mark_pending_for_removal() {
        navigation::ensure_develop_selection_not_marked(editor)
    } else {
        Task::none()
    }
}

pub fn handle_delete_from_disk_confirmed(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::Delete || editor.session.is_deleting {
        return Task::none();
    }
    editor.active_modal = Modal::None;
    run(editor.session.delete_pending_from_disk())
}

pub fn handle_hard_delete_complete(editor: &mut RawEditor, outcome: HardDeleteOutcome) -> Task<Message> {
    let moved = editor.session.hard_delete_complete(outcome);
    navigation::apply_moved(editor, moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The doomed image can still be dirty when the background delete lands.
    /// `handle_image_selected`, which follows via `ImageSelected`, flushes
    /// against the already-moved selection: the neighbour got the deleted
    /// image's edits.
    #[test]
    fn a_hard_delete_does_not_write_the_deleted_edits_onto_the_neighbour() {
        use crate::app::message::Message;
        let dir = std::env::temp_dir().join(format!("raw-editor-del-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lib = crate::database::library::Library::open_at(&dir.join("c.db")).unwrap();
        let a = lib.import_image("/x/a.nef", "a.nef").unwrap();
        let b = lib.import_image("/x/b.nef", "b.nef").unwrap();
        let mut e = RawEditor::new().0;
        e.session.images = lib.get_all_images().unwrap();
        e.session.library = Some(lib);
        e.session.selected_image_id = Some(a);
        e.session.current_edit_params.exposure = 2.0;
        e.session.edits_dirty = true;

        let outcome = HardDeleteOutcome { deleted_ids: vec![a], failed: vec![] };
        let _ = crate::app::update::update(&mut e, Message::Session(crate::session::SessionEvent::HardDeleteComplete(outcome)));
        assert_eq!(e.session.selected_image_id, Some(b));
        let _ = crate::app::update::update(&mut e, Message::ImageMoved(b));

        let saved = e.session.library.as_ref().unwrap().load_edit_params(b).unwrap_or_default();
        assert_eq!(saved.exposure, 0.0, "the deleted image's edits landed on its neighbour");
        assert_eq!(e.session.current_edit_params.exposure, 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
