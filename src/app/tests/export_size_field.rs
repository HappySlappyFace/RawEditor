//! Export's "Max long edge" field: a typo must not export at full size.

use crate::app::message::Message;
use crate::app::testkit::*;

#[test]
fn a_typo_in_the_long_edge_blocks_the_export_and_keeps_the_text() {
    let mut e = develop_with_images(2);
    send(&mut e, Message::ToggleExportResize(true));
    for typo in ["2048x", "", "-5", "0"] {
        send(&mut e, Message::SetExportWidth(typo.to_string()));
        assert_eq!(e.session.export_settings.max_width_text, typo, "the field keeps what was typed");
        send(&mut e, Message::ExportConfirmed);
        assert!(!e.session.is_exporting, "{typo:?} must not start an export");
        assert!(e.session.export_jobs.is_empty());
    }
}

#[test]
fn a_good_long_edge_exports_and_resize_off_ignores_the_field() {
    let mut e = develop_with_images(2);
    send(&mut e, Message::ToggleExportResize(true));
    send(&mut e, Message::SetExportWidth("1600".to_string()));
    assert_eq!(e.session.export_settings.max_width, 1600);
    assert!(!e.session.export_settings.size_invalid());

    send(&mut e, Message::SetExportWidth("".to_string()));
    send(&mut e, Message::ToggleExportResize(false));
    assert!(!e.session.export_settings.size_invalid(), "an unticked resize needs no size");
}

#[test]
fn a_sensor_correction_checkbox_is_one_undo_step() {
    let mut e = develop_with_images(2);
    send(&mut e, Message::BlackPhaseChanged(false, 1));
    assert_eq!(history(&e, 1), Some((2, 1)));
    send(&mut e, Message::Undo);
    assert_eq!(e.session.current_edit_params.black_phase_x, 0);
}
