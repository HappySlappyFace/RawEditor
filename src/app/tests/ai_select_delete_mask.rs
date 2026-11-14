//! Deleting a mask while an AI-select session is open must not leave the
//! session pointing at another mask.

use crate::app::handlers::ai_mask::AiSelect;
use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::app::testkit::*;
use crate::core::types::MaskParams;

/// Three masks; an AI-select session open on slot 2 (a mask it created).
fn session_on_slot_two() -> RawEditor {
    let mut e = develop_with_images(2);
    e.current_edit_params.masks[0] = MaskParams::new_ai();
    e.current_edit_params.masks[1] = MaskParams::new_ai();
    e.current_edit_params.masks[2] = MaskParams::new_ai();
    e.current_edit_params.mask_count = 3;
    e.ai_select = Some(AiSelect::for_test(1, 2, true));
    e.selected_mask = Some(2);
    e
}

#[test]
fn deleting_a_mask_below_the_session_moves_the_session_down() {
    let mut e = session_on_slot_two();
    e.current_edit_params.masks[0].ai_id = 10;
    e.current_edit_params.masks[1].ai_id = 11;
    send(&mut e, Message::DeleteMask(0));
    assert_eq!(e.current_edit_params.mask_count, 2);
    assert_eq!(e.ai_select.as_ref().map(|s| s.slot), Some(1));
    assert_eq!(e.selected_mask, Some(1));
    // Cancel removes the mask the session created (still empty), and only it.
    send(&mut e, Message::AiSelectCancel);
    assert_eq!(e.current_edit_params.mask_count, 1);
    assert_eq!(e.current_edit_params.masks[0].ai_id, 11, "cancel deleted the wrong mask");
}

#[test]
fn deleting_the_sessions_own_mask_ends_the_session() {
    let mut e = session_on_slot_two();
    send(&mut e, Message::DeleteMask(2));
    assert!(e.ai_select.is_none());
    assert_eq!(e.current_edit_params.mask_count, 2);
}

#[test]
fn deleting_a_mask_above_the_session_leaves_it_alone() {
    let mut e = session_on_slot_two();
    e.ai_select = Some(AiSelect::for_test(1, 1, false));
    send(&mut e, Message::DeleteMask(2));
    assert_eq!(e.ai_select.as_ref().map(|s| s.slot), Some(1));
}
