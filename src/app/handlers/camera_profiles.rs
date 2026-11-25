//! Camera profiles: the app's side — the per-image choice (an edit), the
//! per-camera default (a saved setting) and the file picker. Downloads,
//! imports, updates and rescans are the session's (`session::camera_profiles`).
//!
//! A profile is applied when an image's GPU resources are built, so a change
//! that could alter which profile the open image resolves to rebuilds them
//! (`Session::camera_profile_stale`, [`rebuild`]).

use iced::Task;

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::app::update::run;
use crate::core::camera_profiles::ProfileChoice;
use crate::session::Session;

/// Rebuild the open image so it picks its profile again.
pub fn rebuild(editor: &mut RawEditor) -> Task<Message> {
    crate::app::handlers::navigation::reload_selected_image(editor)
}

pub fn handle_selected(editor: &mut RawEditor, choice: ProfileChoice) -> Task<Message> {
    if editor.session.current_edit_params.camera_profile == choice {
        return Task::none();
    }
    editor.session.current_edit_params.camera_profile = choice;
    // An edit like any other: undoable, saved now.
    editor.session.mark_edits_dirty();
    editor.session.commit_current_state();
    rebuild(editor)
}

pub fn handle_set_default(editor: &mut RawEditor, camera: String, value: Option<String>) -> Task<Message> {
    let next = editor.session.set_camera_profile_default(camera, value);
    editor.save_preferences();
    run(next)
}

pub fn handle_download(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.download_camera_profile())
}

pub fn handle_import(editor: &mut RawEditor) -> Task<Message> {
    if editor.session.camera_profiles.busy.is_some() {
        return Task::none();
    }
    // Async, like every picker in the app: the blocking one would freeze the
    // window while the dialog is open.
    Task::perform(
        async {
            rfd::AsyncFileDialog::new()
                .set_title("Import Camera Profiles")
                .add_filter("DNG camera profile", &["dcp", "DCP"])
                .pick_files()
                .await
                .map(|files| files.into_iter().map(|f| f.path().to_path_buf()).collect())
        },
        Message::CameraProfilesPicked,
    )
}

pub fn handle_picked(editor: &mut RawEditor, files: Option<Vec<std::path::PathBuf>>) -> Task<Message> {
    run(editor.session.import_camera_profiles(files.unwrap_or_default()))
}

pub fn handle_update(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.update_camera_profiles())
}

pub fn handle_rescan(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.rescan_camera_profiles())
}

/// Scan the folders for display if nothing has needed them yet (the
/// Camera Profiles page asks on opening).
pub fn ensure_loaded() -> Task<Message> {
    run(Session::camera_profiles_ensure_loaded())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ProfileJob;

    /// One job at a time: a second click while one runs does nothing.
    #[test]
    fn one_profile_job_at_a_time() {
        let (mut e, _) = RawEditor::new();
        let _ = handle_rescan(&mut e);
        assert_eq!(e.session.camera_profiles.busy, Some(ProfileJob::Rescanning));
        let _ = handle_update(&mut e);
        assert_eq!(e.session.camera_profiles.busy, Some(ProfileJob::Rescanning));
        let _ = crate::app::update::update(&mut e, Message::Session(crate::session::SessionEvent::CameraProfilesScanned(false)));
        assert_eq!(e.session.camera_profiles.busy, None);
    }

    /// Choosing a profile is an edit: it lands in the params, unchanged
    /// choices do nothing.
    #[test]
    fn choosing_a_profile_is_an_edit() {
        let (mut e, _) = RawEditor::new();
        let choice = ProfileChoice::named("Nikon D3300 Camera Standard.dcp").unwrap();
        let _ = handle_selected(&mut e, choice);
        assert_eq!(e.session.current_edit_params.camera_profile, choice);
        let _ = handle_selected(&mut e, choice);
        assert_eq!(e.session.current_edit_params.camera_profile, choice);
    }

    /// A camera RawTherapee doesn't cover is remembered, so the prompt can
    /// say so instead of offering the same dead end.
    #[test]
    fn a_missing_remote_profile_is_reported() {
        let (mut e, _) = RawEditor::new();
        e.session.camera_profiles.busy = Some(ProfileJob::Downloading);
        // The name comes from the request, not from whichever image is open
        // when the download finishes.
        let _ = crate::app::update::update(&mut e, Message::Session(crate::session::SessionEvent::CameraProfileDownloaded("Nikon D3300".into(), Ok(None))));
        assert_eq!(e.session.camera_profiles.busy, None);
        assert_eq!(e.session.camera_profiles.unavailable.as_deref(), Some("Nikon D3300"));
        assert!(matches!(e.session.camera_profiles.notice, Some(Err(_))));
    }

    /// Defaults are saved per camera; `None` clears one back to automatic.
    #[test]
    fn camera_defaults_set_and_clear() {
        let (mut e, _) = RawEditor::new();
        let _ = handle_set_default(&mut e, "NIKON D3300".into(), Some("builtin".into()));
        assert_eq!(e.session.camera_profile_defaults.get("NIKON D3300").map(String::as_str), Some("builtin"));
        let _ = handle_set_default(&mut e, "NIKON D3300".into(), None);
        assert!(!e.session.camera_profile_defaults.contains_key("NIKON D3300"));
    }
}
