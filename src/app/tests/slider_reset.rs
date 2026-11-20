//! A double-click reset on a slider track: the first press already jumped the
//! slider and committed; the reset must fold into that step.

use crate::app::message::Message;
use crate::app::testkit::*;

fn exposure(e: &crate::app::state::RawEditor) -> f32 {
    e.session.current_edit_params.exposure
}

#[test]
fn a_double_click_reset_is_one_undo_step_back_to_the_old_value() {
    let mut e = develop_with_images(2);
    send(&mut e, Message::ExposureChanged(1.0));
    send(&mut e, Message::CommitEdit);
    // First press of the double click: jumps to the click, release commits.
    send(&mut e, Message::ExposureChanged(2.5));
    send(&mut e, Message::CommitEdit);
    // Second press: the reset.
    send(&mut e, Message::SliderReset { change: Box::new(Message::ExposureChanged(0.0)) });
    assert_eq!(exposure(&e), 0.0);
    assert_eq!(history(&e, 1), Some((3, 2)), "reset must not add a second step");
    send(&mut e, Message::Undo);
    assert_eq!(exposure(&e), 1.0, "undo returns to the value before the double click");
    assert_eq!(saved_edits(&e, 1).exposure, 1.0);
}

#[test]
fn a_reset_back_to_the_start_leaves_no_step() {
    let mut e = develop_with_images(2);
    send(&mut e, Message::ExposureChanged(2.5));
    send(&mut e, Message::CommitEdit);
    send(&mut e, Message::SliderReset { change: Box::new(Message::ExposureChanged(0.0)) });
    assert_eq!(history(&e, 1), Some((1, 0)));
    assert_eq!(exposure(&e), 0.0);
}

#[test]
fn a_lone_reset_is_an_ordinary_undo_step() {
    let mut e = develop_with_images(2);
    send(&mut e, Message::ExposureChanged(1.0));
    send(&mut e, Message::CommitEdit);
    e.session.last_commit = None; // the earlier commit was long ago
    send(&mut e, Message::SliderReset { change: Box::new(Message::ExposureChanged(0.0)) });
    assert_eq!(history(&e, 1), Some((3, 2)));
    send(&mut e, Message::Undo);
    assert_eq!(exposure(&e), 1.0);
}
