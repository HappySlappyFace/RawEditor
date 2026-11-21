//! Installing and removing the AI models (`ml::models`). Downloads run in
//! `spawn_blocking`; the UI reads their progress on a timer that only ticks
//! while one runs. (The confirm dialog a feature opens when its model is
//! missing is the app's.)

use std::sync::Arc;

use super::{Effects, Session, SessionEvent};
use crate::ml::models::ModelId;
use crate::ml::Progress;

#[derive(Debug, Default)]
pub struct ModelsState {
    /// The running download.
    pub job: Option<(ModelId, Arc<Progress>)>,
    /// The last download's failure, shown until the next attempt.
    pub error: Option<(ModelId, String)>,
}

impl Session {
    pub fn download_model(&mut self, id: ModelId) -> Effects {
        if self.models.job.is_some() {
            return Effects::none();
        }
        let progress = Arc::new(Progress::default());
        self.models.job = Some((id, progress.clone()));
        self.models.error = None;
        Effects::perform("model download", async move {
            let result = tokio::task::spawn_blocking(move || crate::ml::models::install(id, &progress))
                .await
                .unwrap_or_else(|e| Err(format!("download failed: {e}")));
            SessionEvent::ModelDownloaded(id, result)
        })
    }

    /// A download finished. `true` when `id` is now installed.
    pub fn model_downloaded(&mut self, id: ModelId, result: Result<(), String>) -> bool {
        self.models.job = None;
        match result {
            Ok(()) => {
                self.status = format!("{} installed", id.model().title);
                true
            }
            Err(e) if e == crate::ml::CANCELLED => false,
            Err(e) => {
                tracing::error!("AI model {id:?}: {e}");
                self.models.error = Some((id, e));
                false
            }
        }
    }

    pub fn cancel_model_download(&self) {
        if let Some((_, progress)) = &self.models.job {
            progress.cancel();
        }
    }

    pub fn remove_model(&mut self, id: ModelId) {
        if self.models.job.as_ref().is_some_and(|(j, _)| *j == id) {
            return;
        }
        match crate::ml::models::remove(id) {
            Ok(()) => self.status = format!("{} removed", id.model().title),
            Err(e) => self.status = e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cancelled download is neither installed nor an error to show.
    #[test]
    fn a_cancelled_download_is_not_an_error() {
        let mut s = Session::default();
        assert!(!s.model_downloaded(ModelId::Segment, Err(crate::ml::CANCELLED.to_string())));
        assert!(s.models.error.is_none());
        assert!(!s.model_downloaded(ModelId::Segment, Err("offline".into())));
        assert!(s.models.error.is_some());
        assert!(s.model_downloaded(ModelId::Segment, Ok(())));
    }
}
