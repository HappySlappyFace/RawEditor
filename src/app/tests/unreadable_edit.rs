//! A saved edit that no longer parses must not be lost when the image is
//! opened and the next flush writes defaults over it.

use crate::app::message::Message;
use crate::app::testkit::{develop_with_images, send};

#[test]
fn selecting_an_image_with_an_unreadable_edit_keeps_the_old_json_after_the_flush() {
    let mut e = develop_with_images(2);
    let damaged = r#"{"exposure": "oops"}"#;
    e.library.as_ref().unwrap().set_raw_edit_json(2, damaged).unwrap();

    send(&mut e, Message::ImageSelected(2));
    assert_eq!(e.current_edit_params, Default::default(), "opens unedited");
    assert!(e.status.contains("could not be read"), "the user is told: {}", e.status);

    // The flush that follows any selection change or commit.
    e.mark_edits_dirty();
    e.flush_edits();
    send(&mut e, Message::ImageSelected(1));

    let lib = e.library.as_ref().unwrap();
    assert!(lib.load_saved_edit(2).unwrap().is_some(), "defaults were saved");
    assert_eq!(lib.edit_backup(2).as_deref(), Some(damaged), "the damaged edit is still recoverable");
}
