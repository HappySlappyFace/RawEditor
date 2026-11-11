//! The selection / loading state machine, driven by messages.
//!
//! The promise under test: in Develop the selected image is always loaded,
//! being loaded by something actually in flight, or failed — never left on
//! "Loading" with nothing that will finish it. Two bugs of that shape passed
//! green helper tests because the code *after* the helper (the `requeue` that
//! follows every selection) was never run; these tests run all of it.
//!
//! With 10 images and the default windows (RAW preload 1 behind, 4 ahead),
//! selecting image 1 queues RAW preloads [2, 10, 3, 4, 5], of which the first
//! two (`MAX_CONCURRENT_RAW_LOADS`) are running.

use std::sync::Arc;

use crate::app::message::{AppTab, Message};
use crate::app::state::EditorReadiness;
use crate::app::testkit::*;

fn queue(e: &crate::app::state::RawEditor) -> Vec<i64> {
    e.queued_raw_loads.iter().map(|(id, _)| *id).collect()
}

#[test]
fn selecting_in_develop_decodes_it_and_preloads_its_neighbours() {
    let e = develop_with_images(10);
    assert_eq!(e.selected_image_id, Some(1));
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(1));
    assert!(e.direct_raw_loads.contains(&1), "no decode started for the selection");
    assert_eq!(queue(&e), vec![2, 10, 3, 4, 5], "nearest first, in display order");
    assert_invariants(&e);
}

/// The 0202 bug: the selected image was moved within the queue's tail
/// instead of being claimed, and the `requeue` that runs right after every
/// selection dropped it — Develop sat on "Loading" forever.
#[test]
fn a_selection_waiting_in_the_preload_tail_is_decoded_directly() {
    let mut e = develop_with_images(10);
    assert_eq!(queue(&e)[2..], [3, 4, 5]); // 4 is waiting, not running
    send(&mut e, Message::ImageSelected(4));
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(4));
    assert!(e.direct_raw_loads.contains(&4), "the waiting preload was not taken over");
    assert!(!queue(&e).contains(&4) && !e.pending_raw_loads.contains(&4));
    // The running head survives the requeue; the tail is 4's neighbours.
    assert_eq!(queue(&e), vec![2, 10, 5, 3, 6, 7, 8]);
    assert_invariants(&e);
}

#[test]
fn a_selection_already_preloading_is_left_to_its_preload() {
    let mut e = develop_with_images(10);
    send(&mut e, Message::ImageSelected(2)); // running at the head
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(2));
    assert!(!e.direct_raw_loads.contains(&2), "decoded twice");
    assert!(running_raw_preloads(&e).contains(&2));
    assert_invariants(&e);

    // Its preload landing completes the load: the resources build starts.
    send(&mut e, Message::RawPreloaded(2, Ok(Arc::new(raw("preload")))));
    assert_eq!(e.current_metadata_id, Some(2), "the preload did not start the build");
    assert!(!queue(&e).contains(&2));
    assert_invariants(&e);
}

#[test]
fn a_decode_landing_after_the_user_moved_on_is_cached_not_shown() {
    let mut e = develop_with_images(10);
    send(&mut e, Message::ImageSelected(3));
    send(&mut e, Message::RawDataLoaded(1, Ok(raw("late"))));
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(3), "a stale RAW took over");
    assert_ne!(e.current_metadata_id, Some(1));
    assert!(!e.direct_raw_loads.contains(&1));
    assert!(e.raw_cache.contains(&1), "the stale decode was thrown away");
    assert_invariants(&e);

    // Going back is instant: the cached RAW goes straight to the build.
    send(&mut e, Message::ImageSelected(1));
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(1));
    assert_eq!(e.current_metadata_id, Some(1));
    assert!(!e.direct_raw_loads.contains(&1), "re-decoded a cached RAW");
    assert_invariants(&e);
}

#[test]
fn a_failed_decode_fails_only_the_selected_image_and_can_be_retried() {
    let mut e = develop_with_images(10);
    send(&mut e, Message::ImageSelected(3));
    send(&mut e, Message::RawDataLoaded(1, Err("bad file".into())));
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(3), "another image's failure failed this one");
    send(&mut e, Message::RawDataLoaded(3, Err("bad file".into())));
    assert!(matches!(e.editor_readiness, EditorReadiness::Failed(3, _)));
    assert_invariants(&e);

    send(&mut e, Message::ImageSelected(4));
    send(&mut e, Message::ImageSelected(3));
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(3));
    assert!(e.direct_raw_loads.contains(&3), "a failed image is never retried");
    assert_invariants(&e);
}

#[test]
fn resources_built_for_an_image_left_behind_are_ignored() {
    let mut e = develop_with_images(10);
    send(&mut e, Message::RawDataLoaded(1, Ok(raw("1"))));
    assert_eq!(e.current_metadata_id, Some(1)); // building 1
    send(&mut e, Message::ImageSelected(3));
    send(&mut e, Message::ImageResourcesReady(1, Err("late".into())));
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(3));
    assert_invariants(&e);
}

#[test]
fn holding_an_arrow_key_keeps_the_selection_in_flight_at_every_step() {
    let mut e = develop_with_images(12);
    for step in 0..30 {
        press_arrow(&mut e, step % 7 != 6); // mostly forward, the odd step back
        if let Err(why) = check_invariants(&e) {
            panic!("after step {step}: {why}\n{}", dump(&e));
        }
    }
    let sel = e.selected_image_id.expect("selection");
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(sel));
    // Stepping back onto an image whose decode is still running reuses it.
    let before = e.direct_raw_loads.len();
    press_arrow(&mut e, false);
    press_arrow(&mut e, true);
    assert_eq!(e.direct_raw_loads.len(), before, "a second decode of the same image");
}

/// Random interleavings of selections, arrow-key holds and async results
/// landing in any order, in Develop. After every message the invariants hold;
/// after the async work drains, the selection is not stuck on "Loading".
fn fuzz(seed: u64, steps: usize, tabs: bool, preload_failures: bool) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let n = 3 + rng.below(10);
    let mut e = editor_with_images(n);
    let mut w = World { preload_failures, ..World::default() };
    let ids: Vec<i64> = e.images.iter().map(|i| i.id).collect();
    w.send(&mut e, Message::TabChanged(AppTab::Develop));
    w.send(&mut e, Message::ImageSelected(ids[0]));

    let fail = |w: &World, e: &crate::app::state::RawEditor, step: usize, why: String| {
        format!(
            "seed {seed} ({n} images), step {step}: {why}\n{}\n  last messages:\n    {}",
            dump(e),
            w.history()
        )
    };

    for step in 0..steps {
        let roll = rng.below(100);
        match roll {
            0..=19 => {
                let id = rng.pick(&ids).unwrap_or(1);
                w.send(&mut e, Message::ImageSelected(id));
            }
            20..=34 => {
                // A tap, or a held key: a burst of repeats in one direction.
                let forward = rng.chance(60);
                let repeats = if rng.chance(30) { 2 + rng.below(12) } else { 1 };
                for _ in 0..repeats {
                    let target = crate::app::handlers::navigation::step_selection(&e, forward);
                    w.send(&mut e, if forward { Message::SelectNextImage } else { Message::SelectPreviousImage });
                    if let Some(id) = target {
                        w.send(&mut e, Message::ImageSelected(id));
                    }
                    w.check(&e).map_err(|why| fail(&w, &e, step, why))?;
                }
            }
            35..=84 => {
                let jobs = w.jobs(&e);
                if let Some(job) = rng.pick(&jobs) {
                    let ok = rng.chance(85);
                    w.deliver(&mut e, job, ok);
                }
            }
            85..=89 => {
                // A preview read Develop started on its own (not queued).
                let id = rng.pick(&ids).unwrap_or(1);
                let h = iced::widget::image::Handle::from_rgba(1, 1, vec![0u8; 4]);
                w.send(&mut e, Message::WorkingPreviewReady(id, h, None, (1, 1)));
            }
            _ => {
                if tabs {
                    let tab = [AppTab::Library, AppTab::Cull, AppTab::Develop][rng.below(3)];
                    w.send(&mut e, Message::TabChanged(tab));
                } else {
                    let id = rng.pick(&ids).unwrap_or(1);
                    w.send(&mut e, Message::ImageSelected(id));
                }
            }
        }
        w.check(&e).map_err(|why| fail(&w, &e, step, why))?;
    }

    if tabs {
        w.send(&mut e, Message::TabChanged(AppTab::Develop));
        w.check(&e).map_err(|why| fail(&w, &e, steps, why))?;
    }
    w.drain(&mut e, &mut rng).map_err(|why| fail(&w, &e, steps, why))?;
    if let EditorReadiness::Loading(id) = e.editor_readiness {
        return Err(fail(&w, &e, steps, format!("still Loading({id}) with every job delivered")));
    }
    Ok(())
}

fn run_fuzz(seeds: std::ops::Range<u64>, steps: usize, tabs: bool, preload_failures: bool) {
    let failures: Vec<String> =
        seeds.filter_map(|seed| fuzz(seed, steps, tabs, preload_failures).err()).collect();
    if let Some(first) = failures.first() {
        panic!("{} of the seeds failed; first:\n{first}", failures.len());
    }
}

/// Decodes and builds may fail; RAW preloads always succeed.
#[test]
fn fuzz_develop_selection_never_strands_loading() {
    run_fuzz(0..160, 120, false, false);
}

/// The same, with failing preloads (see
/// `a_failed_preload_of_the_selection_fails_it`).
#[test]
fn fuzz_with_failing_preloads_never_strands_loading() {
    run_fuzz(0..160, 120, false, true);
}

/// The same, with Library / Cull / Develop tab switches mixed in (see
/// `selecting_elsewhere_while_develop_loads_reloads_it`).
#[test]
fn fuzz_with_tab_switches_never_strands_loading() {
    run_fuzz(0..160, 120, true, false);
}

/// Found by the fuzz. The selection is already preloading, so Develop waits
/// on that preload instead of decoding it again — and `handle_raw_preloaded`
/// used to only log a failed preload, leaving `Loading` with nothing in
/// flight. Its failure now fails the load, like a direct decode's, and a
/// re-select retries it.
#[test]
fn a_failed_preload_of_the_selection_fails_it() {
    let mut e = develop_with_images(10);
    send(&mut e, Message::ImageSelected(2)); // left to its running preload
    assert!(running_raw_preloads(&e).contains(&2));
    send(&mut e, Message::RawPreloaded(2, Err("unreadable".into())));
    assert!(matches!(e.editor_readiness, EditorReadiness::Failed(2, _)));
    assert_invariants(&e);

    send(&mut e, Message::ImageSelected(3));
    send(&mut e, Message::ImageSelected(2));
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(2));
    assert!(
        e.direct_raw_loads.contains(&2) || running_raw_preloads(&e).contains(&2),
        "a failed preload is never retried"
    );
    assert_invariants(&e);
}

/// Found by the fuzz above. Develop starts loading image 1; the user goes to
/// Library (or Cull) and selects image 2 there — which loads nothing — and
/// 1's decode lands meanwhile, unwanted because 1 is no longer selected. Back
/// on 1 and into Develop, `load_for_tab` used to see "already loading 1",
/// start nothing, and Develop showed "Loading" forever. The selection now
/// drops the stale promise, so Develop loads 1 again (from the cache here).
#[test]
fn selecting_elsewhere_while_develop_loads_reloads_it() {
    let mut e = develop_with_images(10);
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(1));
    send(&mut e, Message::TabChanged(AppTab::Library));
    send(&mut e, Message::ImageSelected(2));
    send(&mut e, Message::RawDataLoaded(1, Ok(raw("1"))));
    send(&mut e, Message::ImageSelected(1));
    send(&mut e, Message::TabChanged(AppTab::Develop));
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(1));
    assert_eq!(e.current_metadata_id, Some(1), "the cached RAW was not built");
    assert_invariants(&e);
}

/// The same trip with 1's decode still running on the way back: Develop
/// waits on that decode instead of starting a second.
#[test]
fn returning_to_an_image_whose_decode_still_runs_reuses_it() {
    let mut e = develop_with_images(10);
    send(&mut e, Message::TabChanged(AppTab::Cull));
    send(&mut e, Message::ImageSelected(2));
    send(&mut e, Message::ImageSelected(1));
    send(&mut e, Message::TabChanged(AppTab::Develop));
    assert_eq!(e.editor_readiness, EditorReadiness::Loading(1));
    assert!(e.direct_raw_loads.contains(&1));
    assert!(!queue(&e).contains(&1), "decoded twice");
    assert_invariants(&e);
    send(&mut e, Message::RawDataLoaded(1, Ok(raw("1"))));
    assert_eq!(e.current_metadata_id, Some(1));
    assert_invariants(&e);
}

/// An image whose direct decode is still running (the user stepped past it)
/// used to be queued again as the new selection's neighbour by
/// `schedule_raw_preloads`, and decoded twice.
#[test]
fn a_running_direct_decode_is_not_queued_again_as_a_preload() {
    let mut e = develop_with_images(10);
    assert!(e.direct_raw_loads.contains(&1));
    send(&mut e, Message::ImageSelected(2)); // 1 is now its neighbour behind
    assert!(e.direct_raw_loads.contains(&1));
    assert!(!queue(&e).contains(&1), "a running decode was queued again: {:?}", queue(&e));
    assert_invariants(&e);

    // Its result is still kept for when the user steps back.
    send(&mut e, Message::RawDataLoaded(1, Ok(raw("1"))));
    assert!(e.raw_cache.contains(&1));
    send(&mut e, Message::ImageSelected(1));
    assert_eq!(e.current_metadata_id, Some(1));
    assert_invariants(&e);
}

/// Setting the RAW preload budget to 0 (Preferences) clears the preload
/// queue; it used to cancel the running preload Develop was waiting on too.
/// That one stays, the neighbours go, and its result completes the load.
#[test]
fn turning_raw_preloads_off_mid_load_keeps_the_awaited_preload() {
    let mut e = develop_with_images(10);
    send(&mut e, Message::ImageSelected(2)); // left to its running preload
    send(&mut e, Message::SetRawPreloadBudget(0.0));
    assert_eq!(queue(&e), vec![2]);
    assert_invariants(&e);
    send(&mut e, Message::RawPreloaded(2, Ok(Arc::new(raw("2")))));
    assert_eq!(e.current_metadata_id, Some(2));
    assert!(e.raw_for(2).is_some(), "the open image's RAW is not pinned");
    assert_invariants(&e);
}
