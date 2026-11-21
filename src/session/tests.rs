//! The session driven on its own, as a front end other than the desktop app
//! would drive it (`session::testkit`).

use super::testkit::*;
use super::{EditorReadiness, Handled, SessionEvent};

/// Opening an image in Develop decodes its RAW; the decode builds its GPU
/// resources; a failed build fails the image — the whole load, headless.
#[test]
fn an_image_loads_through_its_effects() {
    let mut s = session_with_images(3);
    let opened = open_in_develop(&mut s, 2);
    assert!(labels(&opened).contains(&"raw decode"), "asked for {:?}", labels(&opened));
    assert!(matches!(s.editor_readiness, EditorReadiness::Loading(2)));
    assert!(s.direct_raw_loads.contains(&2));

    let decoded = deliver(&mut s, SessionEvent::RawDataLoaded(2, Ok(raw("two"))));
    assert_eq!(labels(&decoded), ["develop resources"]);
    assert!(s.direct_raw_loads.is_empty());

    let built = deliver(&mut s, SessionEvent::ImageResourcesReady(2, Err("no GPU".into())));
    assert!(built.is_empty());
    assert!(matches!(s.editor_readiness, EditorReadiness::Failed(2, _)));
}

/// A result for an image the user has left is dropped, and asks for nothing.
#[test]
fn a_decode_for_an_image_left_behind_builds_nothing() {
    let mut s = session_with_images(3);
    let _ = open_in_develop(&mut s, 1);
    let _ = s.select_image(3, super::SelectionChange::Only);
    let late = deliver(&mut s, SessionEvent::RawDataLoaded(1, Ok(raw("one"))));
    assert!(!labels(&late).contains(&"develop resources"), "built a left image: {:?}", labels(&late));
    assert!(matches!(s.editor_readiness, EditorReadiness::Loading(3)));
}

/// Without GPU resources an edit renders nothing, but it is the user's edit:
/// it is saved at the commit point and becomes an undo step.
#[test]
fn an_edit_is_saved_and_undoable_without_a_gpu() {
    let mut s = session_with_images(1);
    let _ = open_in_develop(&mut s, 1);
    let ticked = s.edit(|p| p.exposure = 1.25);
    assert!(!labels(&ticked).contains(&"render"));
    let _ = s.commit_edit();
    let saved = s.library.as_ref().unwrap().load_edit_params(1).unwrap_or_default();
    assert_eq!(saved.exposure, 1.25);
    let _ = s.undo();
    assert_eq!(s.current_edit_params.exposure, 0.0);
}

/// The few events a front end has a part in come back to it, untouched.
#[test]
fn front_end_events_are_handed_back() {
    let mut s = session_with_images(1);
    let event = SessionEvent::ModelDownloaded(crate::ml::models::ModelId::Segment, Err("offline".into()));
    match s.handle(event) {
        Handled::FrontEnd(e) => assert!(matches!(*e, SessionEvent::ModelDownloaded(..))),
        Handled::Done(_) => panic!("the session kept an event the front end has a part in"),
    }
}

/// A new lens database drops every decode (they carry the old lookup) and
/// decodes the open image again; a failed download changes nothing.
#[test]
fn a_new_lens_database_redecodes_the_open_image() {
    let mut s = session_with_images(2);
    let _ = open_in_develop(&mut s, 1);
    let _ = deliver(&mut s, SessionEvent::RawDataLoaded(1, Ok(raw("one"))));
    assert!(s.raw_for(1).is_some());

    assert!(deliver(&mut s, SessionEvent::LensfunInstalled(Err("offline".into()))).is_empty());
    assert!(s.raw_for(1).is_some(), "a failed download dropped the decode");
    assert!(deliver(&mut s, SessionEvent::LensDbLoaded(false)).is_empty(), "a first read changes nothing");

    let reloaded = deliver(&mut s, SessionEvent::LensDbLoaded(true));
    assert!(s.raw_cache.is_empty());
    assert!(labels(&reloaded).contains(&"raw decode"), "asked for {:?}", labels(&reloaded));
}
