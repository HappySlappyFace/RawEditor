//! AI models: the app's side — the confirm dialog a feature opens when its
//! model is missing, and carrying on once it is installed. Downloading and
//! removing are the session's (`session::models`).

use iced::Task;

use crate::app::message::Message;
use crate::app::state::{Modal, RawEditor};
use crate::app::update::run;
use crate::ml::models::ModelId;

/// What a feature that needed `id` does once it is installed.
fn then(id: ModelId) -> Message {
    match id {
        ModelId::Segment => Message::AiSelectStart,
        ModelId::Denoise => Message::OpenAiDenoise,
    }
}

/// A feature needs `id`: ask before downloading it, then carry on.
pub fn offer(editor: &mut RawEditor, id: ModelId) -> Task<Message> {
    editor.model_offer = Some(id);
    editor.active_modal = Modal::AiModel;
    Task::none()
}

pub fn handle_download(editor: &mut RawEditor, id: ModelId) -> Task<Message> {
    run(editor.session.download_model(id))
}

pub fn handle_downloaded(editor: &mut RawEditor, id: ModelId, result: Result<(), String>) -> Task<Message> {
    // The dialog that asked for it closes and the feature carries on.
    if editor.session.model_downloaded(id, result)
        && editor.active_modal == Modal::AiModel
        && editor.model_offer == Some(id)
    {
        editor.active_modal = Modal::None;
        editor.model_offer = None;
        return Task::done(then(id));
    }
    Task::none()
}

pub fn handle_cancel_download(editor: &mut RawEditor) -> Task<Message> {
    editor.session.cancel_model_download();
    Task::none()
}

pub fn handle_remove(editor: &mut RawEditor, id: ModelId) -> Task<Message> {
    editor.session.remove_model(id);
    Task::none()
}

/// The confirm dialog closed without installing: forget what it was for.
pub fn dismissed(editor: &mut RawEditor) {
    editor.model_offer = None;
}
