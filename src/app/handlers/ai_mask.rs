//! The AI-select tool, the app's side: the model download offer. The tool
//! itself is the session's (`session::ai_mask`).

use iced::Task;

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::app::update::run;
use crate::session::{Effects, ModelMissing};

/// Run a feature's start, or offer the model it needs.
pub(crate) fn or_offer(editor: &mut RawEditor, started: Result<Effects, ModelMissing>) -> Task<Message> {
    match started {
        Ok(effects) => run(effects),
        Err(ModelMissing(id)) => crate::app::handlers::models::offer(editor, id),
    }
}

/// `+ AI Select`: a new AI mask, selecting into it straight away.
pub fn handle_start(editor: &mut RawEditor) -> Task<Message> {
    let started = editor.session.ai_select_start();
    or_offer(editor, started)
}

/// `Refine` on an existing AI mask: reopen it with its prompts.
pub fn handle_refine(editor: &mut RawEditor, slot: usize) -> Task<Message> {
    let started = editor.session.ai_select_refine(slot);
    or_offer(editor, started)
}

pub fn handle_done(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.ai_select_done())
}

pub fn handle_cancel(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.ai_select_cancel())
}

#[cfg(test)]
mod tests {
    use crate::app::message::Message;
    use crate::app::testkit::*;
    use crate::core::types::{EditParams, MaskParams, MASK_LINEAR};
    use crate::ml::masks::Bitmap;
    use crate::session::{AiSelect, Selection, SessionEvent};

    fn session(image_id: i64, slot: usize, created: bool) -> AiSelect {
        AiSelect::for_test(image_id, slot, created)
    }

    /// Image 1 has an open, still empty AI-select slot 0; image 2 has a
    /// linear mask at index 0. Clicking image 2 must leave its mask alone and
    /// drop image 1's empty slot.
    #[test]
    fn leaving_an_image_mid_session_does_not_touch_the_next_images_masks() {
        let mut e = develop_with_images(2);
        let mut b = EditParams::default();
        b.masks[0] = MaskParams::new_linear(0.1, 0.1, 0.9, 0.9);
        b.mask_count = 1;
        e.session.library.as_ref().unwrap().save_edit_params(2, &b).unwrap();

        // Image 1: a new AI slot, session open.
        e.session.current_edit_params.masks[0] = MaskParams::new_ai();
        e.session.current_edit_params.mask_count = 1;
        e.session.ai_select = Some(session(1, 0, true));

        send(&mut e, Message::ImageSelected(2));

        assert!(e.session.ai_select.is_none());
        assert_eq!(e.session.selected_image_id, Some(2));
        assert_eq!(e.session.current_edit_params.mask_count, 1, "image 2's mask was deleted");
        assert_eq!(e.session.current_edit_params.masks[0].mask_type, MASK_LINEAR);
        assert_eq!(saved_edits(&e, 2).mask_count, 1);
        assert_eq!(saved_edits(&e, 1).mask_count, 0, "image 1 kept an empty AI slot");
    }

    /// A decoder answer must not be written into a slot that is no longer an
    /// AI mask (the list shifted under the session).
    #[test]
    fn a_selection_is_not_written_into_a_non_ai_slot() {
        let mut e = develop_with_images(1);
        e.session.current_edit_params.masks[0] = MaskParams::new_linear(0.1, 0.1, 0.9, 0.9);
        e.session.current_edit_params.mask_count = 1;
        let s = session(1, 0, false);
        let token = s.token();
        e.session.ai_select = Some(s);
        let sel = Selection::for_test(42, Bitmap { width: 1, height: 1, data: vec![255] });
        send(&mut e, Message::Session(SessionEvent::AiMaskReady(token, Ok(sel))));
        assert_eq!(e.session.current_edit_params.masks[0].ai_id, 0);
    }
}
