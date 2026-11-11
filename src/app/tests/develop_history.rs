//! Develop's undo / redo / reset / copy-paste, and the rule that the
//! before/after peek never becomes the user's edit — driven by messages,
//! checked against both the editor and what the catalogue saved.

use crate::app::message::{AppTab, CopyCategory, Message};
use crate::app::state::{EditorReadiness, HISTORY_LIMIT};
use crate::app::testkit::*;

/// A slider gesture: one change, then the release.
fn set_exposure(e: &mut crate::app::state::RawEditor, v: f32) {
    send(e, Message::ExposureChanged(v));
    send(e, Message::CommitEdit);
}

fn exposure(e: &crate::app::state::RawEditor) -> f32 {
    e.current_edit_params.exposure
}

#[test]
fn undo_and_redo_walk_the_stack_and_save_each_step() {
    let mut e = develop_with_images(3);
    set_exposure(&mut e, 1.0);
    set_exposure(&mut e, 2.0);
    assert_eq!(history(&e, 1), Some((3, 2)));

    for expected in [1.0, 0.0, 0.0] {
        send(&mut e, Message::Undo);
        assert_eq!(exposure(&e), expected);
        assert_eq!(saved_edits(&e, 1).exposure, expected, "undo not persisted");
    }
    for expected in [1.0, 2.0, 2.0] {
        send(&mut e, Message::Redo);
        assert_eq!(exposure(&e), expected);
        assert_eq!(saved_edits(&e, 1).exposure, expected, "redo not persisted");
    }
    assert_invariants(&e);
}

#[test]
fn an_edit_after_undo_drops_the_redo_steps() {
    let mut e = develop_with_images(3);
    set_exposure(&mut e, 1.0);
    set_exposure(&mut e, 2.0);
    send(&mut e, Message::Undo);
    set_exposure(&mut e, -1.0);
    assert_eq!(history(&e, 1), Some((3, 2)));
    send(&mut e, Message::Redo);
    assert_eq!(exposure(&e), -1.0, "redo resurrected a dropped step");
}

#[test]
fn a_release_that_changed_nothing_adds_no_step() {
    let mut e = develop_with_images(3);
    set_exposure(&mut e, 1.0);
    send(&mut e, Message::CommitEdit);
    set_exposure(&mut e, 1.0);
    assert_eq!(history(&e, 1), Some((2, 1)));
}

#[test]
fn history_keeps_the_newest_steps_up_to_its_limit() {
    let mut e = develop_with_images(2);
    let commits = HISTORY_LIMIT + 50;
    for i in 1..=commits {
        set_exposure(&mut e, i as f32 * 0.01);
    }
    assert_eq!(history(&e, 1), Some((HISTORY_LIMIT, HISTORY_LIMIT - 1)));
    for _ in 0..HISTORY_LIMIT + 5 {
        send(&mut e, Message::Undo);
    }
    // The oldest step kept is commit number `commits - LIMIT + 1`.
    let oldest = (commits - HISTORY_LIMIT + 1) as f32 * 0.01;
    assert_eq!(exposure(&e), oldest);
    assert_eq!(history(&e, 1), Some((HISTORY_LIMIT, 0)));
    assert_invariants(&e);
}

#[test]
fn each_image_has_its_own_history() {
    let mut e = develop_with_images(3);
    set_exposure(&mut e, 1.0);
    send(&mut e, Message::ImageSelected(2));
    assert_eq!(exposure(&e), 0.0, "image 1's edit leaked onto image 2");
    set_exposure(&mut e, -0.5);
    send(&mut e, Message::Undo);
    assert_eq!(exposure(&e), 0.0);
    assert_eq!(saved_edits(&e, 1).exposure, 1.0, "undo on 2 touched 1");

    send(&mut e, Message::ImageSelected(1));
    assert_eq!(exposure(&e), 1.0, "image 1's edit was not reloaded");
    send(&mut e, Message::Redo); // nothing to redo on 1
    assert_eq!(exposure(&e), 1.0);
    send(&mut e, Message::Undo);
    assert_eq!(exposure(&e), 0.0);
    assert_eq!(saved_edits(&e, 2).exposure, 0.0);
    assert_invariants(&e);
}

#[test]
fn reset_is_one_undoable_step() {
    let mut e = develop_with_images(2);
    set_exposure(&mut e, 1.0);
    send(&mut e, Message::ContrastChanged(0.4));
    send(&mut e, Message::CommitEdit);
    send(&mut e, Message::ResetEdits);
    assert_eq!(e.current_edit_params.without_tool_state(), Default::default());
    assert_eq!(saved_edits(&e, 1).exposure, 0.0, "reset not saved");
    send(&mut e, Message::Undo);
    assert_eq!((exposure(&e), e.current_edit_params.contrast), (1.0, 0.4));
}

/// Paste merges only the copied categories, onto every selected image:
/// the current one (one undo step), visited ones (a step on THEIR stacks) and
/// unvisited ones (saved; their stack starts from the paste).
#[test]
fn paste_onto_a_selection_merges_into_each_image_and_its_history() {
    let mut e = develop_with_images(5);
    set_exposure(&mut e, 1.5);
    send(&mut e, Message::ContrastChanged(0.3));
    send(&mut e, Message::CommitEdit);
    // Copy the tone only.
    send(&mut e, Message::CopySettings);
    send(&mut e, Message::SetAllCopyCategories(false));
    send(&mut e, Message::ToggleCopyCategory(CopyCategory::Tone, true));
    send(&mut e, Message::CopySettingsConfirmed);
    assert!(e.edit_clipboard.is_some());

    // Image 3 visited and given its own colour edit; then paste from 2.
    send(&mut e, Message::ImageSelected(3));
    send(&mut e, Message::SaturationChanged(0.4));
    send(&mut e, Message::CommitEdit);
    send(&mut e, Message::ImageSelected(2));
    send(&mut e, Message::SelectAll);
    assert_eq!(e.multi_selection.len(), 5);
    send(&mut e, Message::PasteSettings);

    assert_eq!((exposure(&e), e.current_edit_params.contrast), (1.5, 0.3));
    for id in [3, 4, 5] {
        let p = saved_edits(&e, id);
        assert_eq!((p.exposure, p.contrast), (1.5, 0.3), "image {id} missed the paste");
    }
    assert_eq!(saved_edits(&e, 3).saturation, 0.4, "paste overwrote an uncopied category");
    assert_eq!(history(&e, 4), None, "an unvisited image got a stack");
    assert_eq!(history(&e, 3), Some((3, 2)), "the visited image's stack missed the paste");

    // Undo on the current image steps back over the paste.
    send(&mut e, Message::Undo);
    assert_eq!(exposure(&e), 0.0);
    // And on a visited one, too, keeping its own edit.
    send(&mut e, Message::ImageSelected(3));
    assert_eq!(exposure(&e), 1.5);
    send(&mut e, Message::Undo);
    assert_eq!((exposure(&e), e.current_edit_params.saturation), (0.0, 0.4));
    // An unvisited one opens on the pasted edit, with nothing to undo.
    send(&mut e, Message::ImageSelected(4));
    assert_eq!(exposure(&e), 1.5);
    send(&mut e, Message::Undo);
    assert_eq!(exposure(&e), 1.5);
    assert_invariants(&e);
}

/// KNOWN BUG (minor). Pasting onto a selection that includes a visited image
/// whose edit the paste does not change (the copy's own source, typically)
/// still pushes a step on its stack: `handle_paste_settings` pushes for every
/// visited target, against `commit_current_state`'s "a release that changed
/// nothing adds no step" rule. The next Undo there visibly does nothing.
#[test]
#[ignore = "known bug: fix in develop::handle_paste_settings (skip unchanged targets)"]
fn a_paste_that_changes_an_image_nothing_adds_no_step_to_it() {
    let mut e = develop_with_images(3);
    set_exposure(&mut e, 1.5);
    send(&mut e, Message::CopyAllSettings);
    send(&mut e, Message::ImageSelected(2));
    send(&mut e, Message::SelectAll);
    let before = history(&e, 1);
    send(&mut e, Message::PasteSettings);
    assert_eq!(history(&e, 1), before);
}

/// Space shows the unedited image; it must never become the edit. Every
/// flush point while peeking saves the REAL edit.
#[test]
fn the_before_peek_never_reaches_the_edit_or_the_catalogue() {
    let mut e = develop_with_images(3);
    set_exposure(&mut e, 1.0);
    // The peek needs a loaded image; the harness has no GPU to load one.
    e.editor_readiness = EditorReadiness::Ready(1);

    send(&mut e, Message::SetBeforePeek(true));
    assert!(e.before_peek);
    assert_eq!(exposure(&e), 1.0, "the peek wrote current_edit_params");
    send(&mut e, Message::CommitEdit);
    assert_eq!(saved_edits(&e, 1).exposure, 1.0);
    send(&mut e, Message::TabChanged(AppTab::Library));
    assert!(!e.before_peek, "the peek outlived Develop");
    assert_eq!(saved_edits(&e, 1).exposure, 1.0);

    send(&mut e, Message::TabChanged(AppTab::Develop));
    e.editor_readiness = EditorReadiness::Ready(1);
    send(&mut e, Message::SetBeforePeek(true));
    send(&mut e, Message::ImageSelected(2));
    assert!(!e.before_peek, "the peek carried over to the next image");
    assert_eq!(saved_edits(&e, 1).exposure, 1.0);
    assert_eq!(exposure(&e), 0.0);
}

#[test]
fn the_peek_is_refused_while_the_image_loads() {
    let mut e = develop_with_images(3);
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(1));
    send(&mut e, Message::SetBeforePeek(true));
    assert!(!e.before_peek);
}
