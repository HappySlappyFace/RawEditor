//! Local adjustment masks, the app's side: the pointer state of a placement
//! or handle drag, and the Alt key while painting. The masks themselves are
//! the session's (`session::masks`).

use crate::app::handlers::develop::switch_tool;
use crate::app::message::{MaskAdjustment, Message, StrokeEvent};
use crate::app::state::{DragMode, RawEditor};
use crate::app::update::run;
use crate::ui::preview_renderer::MaskHandle;
use iced::Task;

/// Toggle mask placement mode (0 = linear, 1 = radial), or create a brush
/// (2) or range (3) mask outright.
pub fn handle_toggle_mask_placement(editor: &mut RawEditor, mask_type: u32) -> Task<Message> {
    switch_tool(editor, |s| s.toggle_mask_placement(mask_type))
}

/// First press of the placement drag: the mask is created at the pressed
/// point and the rest of the gesture drags its handle.
pub fn handle_mask_placement_started(editor: &mut RawEditor, u: f32, v: f32) -> Task<Message> {
    let (effects, handle) = editor.session.start_mask_placement(u, v);
    if let Some(handle) = handle {
        // The overlay press that triggered this message becomes a handle drag.
        editor.drag_mode = DragMode::MaskHandle(handle);
        editor.is_dragging = true;
    }
    run(effects)
}

pub fn handle_select_mask(editor: &mut RawEditor, index: Option<usize>) -> Task<Message> {
    run(editor.session.select_mask(index))
}

pub fn handle_delete_mask(editor: &mut RawEditor, index: usize) -> Task<Message> {
    run(editor.session.delete_mask(index))
}

pub fn handle_toggle_mask_enabled(editor: &mut RawEditor, index: usize, enabled: bool) -> Task<Message> {
    run(editor.session.set_mask_enabled(index, enabled))
}

pub fn handle_toggle_mask_invert(editor: &mut RawEditor, inverted: bool) -> Task<Message> {
    run(editor.session.set_mask_inverted(inverted))
}

pub fn handle_mask_adjustment_changed(editor: &mut RawEditor, which: MaskAdjustment, value: f32) -> Task<Message> {
    run(editor.session.set_mask_adjustment(which, value))
}

pub fn handle_toggle_brush_armed(editor: &mut RawEditor) -> Task<Message> {
    editor.session.toggle_brush_armed();
    Task::none()
}

pub fn handle_brush_stroke(editor: &mut RawEditor, ev: StrokeEvent) -> Task<Message> {
    // Alt flips paint/erase for the duration of the stroke.
    let alt = editor.last_modifiers.alt();
    run(editor.session.brush_stroke(ev, alt))
}

pub fn handle_clear_mask_brush(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.clear_mask_brush())
}

pub fn handle_toggle_luma_range(editor: &mut RawEditor, on: bool) -> Task<Message> {
    run(editor.session.set_luma_range(on))
}

pub fn handle_toggle_color_range(editor: &mut RawEditor, on: bool) -> Task<Message> {
    run(editor.session.set_color_range(on))
}

pub fn handle_mask_color_picked(editor: &mut RawEditor, u: f32, v: f32) -> Task<Message> {
    run(editor.session.mask_color_picked(u, v))
}

pub fn handle_mask_handle_grabbed(
    editor: &mut RawEditor,
    handle: MaskHandle,
    _bounds: iced::Rectangle,
) -> Task<Message> {
    editor.drag_mode = DragMode::MaskHandle(handle);
    editor.is_dragging = true;
    // NOTE: `_bounds` is the image rect, NOT the canvas. `viewport_size` is
    // the canvas size (set by Message::ViewportResized) and every consumer —
    // zoom-to-cursor, pan, and the drag math itself —
    // re-derives the image rect from it via core::viewport::image_rect.
    // Assigning the image rect here corrupted all of those for the rest of the
    // session, and made the drag pivot around a phantom center on any
    // letterboxed image.
    Task::none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::update::update;
    use crate::core::types::{MASK_BRUSH, MASK_RANGE};

    /// A headless editor with an image "selected", so undo history exists.
    fn editor() -> RawEditor {
        let mut e = RawEditor::new().0;
        e.session.selected_image_id = Some(1);
        e.session.get_current_history();
        e
    }

    fn paint(e: &mut RawEditor, from: (f32, f32), to: (f32, f32)) {
        let _ = update(e, Message::BrushStroke(StrokeEvent::Begin(from.0, from.1)));
        for i in 1..=20 {
            let t = i as f32 / 20.0;
            let p = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            let _ = update(e, Message::BrushStroke(StrokeEvent::Move(p.0, p.1)));
        }
        let _ = update(e, Message::BrushStroke(StrokeEvent::End));
    }

    #[test]
    fn painting_a_brush_mask_records_one_undoable_stroke() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_BRUSH));
        assert_eq!(e.session.current_edit_params.mask_count, 1);
        assert!(e.session.brush_armed, "a new brush mask is ready to paint");

        let _ = update(&mut e, Message::BrushStroke(StrokeEvent::Begin(0.2, 0.5)));
        assert!(e.session.is_painting);
        assert_eq!(e.session.current_edit_params.mask_overlay, 1, "overlay shows while painting");
        for i in 1..=20 {
            let _ = update(&mut e, Message::BrushStroke(StrokeEvent::Move(0.2 + i as f32 * 0.02, 0.5)));
        }
        let _ = update(&mut e, Message::BrushStroke(StrokeEvent::End));
        assert!(!e.session.is_painting);
        assert_eq!(e.session.current_edit_params.mask_overlay, 0, "overlay off again after the stroke");

        let b = e.session.current_edit_params.brush;
        assert_eq!(b.stroke_count, 1);
        assert!(!b.strokes[0].erase);
        assert_eq!(b.strokes[0].len, 2, "a straight stroke simplifies to its ends");

        let _ = update(&mut e, Message::Undo);
        assert_eq!(e.session.current_edit_params.brush.stroke_count, 0, "undo removes the stroke");
        assert_eq!(e.session.current_edit_params.mask_count, 1, "…but not the mask");
    }

    /// With Straighten on, a stroke is stored where the shader reads the
    /// brush layer for the pixel under the cursor (`frame_to_layer_uv`), not
    /// at the raw canvas UV — the GPU half is `masks_follow_the_picture`.
    #[test]
    fn a_stroke_on_a_straightened_image_is_stored_in_the_layer_frame() {
        let mut e = editor();
        let raw = crate::raw::loader::RawDataResult {
            width: 6000,
            height: 4000,
            orientation: 1,
            ..crate::ml::segment::test_support::disc_raw(60, 40)
        };
        e.session.raw_cache.put(1, std::sync::Arc::new(raw));
        e.session.current_edit_params.rotation = 10.0;
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_BRUSH));
        let _ = update(&mut e, Message::BrushStroke(StrokeEvent::Begin(0.8, 0.3)));
        let _ = update(&mut e, Message::BrushStroke(StrokeEvent::End));
        let b = e.session.current_edit_params.brush;
        let got = b.points_of(&b.strokes[0])[0];
        let lens = crate::core::lens::LensContext::default();
        let want = crate::core::viewport::frame_to_layer_uv(&e.session.current_edit_params, &lens, 1, 6000, 4000, 0.8, 0.3);
        assert!((got[0] - want.0).abs() < 1e-6 && (got[1] - want.1).abs() < 1e-6, "{got:?} vs {want:?}");
        assert!((got[0] - 0.8).abs() + (got[1] - 0.3).abs() > 0.02, "Straighten must move it: {got:?}");
    }

    #[test]
    fn alt_inverts_paint_and_erase() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_BRUSH));
        e.last_modifiers = iced::keyboard::Modifiers::ALT;
        paint(&mut e, (0.2, 0.2), (0.4, 0.2));
        assert!(e.session.current_edit_params.brush.strokes[0].erase);
        e.last_modifiers = iced::keyboard::Modifiers::default();
        let _ = update(&mut e, Message::SetBrushErase(true));
        paint(&mut e, (0.2, 0.3), (0.4, 0.3));
        assert!(e.session.current_edit_params.brush.strokes[1].erase);
    }

    #[test]
    fn painting_needs_an_armed_brush() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_RANGE));
        assert!(!e.session.brush_armed, "range masks do not paint until asked");
        paint(&mut e, (0.2, 0.2), (0.4, 0.2));
        assert!(e.session.current_edit_params.brush.is_empty());
        let _ = update(&mut e, Message::ToggleBrushArmed);
        paint(&mut e, (0.2, 0.2), (0.4, 0.2));
        assert_eq!(e.session.current_edit_params.brush.stroke_count, 1);
    }

    #[test]
    fn deleting_a_mask_takes_its_strokes_and_renumbers_the_rest() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_BRUSH));
        paint(&mut e, (0.1, 0.1), (0.3, 0.1));
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_BRUSH));
        paint(&mut e, (0.1, 0.8), (0.3, 0.8));
        let _ = update(&mut e, Message::DeleteMask(0));
        let b = e.session.current_edit_params.brush;
        assert_eq!(e.session.current_edit_params.mask_count, 1);
        assert_eq!(b.stroke_count, 1);
        assert_eq!(b.strokes[0].mask, 0, "the surviving stroke follows its mask down");
        assert_eq!(b.points_of(&b.strokes[0])[0], [0.1, 0.8]);
    }

    #[test]
    fn the_overlay_is_tool_state_not_an_edit() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_RANGE));
        let _ = update(&mut e, Message::ToggleMaskOverlay(true));
        assert_eq!(e.session.current_edit_params.mask_overlay, 1);
        let json = e.session.current_edit_params.to_json().unwrap();
        assert!(!json.contains("mask_overlay"), "never persisted");
        let history = e.session.history_map.get(&1).unwrap().0.len();
        let _ = update(&mut e, Message::CommitEdit);
        assert_eq!(e.session.history_map.get(&1).unwrap().0.len(), history, "toggling it is not an undo step");
    }

    /// Press-drag places a mask and sizes it in one motion: the press
    /// creates it and the rest of the gesture is a drag of its handle,
    /// which the release ends.
    #[test]
    fn placing_a_mask_hands_the_gesture_to_its_handle() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(1));
        let _ = update(&mut e, Message::MaskPlacementStarted(0.5, 0.5));
        assert_eq!(e.session.current_edit_params.mask_count, 1);
        assert_eq!(e.session.selected_mask, Some(0));
        assert!(e.is_dragging);
        assert_eq!(e.drag_mode, DragMode::MaskHandle(MaskHandle::RadiusUniform));
        let _ = update(&mut e, Message::MouseReleased);
        assert!(!e.is_dragging);
        assert_eq!(e.drag_mode, DragMode::None);

        // With no placement armed, a press places nothing and starts no drag.
        let _ = update(&mut e, Message::MaskPlacementStarted(0.2, 0.2));
        assert_eq!(e.session.current_edit_params.mask_count, 1);
        assert!(!e.is_dragging);
    }

    #[test]
    fn luma_range_starts_neutral() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(0));
        let _ = update(&mut e, Message::MaskPlacementStarted(0.5, 0.5));
        let _ = update(&mut e, Message::ToggleMaskLumaRange(true));
        let m = e.session.current_edit_params.masks[0];
        assert_eq!((m.luma_range, m.luma_lo, m.luma_hi), (1, 0.0, 1.0));
    }
}
