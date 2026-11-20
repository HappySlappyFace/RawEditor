// Wheel scroll boost (library grid) + filmstrip momentum.
//
// iced 0.13's scrollable hardcodes wheel scrolling at 60px/line with no
// configuration API.
//
// Library grid: we let iced apply its native scroll and nudge it further
// with `scroll_by`, while the grid's own mouse_area reports the cursor over it.
//
// Filmstrip: iced's Stack widget dispatches wheel events top-layer-first and
// stops at the first layer that captures them (see iced_widget::stack::
// on_event — `.find(Captured)` short-circuits a lazy iterator), so
// src/ui/filmstrip.rs stacks a transparent `mouse_area::on_scroll` ABOVE its
// scrollable. That interceptor captures every wheel event before the
// scrollable underneath ever sees it, fully suppressing iced's native jump —
// all filmstrip scrolling below is 100% our own physics, so there's no
// native contribution to blend with and no seam.
//
// The develop preview's own `mouse_area::on_scroll` already consumes wheel
// events for zoom, and everything else (sidebars, library folder list)
// deliberately keeps native speed.

use iced::mouse::ScrollDelta;
use iced::widget::scrollable::{self, AbsoluteOffset};
use iced::Task;

use crate::app::message::{AppTab, Message};
use crate::app::state::RawEditor;
use crate::app::state::{GlideTarget, Modal, ScrollGlide};
use crate::ui::motion::{self, Animated, Spring};

/// Extra multiplier on top of iced's native 60px/line.
const GRID_MULT: f32 = 4.0;

const STRIP_LINE_PX: f32 = 60.0;

// ── Filmstrip momentum physics (all deltaT-driven) ─────────────────────────
//
// Every wheel tick measures how fast the user is actually spinning the wheel
// (pixels-equivalent / real elapsed time since the last tick), and that
// measured speed drives two things: how much velocity gets added, and how
// heavy the resulting glide feels.

/// Clamp bounds for the deltaT used to estimate input speed: a floor so two
/// events arriving almost simultaneously (fast trackpad bursts) don't blow
/// up the rate, and a ceiling that also marks "this is a new gesture" (see
/// `is_new_gesture` below) rather than genuine continuous scrolling.
const MIN_WHEEL_DT: f32 = 0.008;
const GESTURE_GAP: f32 = 0.3;
/// EMA smoothing for the input-speed estimate, so a single outlier tick
/// doesn't spike the computed weight.
const INPUT_SMOOTH: f32 = 0.5;

/// Input speed (px/s) that maps to "fully fast" for the weight interpolation.
const SPEED_REF: f32 = 3500.0;
/// Velocity (px/s) added per (px/s) of measured input speed, per tick.
const IMPULSE_GAIN: f32 = 1.3;
const MAX_SPEED: f32 = 5000.0;
const STOP_SPEED: f32 = 15.0;

/// Decay range (1/s): slow input -> DECAY_SLOW (heavy, glides a long time
/// even though the input itself was gentle); fast input -> DECAY_FAST
/// (light — stops/redirects quickly so a fast flick stays controllable
/// instead of sailing past where you meant to land).
const DECAY_SLOW: f32 = 1.2;
const DECAY_FAST: f32 = 6.5;

/// Library grid boost, applied while the cursor is over the grid.
pub fn handle_wheel(editor: &mut RawEditor, delta: ScrollDelta) -> Task<Message> {
    if editor.active_modal != Modal::None || editor.session.current_tab != AppTab::Library {
        return Task::none();
    }
    let lines = match delta {
        ScrollDelta::Lines { y, .. } => y,
        ScrollDelta::Pixels { y, .. } => y / 60.0,
    };
    if lines == 0.0 {
        return Task::none();
    }

    if editor.cursor_over_grid {
        // The user has taken the wheel: a reveal glide stops where it is.
        cancel_glide(editor, GlideTarget::Grid);
        return scrollable::scroll_by(
            crate::app::views::library::grid_scroll_id(),
            AbsoluteOffset { x: 0.0, y: -(GRID_MULT - 1.0) * 60.0 * lines },
        );
    }
    Task::none()
}

/// Filmstrip wheel input, captured directly by its own overlay (native
/// scrolling never fires — see module docs) so this drives 100% of the motion.
pub fn handle_filmstrip_wheel(editor: &mut RawEditor, delta: ScrollDelta) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    let lines = match delta {
        ScrollDelta::Lines { y, .. } => y,
        ScrollDelta::Pixels { y, .. } => y / 60.0,
    };
    if lines == 0.0 {
        return Task::none();
    }

    cancel_glide(editor, GlideTarget::Filmstrip);

    let now = std::time::Instant::now();
    let raw_dt = editor
        .last_wheel_time
        .map(|t| now.duration_since(t).as_secs_f32());
    editor.last_wheel_time = Some(now);

    // How fast is the wheel actually spinning right now?
    let dt_for_rate = raw_dt.unwrap_or(GESTURE_GAP).clamp(MIN_WHEEL_DT, GESTURE_GAP);
    let raw_rate = (lines.abs() * STRIP_LINE_PX) / dt_for_rate;
    // A real pause (or the very first tick) means the previous estimate is
    // stale — start fresh instead of blending into it.
    let is_new_gesture = raw_dt.map(|d| d >= GESTURE_GAP).unwrap_or(true);
    editor.filmstrip_input_speed = if is_new_gesture {
        raw_rate
    } else {
        editor.filmstrip_input_speed * (1.0 - INPUT_SMOOTH) + raw_rate * INPUT_SMOOTH
    }
    .min(SPEED_REF * 2.0);

    // Opposite direction: cancel the old glide instantly instead of
    // fighting it with a subtracted impulse.
    let dir = -lines.signum();
    if editor.filmstrip_velocity != 0.0 && editor.filmstrip_velocity.signum() != dir {
        editor.filmstrip_velocity = 0.0;
    }
    let impulse = dir * editor.filmstrip_input_speed * IMPULSE_GAIN;
    editor.filmstrip_velocity = (editor.filmstrip_velocity + impulse).clamp(-MAX_SPEED, MAX_SPEED);

    // Weight for this glide: fast input -> low weight (high decay, stops
    // quickly); slow input -> high weight (low decay, glides for a while).
    let t = (editor.filmstrip_input_speed / SPEED_REF).clamp(0.0, 1.0);
    editor.filmstrip_decay = DECAY_SLOW + (DECAY_FAST - DECAY_SLOW) * t;

    // No synchronous scroll_by here: velocity is already at its peak right
    // after the impulse (it only decays from here, never ramps up), so
    // letting the per-frame kinetic tick loop apply it from t=0 is already
    // smooth by construction — a synchronous "catch-up" chunk would just
    // front-load a disproportionately large jump relative to the small
    // per-frame steps that follow, recreating the exact discontinuity this
    // whole interceptor was built to eliminate.
    editor.last_kinetic_tick = Some(now);
    Task::none()
}

pub fn handle_kinetic_tick(editor: &mut RawEditor, now: std::time::Instant) -> Task<Message> {
    let glide = step_glide(editor, now);
    if editor.filmstrip_velocity == 0.0 {
        return glide;
    }
    // Tab switched mid-glide (the filmstrip may not even be mounted) — stop.
    if editor.session.current_tab == AppTab::Library {
        editor.filmstrip_velocity = 0.0;
        editor.last_kinetic_tick = None;
        return Task::none();
    }

    let dt = editor
        .last_kinetic_tick
        .map(|t| now.duration_since(t).as_secs_f32())
        .unwrap_or(1.0 / 60.0)
        .clamp(0.0, 0.05);
    editor.last_kinetic_tick = Some(now);

    let step = editor.filmstrip_velocity * dt;
    editor.filmstrip_velocity *= (-editor.filmstrip_decay * dt).exp();
    if editor.filmstrip_velocity.abs() < STOP_SPEED {
        editor.filmstrip_velocity = 0.0;
        editor.last_kinetic_tick = None;
    }

    scrollable::scroll_by(
        crate::ui::filmstrip::scroll_id(),
        AbsoluteOffset { x: step, y: 0.0 },
    )
}

// ── Reveal glide ───────────────────────────────────────────────────────────
//
// Keyboard navigation scrolls the grid / filmstrip to keep the selection in
// view. A single step eases the strip over by one slot. A *held* arrow key
// retargets on every key repeat, and a spring that settled into each new
// target would surge and stall at the repeat rate and trail the selection
// for as long as the key is down. So the glide tracks instead: it measures
// how fast its destination is advancing and feeds that speed forward
// (`Spring::track`), which lets it catch up and then cruise at the
// selection's own constant speed. On key-up it coasts to rest.
//
// Driven by the same per-frame tick as the filmstrip momentum
// (`subscription` runs it while `scroll_glide` is set).

fn scroll_id(target: GlideTarget) -> scrollable::Id {
    match target {
        GlideTarget::Grid => crate::app::views::library::grid_scroll_id(),
        GlideTarget::Filmstrip => crate::ui::filmstrip::scroll_id(),
    }
}

fn current_offset(editor: &RawEditor, target: GlideTarget) -> f32 {
    match target {
        GlideTarget::Grid => editor.grid_scroll_y,
        GlideTarget::Filmstrip => editor.filmstrip_scroll_x,
    }
}

/// Record `offset` as the scrollable's position — ahead of its own
/// `on_scroll` report, so the very next view builds the rows/slots it is
/// scrolling to.
fn set_offset(editor: &mut RawEditor, target: GlideTarget, offset: f32) {
    match target {
        GlideTarget::Grid => editor.grid_scroll_y = offset,
        GlideTarget::Filmstrip => editor.filmstrip_scroll_x = offset,
    }
}

fn scroll_to(target: GlideTarget, offset: f32) -> Task<Message> {
    let offset = match target {
        GlideTarget::Grid => AbsoluteOffset { x: 0.0, y: offset },
        GlideTarget::Filmstrip => AbsoluteOffset { x: offset, y: 0.0 },
    };
    scrollable::scroll_to(scroll_id(target), offset)
}

/// Where an in-flight glide on `target` will come to rest.
pub fn glide_destination(editor: &RawEditor, target: GlideTarget) -> Option<f32> {
    editor.scroll_glide.filter(|g| g.target == target).map(|g| g.to)
}

/// A glide is done once within half a pixel of its destination and barely
/// moving; anything finer is invisible.
const GLIDE_REST_PX: f32 = 0.5;

/// Farthest a reveal glides, in viewports. Beyond it a glide is a blur of
/// thumbnails the user never wanted to see — each one loaded as it passes —
/// so it jumps instead.
const GLIDE_MAX_VIEWPORTS: f32 = 1.5;

/// Time constant (s) of a hold's speed estimate. Long enough that a steady
/// key repeat reads as a steady speed (each repeat moves the estimate ~3%),
/// and repeats arriving in bursts average out.
const HOLD_TAU: f32 = 1.0;

/// Fastest plausible key repeat, per second (OS repeat rates top out near
/// 50): a cap on the speed estimate, against measurement artefacts.
const MAX_REPEAT_HZ: f32 = 60.0;

/// The visible extent of `target` along its scroll axis.
fn viewport_extent(editor: &RawEditor, target: GlideTarget) -> f32 {
    let (reported, window) = match target {
        GlideTarget::Grid => (editor.grid_viewport.1, editor.window_size.height),
        GlideTarget::Filmstrip => (editor.filmstrip_view_w, editor.window_size.width),
    };
    if reported > 0.0 {
        reported
    } else {
        window
    }
}

/// One step along `target`'s scroll axis: a filmstrip slot, a grid row.
fn step_size(editor: &RawEditor, target: GlideTarget) -> f32 {
    match target {
        GlideTarget::Grid => crate::app::views::library::GridGeometry::of(editor).row_height,
        GlideTarget::Filmstrip => crate::ui::filmstrip::ITEM_WIDTH,
    }
}

/// Retargets closer together than this (seconds) are one held key, and a
/// hold is presumed over once this long passes without one. Scaled by the
/// hold's own cadence: the grid only retargets once per row, a few key
/// repeats apart.
fn hold_gap(glide: &ScrollGlide) -> f32 {
    let cadence = match glide.hold_start {
        Some(start) if glide.retargets > 1 => glide
            .last_retarget
            .map_or(0.0, |last| motion::elapsed(start, last) / (glide.retargets - 1) as f32),
        _ => 0.0,
    };
    (2.5 * cadence).max(0.3)
}

/// How fast the destination is moving (px/s, signed) at `now`, during a
/// hold.
///
/// A time average, not a per-repeat one: the distance the destination has
/// travelled, exponentially decayed over `HOLD_TAU`, over the time it had
/// to do it in, decayed the same way. Key repeats are periodic, but they
/// reach `update` in bursts whenever the UI thread is busy — three in 8ms,
/// then nothing for 90 — and averaging per-repeat speeds reads such a burst
/// as 40 000 px/s, sending the strip racing off ahead of the selection.
fn hold_rate(glide: &ScrollGlide, now: std::time::Instant, step: f32) -> f32 {
    let (Some(start), Some(last)) = (glide.hold_start, glide.last_retarget) else {
        return 0.0;
    };
    if glide.retargets < 2 {
        return 0.0;
    }
    let distance = glide.flux * (-motion::elapsed(last, now) / HOLD_TAU).exp();
    let time = HOLD_TAU * (1.0 - (-motion::elapsed(start, now) / HOLD_TAU).exp());
    glide.direction * (distance / time.max(1e-3)).min(step * MAX_REPEAT_HZ)
}

/// Scroll `target` to `offset`: a glide if `animate` and it is a short
/// move, otherwise a jump (also under reduced motion). A glide already in
/// flight is re-aimed without losing its speed.
pub fn glide_to(
    editor: &mut RawEditor,
    target: GlideTarget,
    offset: f32,
    animate: bool,
) -> Task<Message> {
    glide_to_at(editor, target, offset, animate, std::time::Instant::now())
}

fn glide_to_at(
    editor: &mut RawEditor,
    target: GlideTarget,
    offset: f32,
    animate: bool,
    now: std::time::Instant,
) -> Task<Message> {
    let offset = offset.max(0.0);
    let from = editor
        .scroll_glide
        .filter(|g| g.target == target)
        .map_or_else(|| current_offset(editor, target), |g| g.offset());
    let short = (offset - from).abs() <= GLIDE_MAX_VIEWPORTS * viewport_extent(editor, target);
    if !animate || !short || motion::reduced() {
        editor.scroll_glide = None;
        set_offset(editor, target, offset);
        return scroll_to(target, offset);
    }
    match &mut editor.scroll_glide {
        // Re-aim: same position and velocity, measured from the new end.
        Some(glide) if glide.target == target => {
            observe_retarget(glide, offset, now);
            glide.displacement.value = glide.offset() - offset;
            glide.to = offset;
        }
        _ => {
            let mut displacement = Animated::new(from - offset);
            displacement.set(0.0);
            editor.scroll_glide = Some(ScrollGlide {
                target,
                to: offset,
                displacement,
                rate: 0.0,
                direction: (offset - from).signum(),
                hold_start: Some(now),
                flux: 0.0,
                retargets: 1,
                last_retarget: Some(now),
                last_tick: None,
            });
        }
    }
    Task::none()
}

/// Fold a retarget into the hold: one in the same direction soon after the
/// last means the key is still down.
fn observe_retarget(glide: &mut ScrollGlide, to: f32, now: std::time::Instant) {
    let delta = to - glide.to;
    if delta == 0.0 {
        return;
    }
    let held = glide.last_retarget.is_some_and(|last| {
        motion::elapsed(last, now) < hold_gap(glide) && delta.signum() == glide.direction
    });
    if held {
        let since = glide.last_retarget.map_or(0.0, |last| motion::elapsed(last, now));
        glide.flux = glide.flux * (-since / HOLD_TAU).exp() + delta.abs();
        glide.retargets += 1;
    } else {
        // A fresh press, or a reversal: nothing to feed forward yet.
        glide.hold_start = Some(now);
        glide.flux = 0.0;
        glide.retargets = 1;
        glide.direction = delta.signum();
    }
    glide.last_retarget = Some(now);
}

/// End of a hold: stop feeding its speed forward and coast to rest.
///
/// The stop goes where a critically damped spring let go at this velocity
/// comes to rest by itself, `x + v/ω`, so the strip decelerates smoothly and
/// never reverses — the destination moves on past the selection rather than
/// the strip pulling back to it. Clamped so the selection is never left
/// short of view, nor coasted out of the far side.
fn release(glide: &mut ScrollGlide, step: f32, extent: f32) {
    if glide.hold_start.is_none() {
        return;
    }
    glide.hold_start = None;
    glide.rate = 0.0;
    let direction = glide.direction;
    let x = glide.offset();
    let rest = x + glide.displacement.velocity / Spring::FOLLOW.omega();
    let past = ((rest - glide.to) * direction).clamp(0.0, (extent - step).max(0.0));
    let to = (glide.to + direction * past).max(0.0);
    glide.displacement.value = x - to;
    glide.to = to;
}

/// An arrow key came up: the hold (if any) is over.
pub fn handle_navigation_released(editor: &mut RawEditor) -> Task<Message> {
    if let Some(mut glide) = editor.scroll_glide {
        let step = step_size(editor, glide.target);
        let extent = viewport_extent(editor, glide.target);
        release(&mut glide, step, extent);
        editor.scroll_glide = Some(glide);
    }
    Task::none()
}

fn cancel_glide(editor: &mut RawEditor, target: GlideTarget) {
    if editor.scroll_glide.is_some_and(|g| g.target == target) {
        editor.scroll_glide = None;
    }
}

fn step_glide(editor: &mut RawEditor, now: std::time::Instant) -> Task<Message> {
    let Some(mut glide) = editor.scroll_glide else {
        return Task::none();
    };
    // The scrollable may not even be mounted any more.
    let mounted = match glide.target {
        GlideTarget::Grid => editor.session.current_tab == AppTab::Library,
        GlideTarget::Filmstrip => matches!(editor.session.current_tab, AppTab::Cull | AppTab::Develop),
    };
    if !mounted || editor.active_modal != Modal::None {
        editor.scroll_glide = None;
        return Task::none();
    }
    let step = step_size(editor, glide.target);
    let extent = viewport_extent(editor, glide.target);
    // Key-up normally ends a hold; this catches one whose release never
    // arrived (focus lost mid-hold).
    if glide.last_retarget.is_some_and(|t| motion::elapsed(t, now) > hold_gap(&glide)) {
        release(&mut glide, step, extent);
    }

    let dt = motion::frame_dt(glide.last_tick, now);
    glide.last_tick = Some(now);
    glide.rate = hold_rate(&glide, now, step);
    // While held, aim where the selection is *going*. The destination
    // advances in whole steps, one per key repeat, and a spring chasing that
    // staircase lurches at the repeat rate. Between repeats the target is
    // extrapolated at the hold's speed instead (never more than a step
    // ahead), so the spring sees a smooth ramp and the strip glides at
    // constant speed — the selection meeting the edge as each repeat lands.
    // Sampled mid-frame: the spring holds it for the whole step, and an
    // end-of-frame sample would bias the strip half a frame ahead.
    let lead = match glide.last_retarget {
        Some(at) if glide.rate != 0.0 => {
            let since = (motion::elapsed(at, now) - dt / 2.0).max(0.0);
            (glide.rate * since).clamp(-step, step)
        }
        _ => 0.0,
    };
    glide.displacement.target = lead;
    // Governor: if the strip is already past where the selection is
    // heading (repeats stalled, or arrived in a burst), stop pushing it
    // further — fade the feed-forward out over half a step.
    let ahead = (glide.displacement.value - lead) * glide.direction;
    let feed = glide.rate * (1.0 - ahead / (step / 2.0)).clamp(0.0, 1.0);
    let d = &mut glide.displacement;
    Spring::FOLLOW.track(&mut d.value, &mut d.velocity, lead, feed, dt);

    let at_rest = glide.hold_start.is_none()
        && glide.displacement.value.abs() < GLIDE_REST_PX
        && glide.displacement.velocity.abs() < GLIDE_REST_PX * 10.0;
    if at_rest {
        glide.displacement.snap(0.0);
    }
    let offset = glide.offset().max(0.0);
    editor.scroll_glide = (!at_rest).then_some(glide);
    set_offset(editor, glide.target, offset);
    scroll_to(glide.target, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> RawEditor {
        RawEditor::new().0
    }

    /// Re-aiming a glide keeps one continuous motion rather than restarting.
    #[test]
    fn re_aiming_a_glide_keeps_its_speed() {
        let mut e = editor();
        e.session.current_tab = AppTab::Develop;
        let _ = glide_to(&mut e, GlideTarget::Filmstrip, 1000.0, true);
        let start = std::time::Instant::now();
        for i in 0..6 {
            let _ = step_glide(&mut e, start + std::time::Duration::from_millis(16 * i));
        }
        let before = e.scroll_glide.unwrap();
        // Displacement shrinks towards 0 from below: moving forwards.
        assert!(before.displacement.velocity > 0.0);
        let _ = glide_to(&mut e, GlideTarget::Filmstrip, 1200.0, true);
        let after = e.scroll_glide.unwrap();
        assert_eq!(after.displacement.velocity, before.displacement.velocity);
        assert_eq!(after.to, 1200.0);
        assert!((after.offset() - before.offset()).abs() < 1e-3, "position jumped");
    }

    /// The regression behind a crash: at six-figure offsets (a long
    /// filmstrip) a spring on the raw offset stalled a fraction of a pixel
    /// short and glided forever, issuing `scroll_to` every frame — which
    /// panicked iced_aw the moment a dropdown was opened.
    #[test]
    fn a_glide_ends_even_at_huge_offsets() {
        let mut e = editor();
        e.session.current_tab = AppTab::Develop;
        e.filmstrip_scroll_x = 171_000.0;
        let _ = glide_to(&mut e, GlideTarget::Filmstrip, 172_345.67, true);
        let start = std::time::Instant::now();
        let mut frames = 0;
        while e.scroll_glide.is_some() {
            frames += 1;
            assert!(frames < 120, "glide never ended: {:?}", e.scroll_glide);
            let _ = step_glide(&mut e, start + std::time::Duration::from_micros(16_667 * frames));
        }
        assert_eq!(e.filmstrip_scroll_x, 172_345.67);
    }

    #[test]
    fn a_glide_lands_on_its_target_and_ends() {
        let mut e = editor();
        e.session.current_tab = AppTab::Library;
        let _ = glide_to(&mut e, GlideTarget::Grid, 640.0, true);
        let start = std::time::Instant::now();
        let mut frames = 0;
        while e.scroll_glide.is_some() {
            frames += 1;
            assert!(frames < 240, "glide never ended");
            let _ = step_glide(&mut e, start + std::time::Duration::from_micros(16_667 * frames));
        }
        assert_eq!(e.grid_scroll_y, 640.0);
    }

    /// A long reveal — or any reveal on a tab switch — jumps rather than
    /// sweeping past every thumbnail in between.
    #[test]
    fn long_or_instant_reveals_jump() {
        let mut e = editor();
        e.session.current_tab = AppTab::Develop;
        e.filmstrip_view_w = 1600.0;
        let _ = glide_to(&mut e, GlideTarget::Filmstrip, 50_000.0, true);
        assert!(e.scroll_glide.is_none());
        assert_eq!(e.filmstrip_scroll_x, 50_000.0);

        let _ = glide_to(&mut e, GlideTarget::Filmstrip, 50_400.0, false);
        assert!(e.scroll_glide.is_none());
        assert_eq!(e.filmstrip_scroll_x, 50_400.0);

        let _ = glide_to(&mut e, GlideTarget::Filmstrip, 51_000.0, true);
        assert!(e.scroll_glide.is_some(), "a short in-place reveal still glides");
    }

    /// Simulate holding an arrow key: the selection advances one slot per
    /// key repeat (every `repeat` s) for `hold` s, with frames at 60fps.
    /// Returns per-frame `(time, offset, destination)`.
    fn hold(e: &mut RawEditor, repeat: f32, hold: f32, total: f32) -> Vec<(f32, f32, f32)> {
        let start = std::time::Instant::now();
        let at = |t: f32| start + std::time::Duration::from_secs_f32(t);
        let mut to = 0.0;
        let mut next_repeat = repeat;
        let mut frames = Vec::new();
        let mut t = 0.0;
        let mut released = false;
        while t < total {
            t += 1.0 / 60.0;
            while next_repeat <= t && next_repeat <= hold {
                to += crate::ui::filmstrip::ITEM_WIDTH;
                let _ = glide_to_at(e, GlideTarget::Filmstrip, to, true, at(next_repeat));
                next_repeat += repeat;
            }
            if !released && t > hold {
                released = true;
                let _ = handle_navigation_released(e);
            }
            let _ = step_glide(e, at(t));
            frames.push((t, e.filmstrip_scroll_x, to));
        }
        frames
    }

    fn develop() -> RawEditor {
        let mut e = editor();
        e.session.current_tab = AppTab::Develop;
        e.filmstrip_view_w = 1600.0;
        e
    }

    /// The point of the tracking glide: holding an arrow key, the strip
    /// catches up with the selection and then moves at its constant speed,
    /// keeping it in view — not surging at each repeat and trailing behind.
    #[test]
    fn holding_an_arrow_key_cruises_at_the_selections_speed() {
        let mut e = develop();
        let repeat = 1.0 / 30.0;
        let frames = hold(&mut e, repeat, 2.0, 2.0);
        let speed = crate::ui::filmstrip::ITEM_WIDTH / repeat;
        // Caught up by one second in; still held.
        let settled: Vec<_> = frames.iter().filter(|(t, ..)| *t > 1.0 && *t <= 2.0).collect();
        for &&(t, offset, to) in &settled {
            // >= 0: the selection is fully in view. The strip may anticipate
            // the next repeat, but by no more than a slot (a few px of
            // frame/repeat alignment aside).
            let ahead = offset - to;
            assert!(
                (-2.0..=crate::ui::filmstrip::ITEM_WIDTH + 8.0).contains(&ahead),
                "selection out of place at {t:.2}s: strip {ahead:.0}px from it"
            );
        }
        let per_frame: Vec<f32> = settled.windows(2).map(|w| (w[1].1 - w[0].1) * 60.0).collect();
        let mean = per_frame.iter().sum::<f32>() / per_frame.len() as f32;
        assert!((mean - speed).abs() < speed * 0.03, "cruising at {mean:.0}, selection at {speed:.0}");
        let worst = per_frame.iter().map(|v| (v - mean).abs()).fold(0.0, f32::max);
        assert!(worst < mean * 0.03, "speed not constant: off by up to {worst:.0}px/s");
    }

    /// Key repeats reach `update` in bursts when the UI thread is busy —
    /// several within milliseconds, then a long gap. The speed estimate has
    /// to see the average rate through that, or the strip races ahead of
    /// the selection (a burst 4ms apart reads as 40 000 px/s).
    #[test]
    fn bursty_key_repeats_still_cruise_at_the_average_rate() {
        let mut e = develop();
        let start = std::time::Instant::now();
        let at = |t: f32| start + std::time::Duration::from_secs_f32(t);
        let step = crate::ui::filmstrip::ITEM_WIDTH;
        // Bursts of three, 4ms apart, every 100ms: 30 retargets a second.
        let mut repeats: Vec<f32> = Vec::new();
        for burst in 0..20 {
            for k in 0..3 {
                repeats.push(0.1 * burst as f32 + 0.004 * k as f32);
            }
        }
        let (mut to, mut next, mut t) = (0.0, 0, 0.0);
        let mut worst_ahead: f32 = 0.0;
        let mut rates = Vec::new();
        while t < 2.0 {
            t += 1.0 / 60.0;
            while next < repeats.len() && repeats[next] <= t {
                to += step;
                let _ = glide_to_at(&mut e, GlideTarget::Filmstrip, to, true, at(repeats[next]));
                next += 1;
            }
            let _ = step_glide(&mut e, at(t));
            if t > 1.0 {
                worst_ahead = worst_ahead.max(e.filmstrip_scroll_x - to);
                rates.push(e.scroll_glide.expect("still holding").rate);
            }
        }
        let truth = 30.0 * step;
        let mean = rates.iter().sum::<f32>() / rates.len() as f32;
        let worst = rates.iter().map(|r| (r - truth).abs()).fold(0.0, f32::max);
        assert!((mean - truth).abs() < truth * 0.1, "estimated {mean:.0}px/s, actual {truth:.0}");
        assert!(worst < truth * 0.3, "estimate swung {worst:.0}px/s off");
        assert!(worst_ahead < 2.0 * step, "strip ran {worst_ahead:.0}px ahead of the selection");
    }

    #[test]
    fn releasing_the_key_coasts_to_a_stop_without_reversing() {
        let mut e = develop();
        let frames = hold(&mut e, 1.0 / 30.0, 1.0, 2.5);
        assert!(e.scroll_glide.is_none(), "never came to rest");
        let after: Vec<_> = frames.iter().filter(|(t, ..)| *t > 1.0).collect();
        for w in after.windows(2) {
            assert!(w[1].1 >= w[0].1 - 1e-3, "reversed at {:.2}s", w[1].0);
        }
        let (_, rest, last) = *frames.last().unwrap();
        assert!(rest >= last, "stopped short of the selection");
        assert!(rest <= last + 1600.0 - crate::ui::filmstrip::ITEM_WIDTH, "coasted it out of view");
    }

    /// No key-up event (focus lost mid-hold): the glide still ends.
    #[test]
    fn a_hold_without_a_release_still_comes_to_rest() {
        let mut e = develop();
        let start = std::time::Instant::now();
        let at = |t: f32| start + std::time::Duration::from_secs_f32(t);
        let mut to = 0.0;
        for k in 1..=20 {
            to += crate::ui::filmstrip::ITEM_WIDTH;
            let _ = glide_to_at(&mut e, GlideTarget::Filmstrip, to, true, at(k as f32 / 30.0));
            let _ = step_glide(&mut e, at(k as f32 / 30.0));
        }
        let mut frame = 0;
        while e.scroll_glide.is_some() {
            frame += 1;
            assert!(frame < 300, "glide never ended");
            let _ = step_glide(&mut e, at(20.0 / 30.0 + frame as f32 / 60.0));
        }
        assert!(e.filmstrip_scroll_x >= to);
    }

    /// One press is still a short ease onto exactly one slot further.
    #[test]
    fn a_single_step_eases_over_one_slot_exactly() {
        let mut e = develop();
        let frames = hold(&mut e, 0.1, 0.1, 1.0);
        assert!(e.scroll_glide.is_none());
        assert_eq!(frames.last().unwrap().1, crate::ui::filmstrip::ITEM_WIDTH);
    }

    #[test]
    fn wheel_input_cancels_a_glide() {
        let mut e = editor();
        e.session.current_tab = AppTab::Develop;
        let _ = glide_to(&mut e, GlideTarget::Filmstrip, 500.0, true);
        let _ = handle_filmstrip_wheel(&mut e, ScrollDelta::Lines { x: 0.0, y: -1.0 });
        assert!(e.scroll_glide.is_none());
    }

    #[test]
    fn leaving_the_tab_drops_the_glide() {
        let mut e = editor();
        e.session.current_tab = AppTab::Develop;
        let _ = glide_to(&mut e, GlideTarget::Filmstrip, 500.0, true);
        e.session.current_tab = AppTab::Library;
        let _ = step_glide(&mut e, std::time::Instant::now());
        assert!(e.scroll_glide.is_none());
    }
}

