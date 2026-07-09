// Wheel scroll boost + filmstrip momentum.
//
// iced 0.13's scrollable hardcodes wheel scrolling at 60px/line with no
// configuration API. Rather than fight that, we let iced apply its native
// scroll and then nudge the same scrollable further with `scroll_by`,
// routed by which region the (globally-tracked) cursor is currently over.
// The develop preview's own `mouse_area::on_scroll` already consumes wheel
// events for zoom, and everything else (sidebars, library folder list)
// deliberately keeps native speed — only the library grid and the
// cull/develop filmstrip get boosted.

use iced::mouse::ScrollDelta;
use iced::widget::scrollable::{self, AbsoluteOffset};
use iced::Task;

use crate::app::message::{AppTab, Message};
use crate::app::state::RawEditor;
use crate::app::state::Modal;

const TITLE_BAR_H: f32 = 35.0;
const FILMSTRIP_H: f32 = 115.0;
const LIB_SIDEBAR_W: f32 = 250.0;
const LIB_TOOLBAR_H: f32 = 56.0;
const LIB_STATUS_H: f32 = 42.0;

/// Extra multiplier on top of iced's native 60px/line.
const GRID_MULT: f32 = 4.0;

const STRIP_LINE_PX: f32 = 60.0;

// ── Filmstrip momentum physics (all deltaT-driven — see handle_wheel) ─────
//
// Every wheel tick over the filmstrip measures how fast the user is actually
// spinning the wheel (pixels-equivalent / real elapsed time since the last
// tick), and that measured speed drives two things: how much velocity gets
// added, and how heavy the resulting glide feels.

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
const SPEED_REF: f32 = 2500.0;
/// Velocity (px/s) added per (px/s) of measured input speed, per tick.
const IMPULSE_GAIN: f32 = 0.9;
const MAX_SPEED: f32 = 5000.0;
const STOP_SPEED: f32 = 15.0;

/// Decay range (1/s): slow input -> DECAY_SLOW (heavy, glides a long time
/// even though the input itself was gentle); fast input -> DECAY_FAST
/// (light — stops/redirects quickly so a fast flick stays controllable
/// instead of sailing past where you meant to land).
const DECAY_SLOW: f32 = 1.4;
const DECAY_FAST: f32 = 9.0;

pub fn handle_global_cursor_moved(editor: &mut RawEditor, pos: iced::Point) -> Task<Message> {
    editor.global_cursor = pos;
    Task::none()
}

pub fn handle_window_resized(editor: &mut RawEditor, size: iced::Size) -> Task<Message> {
    editor.window_size = size;
    Task::none()
}

pub fn handle_wheel(editor: &mut RawEditor, delta: ScrollDelta) -> Task<Message> {
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

    let c = editor.global_cursor;
    let w = editor.window_size;

    match editor.current_tab {
        AppTab::Library => {
            let over_grid = c.x > LIB_SIDEBAR_W
                && c.y > TITLE_BAR_H + LIB_TOOLBAR_H
                && c.y < w.height - LIB_STATUS_H;
            if over_grid {
                return scrollable::scroll_by(
                    crate::app::views::library::grid_scroll_id(),
                    AbsoluteOffset { x: 0.0, y: -(GRID_MULT - 1.0) * 60.0 * lines },
                );
            }
        }
        AppTab::Cull | AppTab::Develop => {
            // Filmstrip band only — never the develop preview (wheel = zoom
            // there) or the develop sidebar (native speed stays).
            if c.y > w.height - FILMSTRIP_H {
                let now = std::time::Instant::now();
                let raw_dt = editor
                    .last_wheel_time
                    .map(|t| now.duration_since(t).as_secs_f32());
                editor.last_wheel_time = Some(now);

                // How fast is the wheel actually spinning right now?
                let dt_for_rate = raw_dt.unwrap_or(GESTURE_GAP).clamp(MIN_WHEEL_DT, GESTURE_GAP);
                let raw_rate = (lines.abs() * STRIP_LINE_PX) / dt_for_rate;
                // A real pause (or the very first tick) means the previous
                // estimate is stale — start fresh instead of blending into it.
                let is_new_gesture = raw_dt.map(|d| d >= GESTURE_GAP).unwrap_or(true);
                editor.filmstrip_input_speed = if is_new_gesture {
                    raw_rate
                } else {
                    editor.filmstrip_input_speed * (1.0 - INPUT_SMOOTH) + raw_rate * INPUT_SMOOTH
                }
                .min(SPEED_REF * 2.0);

                // Opposite direction: cancel the old glide instantly instead
                // of fighting it with a subtracted impulse.
                let dir = -lines.signum();
                if editor.filmstrip_velocity != 0.0 && editor.filmstrip_velocity.signum() != dir {
                    editor.filmstrip_velocity = 0.0;
                }
                let impulse = dir * editor.filmstrip_input_speed * IMPULSE_GAIN;
                editor.filmstrip_velocity =
                    (editor.filmstrip_velocity + impulse).clamp(-MAX_SPEED, MAX_SPEED);

                // Weight for this glide: fast input -> low weight (high decay,
                // stops quickly); slow input -> high weight (low decay, glides).
                let t = (editor.filmstrip_input_speed / SPEED_REF).clamp(0.0, 1.0);
                editor.filmstrip_decay = DECAY_SLOW + (DECAY_FAST - DECAY_SLOW) * t;

                // Anchor the next kinetic tick's dt to this exact moment
                // (more accurate than assuming a nominal frame time), and
                // let the tick loop itself render the resulting scroll —
                // no separate instant nudge needed.
                editor.last_kinetic_tick = Some(now);
            }
        }
    }
    Task::none()
}

pub fn handle_kinetic_tick(editor: &mut RawEditor, now: std::time::Instant) -> Task<Message> {
    if editor.filmstrip_velocity == 0.0 {
        return Task::none();
    }
    // Tab switched mid-glide (the filmstrip may not even be mounted) — stop.
    if editor.current_tab == AppTab::Library {
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
