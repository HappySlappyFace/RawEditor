use iced::{Task, Point};
use crate::app::state::{RawEditor, DragMode};
use crate::app::message::{Message, AppTab};
use crate::app::update::run;
use crate::session::{Moved, SelectionChange, TabLoad};

pub fn handle_image_selected(editor: &mut RawEditor, image_id: i64) -> Task<Message> {
    select_image(editor, image_id, true)
}

/// Selection the app makes itself (after a delete, auto-advance, a marked
/// image): never reads the modifier keys, which may still be held from the
/// shortcut that caused it and would turn "move on" into a range select or
/// toggle the image out of the selection.
pub fn handle_image_moved(editor: &mut RawEditor, image_id: i64) -> Task<Message> {
    select_image(editor, image_id, false)
}

fn select_image(editor: &mut RawEditor, image_id: i64, from_click: bool) -> Task<Message> {
    // End an AI-select session where the image is left, while the params are
    // still this image's (the clean-up edits masks by slot index).
    let ai_done = if editor.session.selected_image_id != Some(image_id) {
        crate::app::handlers::ai_mask::handle_done(editor)
    } else {
        Task::none()
    };
    // Saves the outgoing image's edits and thumbnail: before anything moves.
    let persist = run(editor.session.leave_image(image_id));
    let shift = from_click && editor.last_modifiers.shift();
    let command = from_click && editor.last_modifiers.command();

    let change = if shift {
        // Range select from the anchor. Falls back to a plain click when
        // there is no anchor yet (first ever click held Shift).
        if let Some(ids) = select_range(editor, image_id) {
            // Shift alone replaces the selection; Shift+Ctrl adds to it.
            SelectionChange::Range { ids, add: command }
        } else {
            editor.selection_anchor = Some(image_id);
            SelectionChange::Only
        }
    } else if command {
        editor.selection_anchor = Some(image_id);
        SelectionChange::Toggle
    } else {
        editor.selection_anchor = Some(image_id);
        SelectionChange::Only
    };
    editor.session.change_selection(image_id, change);
    let load = run(editor.session.open_image(image_id));
    let reveal = reveal_selection(editor);
    Task::batch([ai_done, persist, load, reveal])
}

/// Follow the session's move of the selection.
pub fn apply_moved(moved: Moved) -> Task<Message> {
    match moved {
        Moved::Stay | Moved::Cleared => Task::none(),
        Moved::To(next_id) => Task::done(Message::ImageMoved(next_id)),
    }
}

/// Scroll the current tab's grid or filmstrip so the selection is visible.
///
/// Arrow-key navigation used to walk straight off the edge of both: the
/// selection moved, the view did not. A no-op when the selection is already
/// in view, so a click never makes anything jump.
pub fn reveal_selection(editor: &mut RawEditor) -> Task<Message> {
    reveal(editor, true)
}

/// [`reveal_selection`] without the glide: for a tab switch, whose grid /
/// filmstrip has just been rebuilt at offset 0 — gliding from there would
/// sweep past (and load) every thumbnail between the start and the
/// selection, and there is no previous position for the eye to follow.
pub fn reveal_selection_instantly(editor: &mut RawEditor) -> Task<Message> {
    reveal(editor, false)
}

fn reveal(editor: &mut RawEditor, animate: bool) -> Task<Message> {
    use crate::app::handlers::scroll::{glide_destination, glide_to};
    use crate::app::state::GlideTarget;

    let Some(selected) = editor.session.selected_image_id else {
        return Task::none();
    };
    let Some(index) = editor.session.displayed_ids().iter().position(|&id| id == selected) else {
        return Task::none();
    };
    // "Is it in view?" is asked of where the view is *heading*: mid-glide,
    // the current offset is a transient, and holding an arrow key would
    // otherwise keep re-aiming at stale positions.
    match editor.session.current_tab {
        AppTab::Library => {
            let geom = crate::app::views::library::GridGeometry::of(editor);
            let from = glide_destination(editor, GlideTarget::Grid).unwrap_or(editor.grid_scroll_y);
            if let Some(y) = geom.scroll_to_reveal(index / geom.columns, from) {
                return glide_to(editor, GlideTarget::Grid, y, animate);
            }
        }
        AppTab::Cull | AppTab::Develop => {
            let view_width = if editor.filmstrip_view_w > 0.0 {
                editor.filmstrip_view_w
            } else {
                editor.window_size.width
            };
            let from = glide_destination(editor, GlideTarget::Filmstrip)
                .unwrap_or(editor.filmstrip_scroll_x);
            // Stepping through: just far enough. Arriving: centred.
            let how = if animate {
                crate::ui::filmstrip::Reveal::Edge
            } else {
                crate::ui::filmstrip::Reveal::Center
            };
            if let Some(x) = crate::ui::filmstrip::scroll_to_reveal(index, from, view_width, how) {
                // A reveal overrides any momentum glide.
                editor.filmstrip_velocity = 0.0;
                editor.last_kinetic_tick = None;
                return glide_to(editor, GlideTarget::Filmstrip, x, animate);
            }
        }
        AppTab::Export => {}
    }
    Task::none()
}

/// Library: a double-clicked thumbnail opens in Develop. The double click's
/// first press has already selected it (a plain click); the select here
/// covers a double click that arrives without one.
pub fn handle_open_in_develop(editor: &mut RawEditor, image_id: i64) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    let select = if editor.session.selected_image_id != Some(image_id) {
        handle_image_selected(editor, image_id)
    } else {
        Task::none()
    };
    Task::batch([select, handle_tab_changed(editor, AppTab::Develop)])
}

pub fn handle_tab_changed(editor: &mut RawEditor, tab: AppTab) -> Task<Message> {
    if editor.session.current_tab != tab {
        // A tab switch rebuilds the grid / filmstrip scrollables from scratch,
        // at offset 0. Match that before anything virtualises against stale
        // offsets (which would build slots far from what is on screen: a
        // blank strip). Re-clicking the active tab rebuilds nothing, so it
        // must not reset them either.
        editor.grid_scroll_y = 0.0;
        editor.filmstrip_scroll_x = 0.0;
        editor.filmstrip_velocity = 0.0;
        editor.last_kinetic_tick = None;
    }
    let persist = run(editor.session.set_tab(tab));
    let reveal = reveal_selection_instantly(editor);
    let load = match editor.session.load_for_tab(tab) {
        TabLoad::Nothing => Task::none(),
        TabLoad::Moved(moved) => apply_moved(moved),
        TabLoad::Load(load) => run(load),
    };
    Task::batch([persist, reveal, load])
}

/// Ids between the selection anchor and `to_id` inclusive, in display order.
///
/// Returns `None` when there is no anchor, or when either endpoint is not
/// currently displayed — a range spanning images the active filter is hiding
/// would select things the user cannot see.
fn select_range(editor: &RawEditor, to_id: i64) -> Option<Vec<i64>> {
    let anchor = editor.selection_anchor?;
    let ids = editor.session.displayed_ids();
    let a = ids.iter().position(|id| *id == anchor)?;
    let b = ids.iter().position(|id| *id == to_id)?;
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    Some(ids[lo..=hi].to_vec())
}

/// Ctrl+A — select every image the current tab is displaying.
pub fn handle_select_all(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    editor.session.select_all();
    Task::none()
}

/// If Develop is the active tab and its selection is marked for removal,
/// move on to the next unmarked image (`Session::ensure_selection_not_marked`).
pub fn ensure_develop_selection_not_marked(editor: &mut RawEditor) -> Task<Message> {
    apply_moved(editor.session.ensure_selection_not_marked())
}

/// The next (or previous) image the current tab displays.
pub(crate) fn step_selection(editor: &RawEditor, forward: bool) -> Option<i64> {
    editor.session.step_selection(forward)
}

pub fn handle_select_next_image(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    match step_selection(editor, true) {
        Some(next) => Task::done(Message::ImageMoved(next)),
        None => Task::none(),
    }
}

pub fn handle_select_previous_image(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    match step_selection(editor, false) {
        Some(prev) => Task::done(Message::ImageMoved(prev)),
        None => Task::none(),
    }
}

pub fn handle_zoom(editor: &mut RawEditor, d: f32, mut p: Point) -> Task<Message> {
    if p.x < 0.0 {
        p = editor.last_cursor_position.unwrap_or(Point::ORIGIN);
    }
    run(editor.session.zoom_at(d, [p.x, p.y]))
}

pub fn handle_pan(editor: &mut RawEditor, d: cgmath::Vector2<f32>) -> Task<Message> {
    run(editor.session.pan([d.x, d.y]))
}

/// Start a render unless one is in flight (`Session::throttled_render`).
pub fn throttled_render(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.throttled_render())
}

pub fn handle_reset_view(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.reset_view())
}

pub fn handle_mouse_pressed(editor: &mut RawEditor) -> Task<Message> {
    let now = std::time::Instant::now();
    let double = editor.last_click_time.map(|t| now.duration_since(t).as_millis() < 300).unwrap_or(false);
    editor.last_click_time = Some(now);
    if double { return Task::done(Message::ResetView); }
    if !editor.session.is_cropping && !editor.session.is_wb_picking && !editor.session.is_leveling && !editor.session.mask_overlay_active() {
        editor.is_dragging = true;
        editor.drag_mode = DragMode::Pan;
    }
    Task::none()
}

pub fn handle_mouse_released(editor: &mut RawEditor) -> Task<Message> {
    if editor.is_dragging
        && matches!(
            editor.drag_mode,
            DragMode::CropHandle(_) | DragMode::MaskHandle(_)
        )
    {
        editor.session.end_handle_drag();
    }
    editor.is_dragging = false;
    editor.drag_mode = DragMode::None;
    editor.last_cursor_position = None;
    Task::none()
}

pub fn handle_mouse_moved(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    // Phase 116: Remove broken viewport_size auto-inflation loop.
    // viewport_size is now managed via Message::ViewportResized.
    
    if editor.session.is_cropping {
        handle_crop_interaction(editor, pos)
    } else if editor.session.mask_overlay_active()
        || matches!(editor.drag_mode, DragMode::MaskHandle(_))
    {
        handle_mask_interaction(editor, pos)
    } else {
        handle_pan_interaction(editor, pos)
    }
}

pub fn handle_working_preview_ready(
    editor: &mut RawEditor,
    id: i64,
    bytes: Option<std::sync::Arc<[u8]>>,
    dims: (u32, u32),
    path: String,
) -> Task<Message> {
    editor.session.working_preview_ready(id, bytes, dims, path);
    Task::none()
}

pub fn handle_preview_cached(
    editor: &mut RawEditor,
    id: i64,
    result: Result<(u32, u32, std::sync::Arc<[u8]>), String>,
) -> Task<Message> {
    editor.session.preview_cached(id, result);
    Task::none()
}

// Helpers

fn handle_pan_interaction(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    if editor.is_dragging && editor.drag_mode == DragMode::Pan {
        if let Some(last) = editor.last_cursor_position {
            let delta = pos - last;
            editor.last_cursor_position = Some(pos);
            let [dx, dy] = editor.session.pan_fraction([delta.x, delta.y]);
            return Task::done(Message::Pan(cgmath::Vector2::new(dx, dy)));
        }
    }
    editor.last_cursor_position = Some(pos);
    Task::none()
}

fn handle_crop_interaction(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    if editor.is_dragging {
        if let DragMode::CropHandle(h) = editor.drag_mode {
            if let Some(last) = editor.last_cursor_position {
                editor.session.drag_crop(h, [last.x, last.y], [pos.x, pos.y]);
                editor.last_cursor_position = Some(pos);
            }
        }
    } else {
        editor.last_cursor_position = Some(pos);
    }
    Task::none()
}

fn handle_mask_interaction(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    if editor.is_dragging {
        if let DragMode::MaskHandle(h) = editor.drag_mode {
            if let Some(last) = editor.last_cursor_position {
                editor.last_cursor_position = Some(pos);
                return run(editor.session.drag_mask(h, [last.x, last.y], [pos.x, pos.y]));
            }
        }
    } else {
        editor.last_cursor_position = Some(pos);
    }
    Task::none()
}

/// Decode the selected image again (`Session::reload_selected_image`).
pub fn reload_selected_image(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.reload_selected_image())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::message::AppTab;
    use crate::database::models::Image as ImageData;

    fn img(id: i64, path: &str) -> ImageData {
        ImageData {
            id,
            filename: format!("{id}.NEF"),
            path: path.to_string(),
            cache_path_thumb: None,
            cache_path_instant: None,
            cache_path_working: None,
            cache_path_edited: None,
            file_status: "exists".to_string(),
            rating: 0,
            flag: 0,
        }
    }

    fn editor_with(ids: &[i64]) -> RawEditor {
        let mut e = RawEditor::new().0;
        e.session.current_tab = AppTab::Library;
        e.session.images = ids.iter().map(|i| img(*i, "/p/x.NEF")).collect();
        e
    }

    /// A move the app makes (after a delete, auto-advance) must not be read as
    /// a Shift/Ctrl click because a key from the triggering shortcut is
    /// still held; a real click still is.
    #[test]
    fn app_driven_selection_ignores_held_modifiers() {
        let mut e = editor_with(&[1, 2, 3]);
        let _ = handle_image_selected(&mut e, 1);
        e.last_modifiers = iced::keyboard::Modifiers::SHIFT;
        let _ = crate::app::update::update(&mut e, Message::ImageMoved(3));
        assert_eq!(e.session.multi_selection, [3].into_iter().collect(), "a range select");
        assert_eq!(e.session.selected_image_id, Some(3));

        e.last_modifiers = iced::keyboard::Modifiers::CTRL;
        let _ = crate::app::update::update(&mut e, Message::ImageMoved(2));
        assert_eq!(e.session.multi_selection, [2].into_iter().collect(), "toggled out");

        // A real Shift-click still extends.
        e.last_modifiers = iced::keyboard::Modifiers::SHIFT;
        let _ = crate::app::update::update(&mut e, Message::ImageSelected(3));
        assert_eq!(e.session.multi_selection.len(), 2);
    }

    #[test]
    fn range_is_inclusive_of_both_endpoints() {
        let mut e = editor_with(&[1, 2, 3, 4, 5]);
        e.selection_anchor = Some(2);
        assert_eq!(select_range(&e, 4), Some(vec![2, 3, 4]));
    }

    /// Dragging a range upward must select the same set as dragging downward.
    #[test]
    fn range_is_direction_independent() {
        let mut e = editor_with(&[1, 2, 3, 4, 5]);
        e.selection_anchor = Some(4);
        assert_eq!(select_range(&e, 2), Some(vec![2, 3, 4]));
    }

    #[test]
    fn range_of_one_is_just_that_image() {
        let mut e = editor_with(&[1, 2, 3]);
        e.selection_anchor = Some(2);
        assert_eq!(select_range(&e, 2), Some(vec![2]));
    }

    #[test]
    fn range_needs_an_anchor() {
        let e = editor_with(&[1, 2, 3]);
        assert_eq!(select_range(&e, 2), None);
    }

    /// A range must never reach images the active filter is hiding — the ids
    /// come from `displayed_ids`, so an endpoint outside it has no position.
    #[test]
    fn range_ignores_endpoints_the_filter_hides() {
        let mut e = editor_with(&[1, 2, 3]);
        e.session.images[1].rating = 0;
        e.session.min_filter_rating = 5; // hides everything
        e.selection_anchor = Some(1);
        assert_eq!(select_range(&e, 3), None);
    }
}
