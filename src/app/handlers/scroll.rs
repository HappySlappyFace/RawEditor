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
const STRIP_MULT: f32 = 2.0;
/// Momentum impulse added per wheel line over the filmstrip, px/s.
const STRIP_IMPULSE: f32 = 500.0;
/// Exponential velocity decay, 1/s.
const DECAY: f32 = 4.0;
const STOP_SPEED: f32 = 20.0;
const MAX_SPEED: f32 = 4000.0;

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
                editor.filmstrip_velocity =
                    (editor.filmstrip_velocity - lines * STRIP_IMPULSE).clamp(-MAX_SPEED, MAX_SPEED);
                editor.last_kinetic_tick = None; // fresh dt on the next tick
                return scrollable::scroll_by(
                    crate::ui::filmstrip::scroll_id(),
                    AbsoluteOffset { x: -(STRIP_MULT - 1.0) * 60.0 * lines, y: 0.0 },
                );
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
    editor.filmstrip_velocity *= (-DECAY * dt).exp();
    if editor.filmstrip_velocity.abs() < STOP_SPEED {
        editor.filmstrip_velocity = 0.0;
        editor.last_kinetic_tick = None;
    }

    scrollable::scroll_by(
        crate::ui::filmstrip::scroll_id(),
        AbsoluteOffset { x: step, y: 0.0 },
    )
}
