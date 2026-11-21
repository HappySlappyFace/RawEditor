//! AI denoise, the app's side: the dialog. The denoising is the session's
//! (`session::ai_denoise`); the dialog is open exactly while the session's
//! `ai_denoise.image_id` is set (`update::update` keeps them together).

use iced::Task;

use crate::app::message::Message;
use crate::app::state::{Modal, RawEditor};
use crate::app::update::run;

/// `AI Denoise…`: open the dialog on the selected image.
pub fn handle_open(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    let opened = editor.session.open_ai_denoise();
    if editor.session.ai_denoise.image_id.is_some() {
        editor.active_modal = Modal::AiDenoise;
    }
    crate::app::handlers::ai_mask::or_offer(editor, opened)
}

pub fn handle_apply(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.apply_ai_denoise())
}

pub fn handle_cancel(editor: &mut RawEditor) -> Task<Message> {
    editor.session.cancel_ai_denoise();
    Task::none()
}

#[cfg(test)]
mod tests {
    use crate::app::message::Message;
    use crate::app::state::{Modal, RawEditor};
    use crate::app::update::update;

    fn open_dialog() -> RawEditor {
        let mut e = RawEditor::new().0;
        e.session.ai_denoise.image_id = Some(1);
        e.active_modal = Modal::AiDenoise;
        e
    }

    /// The dialog is open exactly while the session has it open, whichever
    /// side closes it.
    #[test]
    fn the_dialog_and_the_session_close_together() {
        // The session closes it: Cancel with no job running.
        let mut e = open_dialog();
        let _ = update(&mut e, Message::AiDenoiseCancel);
        assert!(e.session.ai_denoise.image_id.is_none());
        assert_eq!(e.active_modal, Modal::None, "the dialog outlived its session");

        // The dialog closes first (Escape): its engine goes with it.
        let mut e = open_dialog();
        let _ = update(&mut e, Message::CloseModal);
        assert_eq!(e.active_modal, Modal::None);
        assert!(e.session.ai_denoise.image_id.is_none(), "the session outlived its dialog");
    }
}
