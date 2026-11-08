//! Installing and removing the AI models (`ml::models`), from Preferences ›
//! AI Models or from the confirm dialog a feature opens when its model is
//! missing. Downloads run in `spawn_blocking`; the UI reads their progress on
//! a timer that only ticks while one runs (`RawEditor::subscription`).

use std::sync::Arc;

use iced::Task;

use crate::app::message::Message;
use crate::app::state::{Modal, RawEditor};
use crate::ml::models::ModelId;
use crate::ml::Progress;

#[derive(Debug, Default)]
pub struct ModelsState {
    /// The model the confirm dialog offers.
    pub offered: Option<ModelId>,
    /// What to do once it is installed (the action that needed it).
    pub then: Option<Message>,
    /// The running download.
    pub job: Option<(ModelId, Arc<Progress>)>,
    /// The last download's failure, shown until the next attempt.
    pub error: Option<(ModelId, String)>,
}

/// A feature needs `id`: ask before downloading it, then carry on.
pub fn offer(editor: &mut RawEditor, id: ModelId) -> Task<Message> {
    let then = match id {
        ModelId::Segment => Message::AiSelectStart,
        ModelId::Denoise => Message::OpenAiDenoise,
    };
    editor.models.offered = Some(id);
    editor.models.then = Some(then);
    editor.active_modal = Modal::AiModel;
    Task::none()
}

pub fn handle_download(editor: &mut RawEditor, id: ModelId) -> Task<Message> {
    if editor.models.job.is_some() {
        return Task::none();
    }
    let progress = Arc::new(Progress::default());
    editor.models.job = Some((id, progress.clone()));
    editor.models.error = None;
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || crate::ml::models::install(id, &progress))
                .await
                .unwrap_or_else(|e| Err(format!("download failed: {e}")))
        },
        move |result| Message::ModelDownloaded(id, result),
    )
}

pub fn handle_downloaded(editor: &mut RawEditor, id: ModelId, result: Result<(), String>) -> Task<Message> {
    editor.models.job = None;
    match result {
        Ok(()) => {
            editor.status = format!("{} installed", id.model().title);
            // The dialog that asked for it closes and the feature carries on.
            if editor.active_modal == Modal::AiModel && editor.models.offered == Some(id) {
                editor.active_modal = Modal::None;
                editor.models.offered = None;
                if let Some(then) = editor.models.then.take() {
                    return Task::done(then);
                }
            }
            Task::none()
        }
        Err(e) if e == crate::ml::CANCELLED => Task::none(),
        Err(e) => {
            tracing::error!("AI model {id:?}: {e}");
            editor.models.error = Some((id, e));
            Task::none()
        }
    }
}

pub fn handle_cancel_download(editor: &mut RawEditor) -> Task<Message> {
    if let Some((_, progress)) = &editor.models.job {
        progress.cancel();
    }
    Task::none()
}

pub fn handle_remove(editor: &mut RawEditor, id: ModelId) -> Task<Message> {
    if editor.models.job.as_ref().is_some_and(|(j, _)| *j == id) {
        return Task::none();
    }
    match crate::ml::models::remove(id) {
        Ok(()) => editor.status = format!("{} removed", id.model().title),
        Err(e) => editor.status = e,
    }
    Task::none()
}

/// The confirm dialog closed without installing: forget what it was for.
pub fn dismissed(editor: &mut RawEditor) {
    editor.models.offered = None;
    editor.models.then = None;
}
