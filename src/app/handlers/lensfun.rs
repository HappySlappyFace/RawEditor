//! Lens-profile database: download, update check and rescan
//! (`core::lensfun`), from Preferences › Lens Profiles and from the Lens
//! Corrections panel when no database is installed.
//!
//! An image's lens profile is looked up when its RAW is read, so whenever
//! the database changes the selected image is decoded again and the raw
//! cache is dropped — otherwise the panel would keep saying "no profile"
//! until the app restarted.

use iced::Task;

use crate::app::message::Message;
use crate::app::state::{LensfunJob, RawEditor};
use crate::core::lens::LensDb;

/// Run blocking `work` off the UI thread and report it as `done`.
fn background<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl Fn(Result<T, String>) -> Message + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(work)
                .await
                .unwrap_or_else(|e| Err(format!("background task failed: {e}")))
        },
        done,
    )
}

pub fn handle_check(editor: &mut RawEditor) -> Task<Message> {
    if editor.lensfun.busy.is_some() {
        return Task::none();
    }
    editor.lensfun.busy = Some(LensfunJob::Checking);
    background(crate::core::lensfun::check, Message::LensfunChecked)
}

pub fn handle_checked(
    editor: &mut RawEditor,
    result: Result<crate::core::lensfun::Remote, String>,
) -> Task<Message> {
    editor.lensfun.busy = None;
    editor.lensfun.remote = Some(result);
    Task::none()
}

pub fn handle_download(editor: &mut RawEditor) -> Task<Message> {
    if editor.lensfun.busy.is_some() {
        return Task::none();
    }
    editor.lensfun.busy = Some(LensfunJob::Downloading);
    editor.lensfun.installed = None;
    background(
        || {
            let installed = crate::core::lensfun::download_latest()?;
            // Swap the new database in here, off the UI thread.
            LensDb::reload();
            Ok(installed)
        },
        Message::LensfunInstalled,
    )
}

pub fn handle_installed(
    editor: &mut RawEditor,
    result: Result<crate::core::lensfun::Installed, String>,
) -> Task<Message> {
    editor.lensfun.busy = None;
    let ok = result.is_ok();
    match &result {
        Ok(i) => {
            editor.status = format!(
                "Lens profiles installed: {} lenses, published {}",
                i.lenses,
                crate::core::lensfun::format_date(i.timestamp)
            );
            // What was checked is now what is installed.
            editor.lensfun.remote = None;
        }
        Err(e) => {
            tracing::error!("Lens profile download failed: {e}");
            editor.status = format!("Lens profile download failed: {e}");
        }
    }
    editor.lensfun.installed = Some(result);
    if ok {
        database_changed(editor)
    } else {
        Task::none()
    }
}

pub fn handle_rescan(editor: &mut RawEditor) -> Task<Message> {
    if editor.lensfun.busy.is_some() {
        return Task::none();
    }
    editor.lensfun.busy = Some(LensfunJob::Rescanning);
    background(
        || {
            LensDb::reload();
            Ok(())
        },
        |_| Message::LensDbLoaded(true),
    )
}

/// Load the database for display if nothing has needed it yet (it is read
/// lazily, by the first RAW decode). The Lens Profiles page asks on opening.
pub fn ensure_loaded() -> Task<Message> {
    if LensDb::loaded().is_some() {
        return Task::none();
    }
    background(
        || {
            LensDb::global();
            Ok(())
        },
        |_| Message::LensDbLoaded(false),
    )
}

pub fn handle_db_loaded(editor: &mut RawEditor, changed: bool) -> Task<Message> {
    if editor.lensfun.busy == Some(LensfunJob::Rescanning) {
        editor.lensfun.busy = None;
    }
    if changed {
        database_changed(editor)
    } else {
        Task::none()
    }
}

/// The database changed: decoded images carry the old lookup.
fn database_changed(editor: &mut RawEditor) -> Task<Message> {
    editor.raw_cache.clear();
    editor.current_raw = None;
    editor.raw_cache_bytes = 0;
    crate::app::handlers::navigation::reload_selected_image(editor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::message::AppTab;
    use crate::app::state::EditorReadiness;
    use crate::core::lensfun::Installed;

    fn installed() -> Installed {
        Installed { timestamp: 1_791_482_221, files: 56, lenses: 1472, cameras: 1057 }
    }

    /// One job at a time: a second click while one runs does nothing.
    #[test]
    fn one_lens_job_at_a_time() {
        let (mut e, _) = RawEditor::new();
        let _ = handle_check(&mut e);
        assert_eq!(e.lensfun.busy, Some(LensfunJob::Checking));
        let _ = handle_download(&mut e);
        let _ = handle_rescan(&mut e);
        assert_eq!(e.lensfun.busy, Some(LensfunJob::Checking));
        let _ = handle_checked(&mut e, Err("offline".into()));
        assert_eq!(e.lensfun.busy, None);
        assert!(matches!(e.lensfun.remote, Some(Err(_))));
    }

    /// A new database means the selected image's lens lookup is stale: it is
    /// decoded again when Develop next shows it.
    #[test]
    fn an_installed_database_redecodes_the_selected_image() {
        let (mut e, _) = RawEditor::new();
        e.selected_image_id = Some(3);
        e.current_tab = AppTab::Library;
        e.editor_readiness = EditorReadiness::Ready(3);
        e.lensfun.busy = Some(LensfunJob::Downloading);
        let _ = handle_installed(&mut e, Ok(installed()));
        assert_eq!(e.lensfun.busy, None);
        assert!(matches!(e.lensfun.installed, Some(Ok(_))));
        assert!(matches!(e.editor_readiness, EditorReadiness::NoSelection), "kept the stale decode");
    }

    /// A failed download leaves everything as it was, and says why.
    #[test]
    fn a_failed_download_changes_nothing() {
        let (mut e, _) = RawEditor::new();
        e.selected_image_id = Some(3);
        e.editor_readiness = EditorReadiness::Ready(3);
        let _ = handle_installed(&mut e, Err("no route to host".into()));
        assert!(matches!(e.editor_readiness, EditorReadiness::Ready(3)));
        assert!(e.status.contains("no route to host"));
    }
}
