//! `handlers::library`: dropping folders and RAWs onto the window, rating and
//! flagging, the folder filter and forgetting a folder. Startup's success
//! path is left out: it opens the real catalogue.

use std::path::PathBuf;

use crate::app::message::{AppTab, Message};
use crate::app::state::Modal;
use crate::app::testkit::*;

fn path(s: &str) -> PathBuf {
    PathBuf::from(s)
}

/// The scan result for a dropped folder, as the handler's task returns it.
fn scanned(folder: &str, files: &[&str]) -> Message {
    Message::DropScanned {
        dropped: path(folder),
        scan: Some((path(folder), files.iter().map(|f| path(&format!("{folder}/{f}"))).collect())),
    }
}

/// A dropped RAW stands for its folder, so a fifty-file drop is one scan.
#[test]
fn dropped_raws_are_scanned_once_per_folder() {
    let mut e = editor_with_images(1);
    for f in ["a.NEF", "b.NEF", "c.NEF"] {
        send(&mut e, Message::FileDropped(path(&format!("/drops/shoot/{f}"))));
    }
    send(&mut e, Message::FileDropped(path("/drops/other")));
    let mut scanning: Vec<_> = e.drop_import.scanning.iter().cloned().collect();
    scanning.sort();
    assert_eq!(scanning, vec![path("/drops/other"), path("/drops/shoot")]);
}

/// Drops are ignored without a catalogue, and under any other dialog.
#[test]
fn drops_wait_for_the_catalogue_and_other_dialogs() {
    let mut e = editor_with_images(1);
    e.active_modal = Modal::Export;
    send(&mut e, Message::FileDropped(path("/drops/shoot")));
    assert!(e.drop_import.scanning.is_empty());

    let mut e = editor_with_images(1);
    e.session.library = None;
    send(&mut e, Message::FileDropped(path("/drops/shoot")));
    assert!(e.drop_import.scanning.is_empty());
}

/// A folder's new photos open the import dialog; a second folder joins it,
/// listed by path, and photos already offered aren't counted twice.
#[test]
fn dropped_folders_gather_in_one_dialog() {
    let mut e = editor_with_images(2);
    send(&mut e, Message::FileDropped(path("/drops/b")));
    send(&mut e, scanned("/drops/b", &["1.NEF", "2.NEF"]));
    assert_eq!(e.active_modal, Modal::ImportDropped);
    assert_eq!(e.drop_import.folders, vec![(path("/drops/b"), 2)]);

    send(&mut e, Message::FileDropped(path("/drops/a")));
    send(&mut e, Message::FileDropped(path("/drops/b")));
    assert_eq!(e.drop_import.scanning.len(), 1, "an offered folder was scanned again");
    send(&mut e, scanned("/drops/a", &["3.NEF"]));
    assert_eq!(e.drop_import.folders, vec![(path("/drops/a"), 1), (path("/drops/b"), 2)]);
    assert_eq!(e.drop_import.files.len(), 3);

    send(&mut e, Message::DropImportConfirmed);
    assert_eq!(e.active_modal, Modal::None);
    assert!(e.drop_import.folders.is_empty() && e.drop_import.files.is_empty());
}

/// Only photos not yet in the catalogue are offered.
#[test]
fn a_drop_offers_only_new_photos() {
    let mut e = editor_with_images(2);
    send(&mut e, Message::FileDropped(path("/nonexistent/testkit")));
    send(&mut e, scanned("/nonexistent/testkit", &["IMG_0001.NEF", "IMG_0002.NEF", "IMG_0003.NEF"]));
    assert_eq!(e.drop_import.folders, vec![(path("/nonexistent/testkit"), 1)]);
    assert!(e.drop_import.files.contains(&path("/nonexistent/testkit/IMG_0003.NEF")));
}

/// A folder that's all catalogued is shown in Library instead; an empty one
/// says so.
#[test]
fn a_catalogued_folder_is_shown_and_an_empty_one_reported() {
    let mut e = develop_with_images(2);
    send(&mut e, Message::FileDropped(path("/nonexistent/testkit")));
    send(&mut e, scanned("/nonexistent/testkit", &["IMG_0001.NEF", "IMG_0002.NEF"]));
    assert_eq!(e.active_modal, Modal::None);
    assert_eq!(e.session.current_tab, AppTab::Library);
    assert_eq!(e.session.library_folder_filter.as_deref(), Some("/nonexistent/testkit"));
    assert!(e.session.status.contains("already in the library"), "{}", e.session.status);

    send(&mut e, Message::FileDropped(path("/drops/empty")));
    send(&mut e, scanned("/drops/empty", &[]));
    assert_eq!(e.active_modal, Modal::None);
    assert!(e.session.status.starts_with("No RAW files"), "{}", e.session.status);
}

/// Cancelling the dialog abandons the scans still running: their results
/// must not reopen it.
#[test]
fn a_scan_landing_after_cancel_is_dropped() {
    let mut e = editor_with_images(1);
    send(&mut e, Message::FileDropped(path("/drops/a")));
    send(&mut e, Message::FileDropped(path("/drops/b")));
    send(&mut e, scanned("/drops/a", &["1.NEF"]));
    send(&mut e, Message::CloseModal);
    send(&mut e, scanned("/drops/b", &["2.NEF"]));
    assert_eq!(e.active_modal, Modal::None, "a stale scan reopened the dialog");
    assert!(e.drop_import.folders.is_empty());

    // So is a dropped path that isn't a folder (the scan comes back empty).
    send(&mut e, Message::FileDropped(path("/drops/notes.txt")));
    send(&mut e, Message::DropScanned { dropped: path("/drops/notes.txt"), scan: None });
    assert_eq!(e.active_modal, Modal::None);
    assert!(e.drop_import.scanning.is_empty());
}

/// Ratings and flags reach the catalogue and the grid, and wait while a
/// dialog has the keyboard.
#[test]
fn ratings_and_flags_are_saved_but_not_under_a_dialog() {
    let mut e = editor_with_images(3);
    send(&mut e, Message::ImageSelected(2));
    send(&mut e, Message::SetRating(4));
    send(&mut e, Message::SetFlag(-1));
    let row = |e: &crate::app::state::RawEditor| {
        let img = e.session.images.iter().find(|i| i.id == 2).unwrap();
        (img.rating, img.flag)
    };
    assert_eq!(row(&e), (4, -1));
    let saved = e.session.library.as_ref().unwrap().get_all_images().unwrap();
    let saved = saved.iter().find(|i| i.id == 2).unwrap();
    assert_eq!((saved.rating, saved.flag), (4, -1), "not saved to the catalogue");

    e.active_modal = Modal::Export;
    send(&mut e, Message::SetRating(1));
    send(&mut e, Message::SetFlag(1));
    assert_eq!(row(&e), (4, -1), "rated under a dialog");
}

#[test]
fn auto_advance_toggles() {
    let mut e = editor_with_images(1);
    let start = e.session.auto_advance;
    send(&mut e, Message::ToggleAutoAdvance);
    assert_eq!(e.session.auto_advance, !start);
}

/// A folder filter that hides the shift-click anchor drops it, so a range
/// can't reach images the user no longer sees.
#[test]
fn a_filter_hiding_the_anchor_drops_it() {
    let mut e = editor_with_images(3);
    e.session.images[2].path = "/elsewhere/IMG_0003.NEF".into();
    e.selection_anchor = Some(1);
    send(&mut e, Message::SetLibraryFolderFilter(Some("/nonexistent/testkit".into())));
    assert_eq!(e.selection_anchor, Some(1), "a visible anchor was dropped");
    e.selection_anchor = Some(3);
    send(&mut e, Message::SetLibraryFolderFilter(Some("/nonexistent/testkit".into())));
    assert_eq!(e.selection_anchor, None);
}

/// Forgetting a folder asks first, then drops its rows (and the anchor if it
/// was one of them), leaving other folders alone.
#[test]
fn forgetting_a_folder_asks_then_drops_only_its_photos() {
    let mut e = editor_with_images(3);
    e.session.images[2].path = "/elsewhere/IMG_0003.NEF".into();
    e.selection_anchor = Some(2);

    send(&mut e, Message::RemoveFolderRequested("/empty".into()));
    assert_eq!(e.active_modal, Modal::None, "asked about a folder with no photos");

    send(&mut e, Message::RemoveFolderRequested("/nonexistent/testkit".into()));
    assert_eq!(e.active_modal, Modal::RemoveFolder);
    assert_eq!(folder_count(&e), 2);
    send(&mut e, Message::ModalConfirm);
    assert_eq!(e.active_modal, Modal::None);
    let left: Vec<i64> = e.session.images.iter().map(|i| i.id).collect();
    assert_eq!(left, vec![3]);
    assert_eq!(e.selection_anchor, None);
    let saved = e.session.library.as_ref().unwrap().get_all_images().unwrap();
    assert_eq!(saved.len(), 1, "the catalogue kept the rows");

    // A stray confirm with no dialog open does nothing.
    send(&mut e, Message::RemoveFolderConfirmed);
    assert_eq!(e.session.images.len(), 1);
}

fn folder_count(e: &crate::app::state::RawEditor) -> usize {
    crate::app::handlers::library::folder_image_count(e, "/nonexistent/testkit")
}

#[test]
fn startup_reports_an_unreadable_catalogue() {
    let mut e = editor_with_images(0);
    send(&mut e, Message::DatabaseLoaded(Err("disk on fire".into())));
    assert!(e.session.status.contains("disk on fire"), "{}", e.session.status);
}

#[test]
fn a_cancelled_folder_picker_imports_nothing() {
    let mut e = editor_with_images(1);
    send(&mut e, Message::ImportFolderPicked(None));
    send(&mut e, Message::ImportFromCamera);
    assert!(e.session.status.contains("not implemented"), "{}", e.session.status);
    assert_eq!(e.session.images.len(), 1);
}
