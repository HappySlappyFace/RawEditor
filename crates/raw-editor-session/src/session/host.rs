//! The simplest host for a session: run its work to completion, one piece
//! at a time, on the calling thread. For front ends with no executor loop
//! of their own to fold effects into (a headless probe, a phone's render
//! thread); the desktop runs effects as iced tasks instead.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use super::{CacheStep, Handled, LoadJob, Session, SessionEvent};
use crate::Effects;

/// Run `effects` and everything they lead to — each result handed back to
/// the session, plus the background loads it lists (`load_jobs`), each run
/// once — until nothing is left. `timed` hears each piece of work's label
/// and how long it took. Events only a front end can act on, other than
/// the cache workers' (handled here), are logged and dropped.
pub fn run(
    session: &mut Session,
    rt: &tokio::runtime::Runtime,
    effects: Effects<SessionEvent>,
    mut timed: impl FnMut(&'static str, Duration),
) -> Result<(), String> {
    let mut queue: VecDeque<_> = effects.into_iter().map(|e| (e.label, e.future)).collect();
    let mut started: HashSet<LoadJob> = HashSet::new();
    let mut steps = 0;
    loop {
        if queue.is_empty() {
            let Some(job) = session.load_jobs().find(|j| !started.contains(j)) else { break };
            started.insert(job.clone());
            let label = match &job {
                LoadJob::Preview(..) => "load preview",
                LoadJob::Raw(..) => "load raw",
                LoadJob::Proxy(..) => "load proxy",
            };
            queue.push_back((label, Box::pin(job.run())));
        }
        let Some((label, future)) = queue.pop_front() else { break };
        steps += 1;
        if steps > 10_000 {
            return Err("the session's work never settled".into());
        }
        let t = Instant::now();
        let event = rt.block_on(future);
        timed(label, t.elapsed());
        queue.extend(handle(session, event).into_iter().map(|e| (e.label, e.future)));
    }
    Ok(())
}

/// Hand one finished piece of work back to the session, as a front end
/// with no dialogs of its own does: the cache workers' results are acted
/// on, other front-end events are logged and dropped. For hosts that run
/// effects in the background and collect their results as they come.
pub fn handle(session: &mut Session, event: SessionEvent) -> Effects<SessionEvent> {
    match session.handle(event) {
        Handled::Done(fx) => fx,
        Handled::FrontEnd(event) => match *event {
            SessionEvent::CacheProcessed(result) => match session.cache_processed(result) {
                CacheStep::Continue(fx) => fx,
                CacheStep::Retired | CacheStep::Finished => Effects::none(),
            },
            other => {
                tracing::warn!("a front-end event this host ignores: {other:?}");
                Effects::none()
            }
        },
    }
}
