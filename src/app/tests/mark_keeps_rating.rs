//! Marking images for removal and un-marking them must not cost the star
//! ratings; every reader of the mark goes through `models::is_marked`.

use crate::app::message::{AppTab, Message};
use crate::app::testkit::*;
use crate::database::models::is_marked;

fn rating(e: &crate::app::state::RawEditor, id: i64) -> u8 {
    e.session.images.iter().find(|i| i.id == id).unwrap().rating
}

fn saved_rating(e: &crate::app::state::RawEditor, id: i64) -> u8 {
    let lib = e.session.library.as_ref().unwrap();
    lib.get_all_images().unwrap().iter().find(|i| i.id == id).unwrap().rating
}

#[test]
fn delete_then_delete_again_restores_each_images_stars() {
    let mut e = editor_with_images(3);
    send(&mut e, Message::TabChanged(AppTab::Library));
    for (id, stars) in [(1, 5u8), (2, 0), (3, 2)] {
        e.session.library.as_ref().unwrap().set_ratings(&[id], stars).unwrap();
        e.session.images.iter_mut().find(|i| i.id == id).unwrap().rating = stars;
    }
    e.session.multi_selection = [1, 2, 3].into_iter().collect();

    send(&mut e, Message::DeleteImage);
    send(&mut e, Message::MarkForRemovalConfirmed);
    for id in [1, 2, 3] {
        assert!(is_marked(rating(&e, id)), "image {id} not marked in memory");
        assert!(is_marked(saved_rating(&e, id)), "image {id} not marked on disk");
    }

    // Develop and Export hide marked images; Library still shows them.
    assert_eq!(e.session.displayed_images().len(), 3);

    send(&mut e, Message::DeleteImage); // all marked: toggles off
    for (id, stars) in [(1, 5u8), (2, 0), (3, 2)] {
        assert_eq!(rating(&e, id), stars, "image {id} lost its stars in memory");
        assert_eq!(saved_rating(&e, id), stars, "image {id} lost its stars on disk");
    }
}
