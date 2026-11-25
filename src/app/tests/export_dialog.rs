//! `handlers::export`: the dialog's fields, where it points by default, and
//! when Export closes it. The batch itself is `session::export`'s.

use std::path::PathBuf;

use crate::app::message::Message;
use crate::app::state::Modal;
use crate::app::testkit::*;
use crate::core::output_space::OutputSpace;
use crate::session::{ExportFormat, ExportJobState};

#[test]
fn the_fields_set_the_export_settings() {
    let mut e = develop_with_images(1);
    send(&mut e, Message::SetExportFormat(ExportFormat::Tiff));
    send(&mut e, Message::SetExportColorSpace(OutputSpace::DisplayP3));
    send(&mut e, Message::SetExportQuality(73));
    send(&mut e, Message::SetExportSubfolder("Edits".into()));
    send(&mut e, Message::SetExportBasePath(PathBuf::from("/exports")));
    let s = &e.session.export_settings;
    assert_eq!(s.format, ExportFormat::Tiff);
    assert_eq!(s.color_space, OutputSpace::DisplayP3);
    assert_eq!(s.quality, 73);
    assert_eq!(s.subfolder, "Edits");
    assert_eq!(s.base_path, PathBuf::from("/exports"));

    // A picker that hands back nothing leaves the folder alone.
    send(&mut e, Message::SetExportBasePath(PathBuf::new()));
    assert_eq!(e.session.export_settings.base_path, PathBuf::from("/exports"));
}

/// Until the user picks a folder, the dialog suggests the photo's own.
#[test]
fn the_dialog_suggests_the_photos_folder_until_one_is_picked() {
    let mut e = develop_with_images(1);
    send(&mut e, Message::OpenExportModal);
    assert_eq!(e.active_modal, Modal::Export);
    assert_eq!(e.session.export_settings.base_path, PathBuf::from("/nonexistent/testkit"));

    send(&mut e, Message::CloseModal);
    send(&mut e, Message::SetExportBasePath(PathBuf::from("/exports")));
    send(&mut e, Message::OpenExportModal);
    assert_eq!(e.session.export_settings.base_path, PathBuf::from("/exports"), "the pick was overridden");
}

/// Export closes the dialog once the batch starts, and keeps it open (with
/// the reason) when it can't.
#[test]
fn export_closes_the_dialog_only_when_it_starts() {
    let mut e = develop_with_images(2);
    send(&mut e, Message::OpenExportModal);
    send(&mut e, Message::ToggleExportResize(true));
    send(&mut e, Message::SetExportWidth("wide".into()));
    send(&mut e, Message::ExportConfirmed);
    assert_eq!(e.active_modal, Modal::Export, "closed on an invalid size");
    assert!(!e.session.status.is_empty(), "no reason given");

    send(&mut e, Message::SetExportWidth("2048".into()));
    send(&mut e, Message::ExportConfirmed);
    assert_eq!(e.active_modal, Modal::None);
    assert!(e.session.is_exporting);
}

/// Cancel lets the image in flight finish and drops the rest.
#[test]
fn cancel_drops_the_images_not_started() {
    let mut e = editor_with_images(4);
    send(&mut e, Message::ImageSelected(1));
    send(&mut e, Message::SelectAll);
    send(&mut e, Message::OpenExportModal);
    send(&mut e, Message::ExportConfirmed);
    assert_eq!(e.session.export_jobs.len(), 4, "{}", e.session.status);
    send(&mut e, Message::CancelExport);
    assert!(e.session.export_queue.is_empty());
    let cancelled = e.session.export_jobs.iter().filter(|j| j.state == ExportJobState::Failed("cancelled".into())).count();
    assert_eq!(cancelled, 4, "nothing had started, so all four go");
    assert!(e.session.status.contains("cancelled"), "{}", e.session.status);
}
