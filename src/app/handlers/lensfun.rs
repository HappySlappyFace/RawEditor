//! Lens-profile database: the app's side. The jobs are the session's
//! (`session::lensfun`); a changed database reloads the open image, which is
//! still the app's to do.

use iced::Task;

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::app::update::{follow_up, run};
use crate::session::Session;

pub fn handle_check(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.lensfun_check())
}

pub fn handle_checked(
    editor: &mut RawEditor,
    result: Result<crate::core::lensfun::Remote, String>,
) -> Task<Message> {
    editor.session.lensfun_checked(result);
    Task::none()
}

pub fn handle_download(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.lensfun_download())
}

pub fn handle_installed(
    editor: &mut RawEditor,
    result: Result<crate::core::lensfun::Installed, String>,
) -> Task<Message> {
    let next = editor.session.lensfun_installed(result);
    follow_up(editor, next)
}

pub fn handle_rescan(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.lensfun_rescan())
}

/// Load the database for display if nothing has needed it yet. The Lens
/// Profiles page asks on opening.
pub fn ensure_loaded() -> Task<Message> {
    run(Session::lens_db_ensure_loaded())
}

pub fn handle_db_loaded(editor: &mut RawEditor, changed: bool) -> Task<Message> {
    let next = editor.session.lens_db_loaded(changed);
    follow_up(editor, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::message::AppTab;
    use crate::app::state::EditorReadiness;
    use crate::session::LensfunJob;
    use crate::core::lensfun::Installed;

    fn installed() -> Installed {
        Installed { timestamp: 1_791_482_221, files: 56, lenses: 1472, cameras: 1057 }
    }

    /// One job at a time: a second click while one runs does nothing.
    #[test]
    fn one_lens_job_at_a_time() {
        let (mut e, _) = RawEditor::new();
        let _ = handle_check(&mut e);
        assert_eq!(e.session.lensfun.busy, Some(LensfunJob::Checking));
        let _ = handle_download(&mut e);
        let _ = handle_rescan(&mut e);
        assert_eq!(e.session.lensfun.busy, Some(LensfunJob::Checking));
        let _ = handle_checked(&mut e, Err("offline".into()));
        assert_eq!(e.session.lensfun.busy, None);
        assert!(matches!(e.session.lensfun.remote, Some(Err(_))));
    }

    /// A new database means the selected image's lens lookup is stale: it is
    /// decoded again when Develop next shows it.
    #[test]
    fn an_installed_database_redecodes_the_selected_image() {
        let (mut e, _) = RawEditor::new();
        e.session.selected_image_id = Some(3);
        e.session.current_tab = AppTab::Library;
        e.session.editor_readiness = EditorReadiness::Ready(3);
        e.session.lensfun.busy = Some(LensfunJob::Downloading);
        let _ = handle_installed(&mut e, Ok(installed()));
        assert_eq!(e.session.lensfun.busy, None);
        assert!(matches!(e.session.lensfun.installed, Some(Ok(_))));
        assert!(matches!(e.session.editor_readiness, EditorReadiness::NoSelection), "kept the stale decode");
    }

    /// A failed download leaves everything as it was, and says why.
    #[test]
    fn a_failed_download_changes_nothing() {
        let (mut e, _) = RawEditor::new();
        e.session.selected_image_id = Some(3);
        e.session.editor_readiness = EditorReadiness::Ready(3);
        let _ = handle_installed(&mut e, Err("no route to host".into()));
        assert!(matches!(e.session.editor_readiness, EditorReadiness::Ready(3)));
        assert!(e.session.status.contains("no route to host"));
    }
}
