//! Message-level test harness.
//!
//! Builds a `RawEditor` in memory — an in-memory SQLite catalogue, synthetic
//! image rows whose files do not exist, the default settings (under test
//! `AppSettings::load_or_default` never reads the real file) — and drives it
//! through `update` exactly as iced does. No window, no GPU.
//!
//! **Tasks never run.** `update` returns a `Task` that iced would execute;
//! here it is dropped. Anything async (a RAW decode, a preview read, a GPU
//! build) has to be *delivered* by the test as the message it would have
//! produced: `send(&mut e, Message::RawDataLoaded(id, Ok(raw(id))))`. Which
//! jobs are in flight is read back from the editor's own bookkeeping (the
//! load queues, `direct_raw_loads`), and the GPU resource builds — which leave
//! no trace in state — are tracked by [`World`].
//!
//! Two consequences worth knowing:
//! - `ImageResourcesReady(id, Ok(..))` needs real GPU resources, so a harness
//!   load ends in `Loading` (build in flight) or `Failed`, never `Ready`. Tests
//!   that need `Ready` (the before/after peek) set it by hand.
//! - Arrow keys answer with `Task::done(ImageSelected(next))`; [`press_arrow`]
//!   delivers that follow-up message itself, using the production
//!   `step_selection` to compute it.

use std::collections::HashSet;
use std::sync::Arc;

use crate::app::loader::{MAX_CONCURRENT_PREVIEW_LOADS, MAX_CONCURRENT_RAW_LOADS};
use crate::app::message::{AppTab, Message};
use crate::app::state::{EditorReadiness, RawEditor, HISTORY_LIMIT};
use crate::core::types::EditParams;
use crate::database::library::Library;
use crate::database::models::Image as ImageData;
use crate::raw::loader::RawDataResult;

/// An empty catalogue in memory (the real schema, nothing on disk).
pub fn library() -> Library {
    Library::open_at(std::path::Path::new(":memory:")).expect("in-memory catalogue")
}

/// An editor with `n` images in its catalogue (ids 1..=n, in display order),
/// in the Library tab, nothing selected. Each image has a (nonexistent) RAW
/// path and working-preview path, so loads and preloads are queued exactly as
/// for a real catalogue.
pub fn editor_with_images(n: usize) -> RawEditor {
    let lib = library();
    let mut images = Vec::with_capacity(n);
    for i in 1..=n {
        let filename = format!("IMG_{i:04}.NEF");
        let path = format!("/nonexistent/testkit/{filename}");
        let id = lib.import_image(&path, &filename).expect("import");
        images.push(ImageData {
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
        });
    }
    let mut e = RawEditor::new().0;
    e.library = Some(lib);
    e.images = images;
    e
}

/// [`editor_with_images`], switched to Develop with the first image selected
/// (its RAW decode requested, not delivered).
pub fn develop_with_images(n: usize) -> RawEditor {
    let mut e = editor_with_images(n);
    send(&mut e, Message::TabChanged(AppTab::Develop));
    if let Some(first) = e.images.first().map(|i| i.id) {
        send(&mut e, Message::ImageSelected(first));
    }
    e
}

/// Feed one message through `update`, as iced would. The returned `Task` is
/// dropped: deliver async results yourself (see the module docs).
pub fn send(e: &mut RawEditor, message: Message) {
    let _ = crate::app::update::update(e, message);
}

/// An arrow key: `SelectNextImage`/`SelectPreviousImage`, then the
/// `ImageSelected` its `Task::done` would deliver.
pub fn press_arrow(e: &mut RawEditor, forward: bool) {
    // Computed before the message, from the same state the handler reads.
    let target = (e.active_modal == crate::app::state::Modal::None)
        .then(|| crate::app::handlers::navigation::step_selection(e, forward))
        .flatten();
    send(e, if forward { Message::SelectNextImage } else { Message::SelectPreviousImage });
    if let Some(id) = target {
        send(e, Message::ImageSelected(id));
    }
}

/// A tiny but valid decoded RAW: 8×8 RGGB, a flat grey. `tag` lands in
/// `model`, so tests can tell which delivery the editor is holding.
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

/// The RAW preloads the loader subscription is running: the queue's head.
pub fn running_raw_preloads(e: &RawEditor) -> Vec<i64> {
    e.queued_raw_loads.iter().take(MAX_CONCURRENT_RAW_LOADS).map(|(id, _)| *id).collect()
}

/// The preview reads the loader subscription is running.
pub fn running_preview_loads(e: &RawEditor) -> Vec<i64> {
    e.queued_loads.iter().take(MAX_CONCURRENT_PREVIEW_LOADS).map(|(id, _)| *id).collect()
}

/// The image's saved edits, read back from the catalogue.
pub fn saved_edits(e: &RawEditor, id: i64) -> EditParams {
    e.library.as_ref().expect("library").load_edit_params(id).unwrap_or_default()
}

/// `(stack length, index)` of an image's undo history, if it has one.
pub fn history(e: &RawEditor, id: i64) -> Option<(usize, usize)> {
    e.history_map.get(&id).map(|(stack, index)| (stack.len(), *index))
}

/// The resources build `prepare_image_resources_from_raw` starts leaves one
/// mark in state: `current_metadata` for that image while it is `Loading`.
fn build_mark(e: &RawEditor) -> Option<(i64, String)> {
    match e.editor_readiness {
        EditorReadiness::Loading(id) if e.current_metadata_id == Some(id) => {
            e.current_metadata.as_ref().map(|m| (id, m.model.clone()))
        }
        _ => None,
    }
}

/// Structural invariants that must hold after every message. Returns the
/// first violation.
///
/// "In flight" for a `Loading` image is judged from state alone here: a
/// direct decode, a running preload, or a resources build (its mark, see
/// `build_mark`). [`World::check`] is stricter about builds.
pub fn check_invariants(e: &RawEditor) -> Result<(), String> {
    check_with(e, |id| build_mark(e).is_some_and(|(b, _)| b == id))
}

/// [`check_invariants`], panicking with the violation and a state dump.
pub fn assert_invariants(e: &RawEditor) {
    if let Err(why) = check_invariants(e) {
        panic!("invariant violated: {why}\n{}", dump(e));
    }
}

fn check_with(e: &RawEditor, build_in_flight: impl Fn(i64) -> bool) -> Result<(), String> {
    // Every load queue is mirrored exactly by its pending set, no duplicates.
    for (name, queue, pending) in [
        ("raw", &e.queued_raw_loads, &e.pending_raw_loads),
        ("preview", &e.queued_loads, &e.pending_loads),
        ("proxy", &e.queued_proxy_loads, &e.pending_proxy_loads),
    ] {
        let ids: Vec<i64> = queue.iter().map(|(id, _)| *id).collect();
        let set: HashSet<i64> = ids.iter().copied().collect();
        if set.len() != ids.len() {
            return Err(format!("{name} queue holds duplicates: {ids:?}"));
        }
        if &set != pending {
            return Err(format!("{name} queue {ids:?} disagrees with its pending set {pending:?}"));
        }
    }

    // `Loading(id)` is a promise that something will finish loading it.
    if let EditorReadiness::Loading(id) = e.editor_readiness {
        let in_flight = e.direct_raw_loads.contains(&id)
            || running_raw_preloads(e).contains(&id)
            || build_in_flight(id);
        if !in_flight {
            return Err(format!(
                "Loading({id}) with nothing in flight for it (not a direct decode, not a \
                 running preload, no resources build): Develop would wait forever"
            ));
        }
    }

    // In Develop, what the editor is busy with is the selected image.
    if e.current_tab == AppTab::Develop {
        if let Some(sel) = e.selected_image_id {
            let ok = match &e.editor_readiness {
                EditorReadiness::Loading(id) | EditorReadiness::Ready(id) => *id == sel,
                EditorReadiness::Failed(id, _) => *id == sel,
                EditorReadiness::NoSelection => false,
            };
            if !ok {
                return Err(format!("Develop shows {sel} but the editor is {:?}", e.editor_readiness));
            }
        }
    }

    // Undo history stays in range and within its cap.
    for (id, (stack, index)) in &e.history_map {
        if stack.is_empty() || *index >= stack.len() || stack.len() > HISTORY_LIMIT {
            return Err(format!("image {id}: history of {} with index {index}", stack.len()));
        }
    }
    Ok(())
}

/// The loading state worth printing when an invariant breaks.
pub fn dump(e: &RawEditor) -> String {
    let ids = |q: &[(i64, String)]| q.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let mut direct: Vec<i64> = e.direct_raw_loads.iter().copied().collect();
    direct.sort_unstable();
    let mut cached: Vec<i64> = e.raw_cache.iter().map(|(id, _)| *id).collect();
    cached.sort_unstable();
    format!(
        "  tab {:?}, selected {:?}, readiness {:?}\n  raw queue {:?} (running {:?})\n  \
         direct decodes {:?}\n  raw cache {:?}\n  metadata for {:?}",
        e.current_tab,
        e.selected_image_id,
        e.editor_readiness,
        ids(&e.queued_raw_loads),
        running_raw_preloads(e),
        direct,
        cached,
        e.current_metadata_id,
    )
}

/// A small deterministic RNG (xorshift64*), so fuzz runs need no dependency
/// and a failing seed replays exactly.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Never zero (xorshift's fixed point); spread nearby seeds apart.
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    /// True with probability `percent`/100.
    pub fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent as usize
    }
    pub fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        (!items.is_empty()).then(|| items[self.below(items.len())])
    }
}

/// The async side of the app: the jobs `update` has started, and delivering
/// their results.
///
/// RAW and preview loads are read from the editor's queues. GPU resource
/// builds leave no state behind, so they are recorded here as they start
/// (noticed through `build_mark` changing across a message) and finished with
/// `ImageResourcesReady(id, Err(..))` — the harness has no GPU to make an Ok.
#[derive(Debug, Default)]
pub struct World {
    /// Resource builds started and not yet delivered, oldest first.
    pub builds: Vec<i64>,
    /// Serial for synthetic RAWs, so each delivery is distinguishable.
    pub serial: u64,
    /// The last messages sent, for failure reports.
    pub log: Vec<String>,
    /// Let RAW *preloads* fail. Off by default: a failed preload of the image
    /// Develop is waiting on strands it today (see
    /// `tests::loading::a_failed_preload_of_the_selection_strands_it`).
    pub preload_failures: bool,
}

/// One async result [`World`] could deliver next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    RawPreload(i64),
    DirectDecode(i64),
    Build(i64),
    Preview(i64),
}

impl World {
    /// `send`, also recording any resources build the message started.
    pub fn send(&mut self, e: &mut RawEditor, message: Message) {
        let label = short(&message);
        let before = build_mark(e);
        send(e, message);
        let after = build_mark(e);
        if let Some((id, _)) = after.clone().filter(|_| after != before) {
            self.builds.push(id);
        }
        self.log.push(label);
        if self.log.len() > 40 {
            self.log.remove(0);
        }
    }

    /// Every result that could arrive now.
    pub fn jobs(&self, e: &RawEditor) -> Vec<Job> {
        let mut direct: Vec<i64> = e.direct_raw_loads.iter().copied().collect();
        direct.sort_unstable(); // HashSet order is not deterministic
        running_raw_preloads(e)
            .into_iter()
            .map(Job::RawPreload)
            .chain(direct.into_iter().map(Job::DirectDecode))
            .chain(self.builds.iter().copied().map(Job::Build))
            .chain(running_preview_loads(e).into_iter().map(Job::Preview))
            .collect()
    }

    /// Deliver `job`'s result; `ok` picks success or failure (a build always
    /// fails: there is no GPU).
    pub fn deliver(&mut self, e: &mut RawEditor, job: Job, ok: bool) {
        self.serial += 1;
        let tag = self.serial;
        let message = match job {
            Job::RawPreload(id) => Message::RawPreloaded(
                id,
                if ok || !self.preload_failures { Ok(Arc::new(raw(tag))) } else { Err("testkit: unreadable".into()) },
            ),
            Job::DirectDecode(id) => Message::RawDataLoaded(
                id,
                if ok { Ok(raw(tag)) } else { Err("testkit: unreadable".into()) },
            ),
            Job::Build(id) => {
                if let Some(i) = self.builds.iter().position(|b| *b == id) {
                    self.builds.remove(i);
                }
                Message::ImageResourcesReady(id, Err("testkit: no GPU".into()))
            }
            Job::Preview(id) => Message::PreviewCached(
                id,
                if ok { Ok((2, 2, Arc::from(vec![0u8; 16]))) } else { Err("testkit: no file".into()) },
            ),
        };
        self.send(e, message);
    }

    /// Deliver everything in flight, in `rng`'s order, until nothing is left.
    pub fn drain(&mut self, e: &mut RawEditor, rng: &mut Rng) -> Result<(), String> {
        for _ in 0..10_000 {
            let jobs = self.jobs(e);
            let Some(job) = rng.pick(&jobs) else { return Ok(()) };
            let ok = rng.chance(85);
            self.deliver(e, job, ok);
            self.check(e)?;
        }
        Err("the async work never drained".into())
    }

    /// [`check_invariants`], with builds judged by what was actually started
    /// and not yet delivered rather than by their mark.
    pub fn check(&self, e: &RawEditor) -> Result<(), String> {
        check_with(e, |id| self.builds.contains(&id))
    }

    /// The recent messages, one per line.
    pub fn history(&self) -> String {
        self.log.join("\n    ")
    }
}

/// A one-line description of a message, without its payload's bulk.
fn short(m: &Message) -> String {
    match m {
        Message::RawDataLoaded(id, r) => format!("RawDataLoaded({id}, {})", if r.is_ok() { "Ok" } else { "Err" }),
        Message::RawPreloaded(id, r) => format!("RawPreloaded({id}, {})", if r.is_ok() { "Ok" } else { "Err" }),
        Message::ImageResourcesReady(id, _) => format!("ImageResourcesReady({id}, Err)"),
        Message::PreviewCached(id, _) => format!("PreviewCached({id})"),
        Message::WorkingPreviewReady(id, ..) => format!("WorkingPreviewReady({id})"),
        other => {
            let s = format!("{other:?}");
            s.chars().take(60).collect()
        }
    }
}
