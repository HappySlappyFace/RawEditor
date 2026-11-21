//! Session-level test harness: a [`Session`] over an in-memory catalogue,
//! driven directly — no front end, no iced, no GPU.
//!
//! **Effects never run.** A session call returns [`Effects`]; here they are
//! read, not executed: [`labels`] says what was asked for ("raw decode",
//! "develop resources", "render"…), and the test delivers the result itself
//! through [`deliver`], exactly as a front end hands it back with
//! `Session::handle`.
//!
//! The app's message-level harness (`app::testkit`) shares these fixtures.

use crate::app::message::AppTab;
use crate::database::library::Library;
use crate::database::models::Image as ImageData;
use crate::raw::loader::RawDataResult;

use super::{Effects, Handled, SelectionChange, Session, SessionEvent, TabLoad};

/// An empty catalogue in memory (the real schema, nothing on disk).
pub fn library() -> Library {
    Library::open_at(std::path::Path::new(":memory:")).expect("in-memory catalogue")
}

/// `n` images imported into `lib` (ids 1..=n, in display order). Each has a
/// (nonexistent) RAW path and working-preview path, so loads and preloads are
/// queued exactly as for a real catalogue.
pub fn image_rows(lib: &Library, n: usize) -> Vec<ImageData> {
    (1..=n)
        .map(|i| {
            let filename = format!("IMG_{i:04}.NEF");
            let path = format!("/nonexistent/testkit/{filename}");
            let id = lib.import_image(&path, &filename).expect("import");
            ImageData {
                id,
                filename,
                path,
                cache_path_thumb: None,
                cache_path_instant: None,
                cache_path_working: Some(format!("/nonexistent/testkit/working/{id}.jpg")),
                cache_path_edited: None,
                file_status: "exists".into(),
                rating: 0,
                flag: 0,
            }
        })
        .collect()
}

/// A session with `n` images in its catalogue, in the Library tab, nothing
/// selected.
pub fn session_with_images(n: usize) -> Session {
    let lib = library();
    let mut s = Session::default();
    s.images = image_rows(&lib, n);
    s.library = Some(lib);
    s
}

/// What a front end does when the user opens `image_id` in Develop: switch
/// tab, then select it. Returns everything that asked for.
pub fn open_in_develop(s: &mut Session, image_id: i64) -> Effects {
    let mut all = vec![s.set_tab(AppTab::Develop)];
    match s.load_for_tab(AppTab::Develop) {
        TabLoad::Load(e) => all.push(e),
        TabLoad::Nothing | TabLoad::Moved(_) => {}
    }
    all.push(s.select_image(image_id, SelectionChange::Only));
    Effects::batch(all)
}

/// Hand a result back to the session, as a front end does. Panics on an
/// event the front end has a part in: call its method directly instead.
pub fn deliver(s: &mut Session, event: SessionEvent) -> Effects {
    match s.handle(event) {
        Handled::Done(effects) => effects,
        Handled::FrontEnd(event) => panic!("{event:?} needs the front end; call its session method"),
    }
}

/// What `effects` asks for, in order.
pub fn labels(effects: &Effects) -> Vec<&'static str> {
    effects.labels().collect()
}

/// A tiny but valid decoded RAW: 8×8 RGGB, a flat grey. `tag` lands in
/// `model`, so tests can tell which delivery the session is holding.
pub fn raw(tag: impl std::fmt::Display) -> RawDataResult {
    RawDataResult {
        data: vec![1000; 64],
        width: 8,
        height: 8,
        wb_multipliers: [2.0, 1.0, 1.5, 1.0],
        color_matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        cfa_pattern: 0,
        black_levels: [0; 4],
        white_level: 4095,
        color_matrix_is_d65: true,
        crops: [0; 4],
        cfa_name: "RGGB".into(),
        measured_black_levels: [0.0; 4],
        make: "Testkit".into(),
        model: format!("synthetic {tag}"),
        iso: String::new(),
        shutter_speed: String::new(),
        aperture: String::new(),
        lens: String::new(),
        dcp_profile: None,
        lens_context: Default::default(),
        orientation: 1,
    }
}
