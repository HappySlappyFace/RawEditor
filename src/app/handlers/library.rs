//! The catalogue, the app's side: the folder picker, the drop-import dialog,
//! the window at startup and selection anchors. The catalogue work itself is
//! the session's (`session::library`).

use iced::Task;
use std::path::{Path, PathBuf};

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::app::update::run;
use crate::raw;
use crate::session::{CacheResult, CacheStep};
use crate::ui;

pub fn handle_database_loaded(editor: &mut RawEditor, result: Result<Vec<crate::database::models::Image>, String>) -> Task<Message> {
    let Some(workers) = editor.session.catalogue_loaded(result) else {
        return Task::none();
    };
    use iced::window;
    let maximize_window = window::get_latest().and_then(|id| window::maximize(id, true));
    let scale = crate::app::handlers::window::query_scale_factor();
    // The first-run question, or the check the user agreed to.
    let updates = crate::app::handlers::updates::on_startup(editor);
    Task::batch(vec![maximize_window, scale, run(workers), updates])
}

pub fn handle_import_folder(editor: &mut RawEditor) -> Task<Message> {
    if editor.session.library.is_none() {
        return Task::none();
    }
    // AsyncFileDialog, not the blocking FileDialog: the blocking one runs the
    // native picker's event loop inside `update`, freezing the whole window
    // for as long as the dialog is open. The export path already does it this
    // way (handlers::export::handle_pick_export_base_path).
    Task::perform(
        async {
            rfd::AsyncFileDialog::new()
                .set_title("Select Folder")
                .pick_folder()
                .await
                .map(|handle| handle.path().to_path_buf())
        },
        Message::ImportFolderPicked,
    )
}

/// Landing point for the async folder picker. `None` means the user cancelled.
pub fn handle_import_folder_picked(
    editor: &mut RawEditor,
    folder: Option<PathBuf>,
) -> Task<Message> {
    let Some(folder) = folder else { return Task::none() };
    run(editor.session.import_folder(folder))
}

/// A file or folder dropped onto the window. A folder is offered for import;
/// a RAW file stands for the folder it is in. Anything else is ignored.
///
/// The stat and the folder walk happen off the UI thread. A RAW is
/// recognised by its extension alone, so the fifty events of a fifty-file
/// drop collapse into one scan of their folder here, without touching disk.
pub fn handle_file_dropped(editor: &mut RawEditor, path: PathBuf) -> Task<Message> {
    use crate::app::state::Modal;
    if editor.session.library.is_none() || !matches!(editor.active_modal, Modal::None | Modal::ImportDropped) {
        return Task::none();
    }
    let key = if raw::is_supported_raw(&path) {
        path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.clone())
    } else {
        path.clone()
    };
    let drop = &mut editor.drop_import;
    if drop.scanning.contains(&key) || drop.folders.iter().any(|(f, _)| *f == key) {
        return Task::none();
    }
    drop.scanning.insert(key.clone());
    Task::perform(
        async move {
            let scan = tokio::task::spawn_blocking({
                let key = key.clone();
                move || key.is_dir().then(|| { let files = crate::session::collect_raw_files(&key); (key, files) })
            })
            .await
            .ok()
            .flatten();
            (key, scan)
        },
        |(dropped, scan)| Message::DropScanned { dropped, scan },
    )
}

/// A dropped folder's scan is back: offer its new photos, say there are
/// none, or (when it is all catalogued already) show it.
pub fn handle_drop_scanned(
    editor: &mut RawEditor,
    dropped: PathBuf,
    scan: Option<(PathBuf, Vec<PathBuf>)>,
) -> Task<Message> {
    use crate::app::state::Modal;
    // Not ours any more: the dialog was cancelled while this scan ran.
    if !editor.drop_import.scanning.remove(&dropped) {
        return Task::none();
    }
    let Some((folder, files)) = scan else {
        return Task::none();
    };
    let catalogued: std::collections::HashSet<&str> = editor.session.images.iter().map(|img| img.path.as_str()).collect();
    let new: Vec<PathBuf> = files
        .iter()
        .filter(|f| !catalogued.contains(f.to_string_lossy().as_ref()))
        .cloned()
        .collect();
    let offering = !editor.drop_import.folders.is_empty();

    if new.is_empty() {
        if offering {
            return Task::none();
        }
        if files.is_empty() {
            editor.session.status = format!("No RAW files in {}.", folder.display());
            return Task::none();
        }
        // All catalogued: show it (the folder filter is a path prefix, so
        // this works for a folder whose photos sit in subfolders too).
        editor.session.status = format!("{} is already in the library.", folder.display());
        let key = folder.to_string_lossy().into_owned();
        if editor.active_modal == Modal::None {
            let tab = crate::app::handlers::navigation::handle_tab_changed(editor, crate::app::message::AppTab::Library);
            let filter = handle_set_library_folder_filter(editor, Some(key));
            return Task::batch([tab, filter]);
        }
        return Task::none();
    }

    let drop = &mut editor.drop_import;
    let before = drop.files.len();
    drop.files.extend(new);
    let added = drop.files.len() - before;
    if added > 0 {
        drop.folders.push((folder, added));
        // Scans land in any order; list the folders by path.
        drop.folders.sort();
    }
    if editor.active_modal == Modal::None && !editor.drop_import.folders.is_empty() {
        return crate::app::handlers::window::handle_open_modal(editor, Modal::ImportDropped);
    }
    Task::none()
}

/// Import every folder the dropped-import dialog lists, in one background job.
pub fn handle_drop_import_confirmed(editor: &mut RawEditor) -> Task<Message> {
    use crate::app::state::Modal;
    if editor.active_modal != Modal::ImportDropped {
        return Task::none();
    }
    editor.active_modal = Modal::None;
    let drop = std::mem::take(&mut editor.drop_import);
    let folders: Vec<PathBuf> = drop.folders.into_iter().map(|(f, _)| f).collect();
    run(editor.session.import_folders(folders))
}

pub fn handle_import_from_camera(editor: &mut RawEditor) -> Task<Message> {
    editor.session.status = "Camera import is not implemented yet.".to_string();
    Task::none()
}

pub fn handle_cache_processed(editor: &mut RawEditor, result: CacheResult) -> Task<Message> {
    match editor.session.cache_processed(result) {
        CacheStep::Continue(next) => run(next),
        CacheStep::Retired => Task::none(),
        CacheStep::Finished => {
            editor.session.status = format!("{} All cache tiers generated!", ui::icons::CHECK);
            Task::none()
        }
    }
}

pub fn handle_set_rating(editor: &mut RawEditor, rating: u8) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    if editor.session.rate_targets(rating) && editor.session.auto_advance {
        return Task::done(Message::SelectNextImage);
    }
    Task::none()
}

pub fn handle_set_flag(editor: &mut RawEditor, flag: i8) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    if editor.session.flag_targets(flag) && editor.session.auto_advance {
        return Task::done(Message::SelectNextImage);
    }
    Task::none()
}

pub fn handle_toggle_auto_advance(editor: &mut RawEditor) -> Task<Message> {
    editor.session.auto_advance = !editor.session.auto_advance;
    editor.save_preferences();
    Task::none()
}

pub fn handle_set_library_folder_filter(
    editor: &mut RawEditor,
    folder_filter: Option<String>,
) -> Task<Message> {
    let visible = editor.session.set_library_folder_filter(folder_filter);
    if editor.selection_anchor.is_some_and(|a| !visible.contains(&a)) {
        editor.selection_anchor = None;
    }
    Task::none()
}

/// Count for the confirmation dialog.
pub fn folder_image_count(editor: &RawEditor, folder: &str) -> usize {
    editor.session.folder_image_count(folder)
}

/// Open the "forget this folder" confirmation.
pub fn handle_remove_folder_requested(editor: &mut RawEditor, folder: String) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    if editor.session.images_in_folder(&folder).is_empty() {
        return Task::none();
    }
    editor.pending_remove_folder = Some(folder);
    editor.active_modal = crate::app::state::Modal::RemoveFolder;
    Task::none()
}

/// Forget the folder: drop its catalogue rows and edits, delete its generated
/// cache files, and leave every RAW file exactly where it is.
pub fn handle_remove_folder_confirmed(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::RemoveFolder {
        return Task::none();
    }
    let Some(folder) = editor.pending_remove_folder.take() else {
        editor.active_modal = crate::app::state::Modal::None;
        return Task::none();
    };
    editor.active_modal = crate::app::state::Modal::None;

    let gone = editor.session.remove_folder(&folder);
    if editor.selection_anchor.is_some_and(|a| gone.contains(&a)) {
        editor.selection_anchor = None;
    }
    Task::none()
}

