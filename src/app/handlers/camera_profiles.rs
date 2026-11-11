//! Camera profiles (`core::camera_profiles`): the per-image choice from
//! Develop's dropdown, the per-camera default from Preferences, and getting
//! more — RawTherapee's open-source profiles, imported files, updates.
//!
//! A profile is applied when an image's GPU resources are built, so any
//! change that could alter which profile the open image resolves to —
//! choice, default, or the installed set — rebuilds them
//! ([`resources_stale`], [`rebuild`]). From the cached RAW when there is
//! one: no new decode.

use iced::Task;

use crate::app::message::Message;
use crate::app::state::{ProfileJob, RawEditor};
use crate::core::camera_profiles::{self as profiles, Catalog, ProfileChoice};

/// Run blocking `work` off the UI thread and report it with `done`.
fn background<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl Fn(Result<T, String>) -> Message + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move { tokio::task::spawn_blocking(work).await.map_err(|e| format!("background task failed: {e}")) },
        done,
    )
}

/// The open image's camera, as `(make, model)`.
fn camera(editor: &RawEditor) -> Option<(String, String)> {
    editor.current_metadata.as_ref().map(|m| (m.make.clone(), m.model.clone()))
}

/// Were the open image's GPU resources built with a different profile
/// choice than it now has (its own, or its camera's default)?
pub fn resources_stale(editor: &RawEditor) -> bool {
    let (Some(resources), Some((make, model))) = (&editor.image_resources, camera(editor)) else {
        return false;
    };
    let Some(used) = &resources.profile_use else { return false };
    used.is_stale(editor.current_edit_params.camera_profile, &editor.camera_profile_defaults, &make, &model)
}

/// Rebuild the open image so it picks its profile again.
pub fn rebuild(editor: &mut RawEditor) -> Task<Message> {
    crate::app::handlers::navigation::reload_selected_image(editor)
}

pub fn handle_selected(editor: &mut RawEditor, choice: ProfileChoice) -> Task<Message> {
    if editor.current_edit_params.camera_profile == choice {
        return Task::none();
    }
    editor.current_edit_params.camera_profile = choice;
    // An edit like any other: undoable, saved now.
    editor.mark_edits_dirty();
    editor.commit_current_state();
    rebuild(editor)
}

pub fn handle_set_default(editor: &mut RawEditor, camera: String, value: Option<String>) -> Task<Message> {
    match value {
        Some(v) => editor.camera_profile_defaults.insert(camera, v),
        None => editor.camera_profile_defaults.remove(&camera),
    };
    editor.save_preferences();
    if resources_stale(editor) {
        rebuild(editor)
    } else {
        Task::none()
    }
}

/// The installed set changed: the open image may now resolve differently.
fn catalog_changed(editor: &mut RawEditor) -> Task<Message> {
    if editor.image_resources.is_some() {
        rebuild(editor)
    } else {
        Task::none()
    }
}

fn start(editor: &mut RawEditor, job: ProfileJob) -> bool {
    if editor.camera_profiles.busy.is_some() {
        return false;
    }
    editor.camera_profiles.busy = Some(job);
    editor.camera_profiles.notice = None;
    true
}

pub fn handle_download(editor: &mut RawEditor) -> Task<Message> {
    let Some((make, model)) = camera(editor) else { return Task::none() };
    if !start(editor, ProfileJob::Downloading) {
        return Task::none();
    }
    let camera_name = profiles::camera_name(&make, &model);
    background(
        move || {
            let result = profiles::download_for_camera(&make, &model);
            if matches!(result, Ok(Some(_))) {
                Catalog::rescan();
            }
            result
        },
        move |r| Message::CameraProfileDownloaded(camera_name.clone(), r.and_then(|inner| inner)),
    )
}

pub fn handle_downloaded(editor: &mut RawEditor, camera_name: String, result: Result<Option<String>, String>) -> Task<Message> {
    editor.camera_profiles.busy = None;
    match result {
        Ok(Some(file)) => {
            editor.status = format!("Camera profile downloaded: {file}");
            editor.camera_profiles.notice = Some(Ok(format!("Downloaded {file}")));
            editor.camera_profiles.unavailable = None;
            catalog_changed(editor)
        }
        Ok(None) => {
            let name = camera_name;
            editor.camera_profiles.notice = Some(Err(format!("RawTherapee has no profile for {name}")));
            editor.camera_profiles.unavailable = Some(name);
            Task::none()
        }
        Err(e) => {
            tracing::error!("Camera profile download failed: {e}");
            editor.camera_profiles.notice = Some(Err(format!("Download failed: {e}")));
            Task::none()
        }
    }
}

pub fn handle_import(editor: &mut RawEditor) -> Task<Message> {
    if editor.camera_profiles.busy.is_some() {
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
    let Some(files) = files.filter(|f| !f.is_empty()) else { return Task::none() };
    if !start(editor, ProfileJob::Importing) {
        return Task::none();
    }
    background(
        move || {
            let outcome = profiles::import(&files);
            if !outcome.0.is_empty() {
                Catalog::rescan();
            }
            outcome
        },
        |r| match r {
            Ok((ok, failed)) => Message::CameraProfilesImported(ok, failed),
            Err(e) => Message::CameraProfilesImported(Vec::new(), vec![e]),
        },
    )
}

pub fn handle_imported(editor: &mut RawEditor, ok: Vec<String>, failed: Vec<String>) -> Task<Message> {
    editor.camera_profiles.busy = None;
    editor.camera_profiles.notice = Some(match (ok.len(), failed.is_empty()) {
        (_, false) => Err(format!(
            "Imported {} — could not import: {}",
            ok.len(),
            failed.join("; ")
        )),
        (1, true) => Ok(format!("Imported {}", ok[0])),
        (n, true) => Ok(format!("Imported {n} profiles")),
    });
    if ok.is_empty() {
        Task::none()
    } else {
        catalog_changed(editor)
    }
}

pub fn handle_update(editor: &mut RawEditor) -> Task<Message> {
    if !start(editor, ProfileJob::Updating) {
        return Task::none();
    }
    background(
        || {
            let report = profiles::update_downloaded();
            if report.as_ref().is_ok_and(|r| !r.updated.is_empty()) {
                Catalog::rescan();
            }
            report
        },
        |r| Message::CameraProfilesUpdated(r.and_then(|inner| inner)),
    )
}

pub fn handle_updated(
    editor: &mut RawEditor,
    result: Result<profiles::UpdateReport, String>,
) -> Task<Message> {
    editor.camera_profiles.busy = None;
    let changed = result.as_ref().is_ok_and(|r| !r.updated.is_empty());
    editor.camera_profiles.notice = Some(match result {
        Ok(r) if r.checked == 0 => Ok("No downloaded profiles to update".to_string()),
        Ok(r) if r.updated.is_empty() => Ok(format!(
            "All {} downloaded profile{} up to date",
            r.checked,
            if r.checked == 1 { " is" } else { "s are" }
        )),
        Ok(r) => Ok(format!("Updated {}", r.updated.join(", "))),
        Err(e) => Err(format!("Could not check for updates: {e}")),
    });
    if changed {
        catalog_changed(editor)
    } else {
        Task::none()
    }
}

pub fn handle_rescan(editor: &mut RawEditor) -> Task<Message> {
    if !start(editor, ProfileJob::Rescanning) {
        return Task::none();
    }
    background(
        || {
            Catalog::rescan();
        },
        |_| Message::CameraProfilesScanned(true),
    )
}

/// Scan the folders for display if nothing has needed them yet (the
/// Camera Profiles page asks on opening).
pub fn ensure_loaded() -> Task<Message> {
    if Catalog::loaded().is_some() {
        return Task::none();
    }
    background(
        || {
            Catalog::global();
        },
        |_| Message::CameraProfilesScanned(false),
    )
}

pub fn handle_scanned(editor: &mut RawEditor, changed: bool) -> Task<Message> {
    if editor.camera_profiles.busy == Some(ProfileJob::Rescanning) {
        editor.camera_profiles.busy = None;
    }
    if changed {
        catalog_changed(editor)
    } else {
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One job at a time: a second click while one runs does nothing.
    #[test]
    fn one_profile_job_at_a_time() {
        let (mut e, _) = RawEditor::new();
        let _ = handle_rescan(&mut e);
        assert_eq!(e.camera_profiles.busy, Some(ProfileJob::Rescanning));
        let _ = handle_update(&mut e);
        assert_eq!(e.camera_profiles.busy, Some(ProfileJob::Rescanning));
        let _ = handle_scanned(&mut e, false);
        assert_eq!(e.camera_profiles.busy, None);
    }

    /// Choosing a profile is an edit: it lands in the params, unchanged
    /// choices do nothing.
    #[test]
    fn choosing_a_profile_is_an_edit() {
        let (mut e, _) = RawEditor::new();
        let choice = ProfileChoice::named("Nikon D3300 Camera Standard.dcp").unwrap();
        let _ = handle_selected(&mut e, choice);
        assert_eq!(e.current_edit_params.camera_profile, choice);
        let _ = handle_selected(&mut e, choice);
        assert_eq!(e.current_edit_params.camera_profile, choice);
    }

    /// A camera RawTherapee doesn't cover is remembered, so the prompt can
    /// say so instead of offering the same dead end.
    #[test]
    fn a_missing_remote_profile_is_reported() {
        let (mut e, _) = RawEditor::new();
        e.camera_profiles.busy = Some(ProfileJob::Downloading);
        // The name comes from the request, not from whichever image is open
        // when the download finishes.
        let _ = handle_downloaded(&mut e, "Nikon D3300".into(), Ok(None));
        assert_eq!(e.camera_profiles.busy, None);
        assert_eq!(e.camera_profiles.unavailable.as_deref(), Some("Nikon D3300"));
        assert!(matches!(e.camera_profiles.notice, Some(Err(_))));
    }

    /// Defaults are saved per camera; `None` clears one back to automatic.
    #[test]
    fn camera_defaults_set_and_clear() {
        let (mut e, _) = RawEditor::new();
        let _ = handle_set_default(&mut e, "NIKON D3300".into(), Some("builtin".into()));
        assert_eq!(e.camera_profile_defaults.get("NIKON D3300").map(String::as_str), Some("builtin"));
        let _ = handle_set_default(&mut e, "NIKON D3300".into(), None);
        assert!(!e.camera_profile_defaults.contains_key("NIKON D3300"));
    }
}
