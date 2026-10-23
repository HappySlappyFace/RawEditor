// Local adjustment masks: create/select/delete masks and edit their
// per-mask adjustments. Mask geometry lives in full-image display UV (the
// same space as the crop rectangle); the color shader evaluates the masks
// analytically in Step 10.5.

use crate::app::handlers::develop::update_pipeline;
use crate::app::message::{MaskAdjustment, Message, StrokeEvent};
use crate::app::state::{DragMode, MaskTool, RawEditor};
use crate::core::types::{MaskParams, MASK_BRUSH, MASK_RANGE, MAX_MASKS};
use crate::ui::preview_renderer::MaskHandle;
use iced::Task;

/// Toggle mask placement mode (0 = linear, 1 = radial), or create a brush
/// (2) or range (3) mask outright — those have no geometry to drag out.
/// Mutually exclusive with crop mode and the WB picker.
pub fn handle_toggle_mask_placement(editor: &mut RawEditor, mask_type: u32) -> Task<Message> {
    if mask_type == MASK_BRUSH || mask_type == MASK_RANGE {
        return add_mask_now(editor, mask_type);
    }
    let target = if mask_type == 0 {
        MaskTool::PlacingLinear
    } else {
        MaskTool::PlacingRadial
    };
    if editor.mask_tool == target {
        editor.mask_tool = MaskTool::Inactive;
        editor.status.clear();
        return Task::none();
    }

    leave_other_tools(editor);
    editor.brush_armed = false;
    editor.mask_tool = target;
    editor.status = if mask_type == 0 {
        "Linear mask: drag across the image to place the gradient".to_string()
    } else {
        "Radial mask: drag on the image to place the ellipse".to_string()
    };
    update_pipeline(editor)
}

fn leave_other_tools(editor: &mut RawEditor) {
    editor.is_wb_picking = false;
    editor.is_leveling = false;
    editor.color_picking_mask = None;
    if editor.is_cropping {
        // update_pipeline clears the shader's crop flag to match.
        editor.is_cropping = false;
        editor.drag_mode = DragMode::None;
    }
}

/// Create a brush or range mask, select it, and arm the brush for a brush mask.
fn add_mask_now(editor: &mut RawEditor, mask_type: u32) -> Task<Message> {
    let count = editor.current_edit_params.mask_count as usize;
    if count >= MAX_MASKS {
        editor.status = format!("Mask limit reached ({MAX_MASKS})");
        return Task::none();
    }
    leave_other_tools(editor);
    editor.mask_tool = MaskTool::Inactive;
    editor.current_edit_params.masks[count] = if mask_type == MASK_BRUSH {
        MaskParams::new_brush()
    } else {
        MaskParams::new_range()
    };
    editor.current_edit_params.mask_count = (count + 1) as u32;
    editor.selected_mask = Some(count);
    editor.brush_armed = mask_type == MASK_BRUSH;
    editor.status = if mask_type == MASK_BRUSH {
        "Brush: paint on the image (hold Alt to erase)".to_string()
    } else {
        "Range mask: narrow it with the luminance and colour ranges".to_string()
    };
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

/// First press of the placement drag: create the mask at the pressed point
/// and hand the rest of the gesture to the drag machinery, so press-drag
/// sizes the new mask in one motion. `(u, v)` is full-image display UV.
pub fn handle_mask_placement_started(editor: &mut RawEditor, u: f32, v: f32) -> Task<Message> {
    let count = editor.current_edit_params.mask_count as usize;
    if count >= MAX_MASKS {
        editor.mask_tool = MaskTool::Inactive;
        editor.status = format!("Mask limit reached ({MAX_MASKS})");
        return Task::none();
    }

    let (mask, drag_handle) = match editor.mask_tool {
        MaskTool::PlacingLinear => (
            MaskParams::new_linear(u, v, u, v),
            MaskHandle::LinearEnd,
        ),
        MaskTool::PlacingRadial => (
            MaskParams::new_radial(u, v, 0.001, 0.001),
            MaskHandle::RadiusUniform,
        ),
        MaskTool::Inactive => return Task::none(),
    };

    editor.current_edit_params.masks[count] = mask;
    editor.current_edit_params.mask_count = (count + 1) as u32;
    editor.selected_mask = Some(count);
    editor.mask_tool = MaskTool::Inactive;
    editor.status.clear();

    // The overlay press that triggered this message becomes a handle drag.
    editor.drag_mode = DragMode::MaskHandle(drag_handle);
    editor.is_dragging = true;

    update_pipeline(editor)
}

pub fn handle_select_mask(editor: &mut RawEditor, index: Option<usize>) -> Task<Message> {
    let count = editor.current_edit_params.mask_count as usize;
    editor.selected_mask = index.filter(|&i| i < count);
    // Selecting from the list shouldn't leave a stale placement mode active.
    editor.mask_tool = MaskTool::Inactive;
    editor.color_picking_mask = None;
    if editor.selected_mask.is_some() {
        // Its handles take the clicks now.
        editor.is_leveling = false;
    }
    // A brush mask is only ever edited by painting; other masks paint only
    // when explicitly armed.
    editor.brush_armed = editor
        .selected_mask
        .is_some_and(|i| editor.current_edit_params.masks[i].mask_type == MASK_BRUSH);
    editor.canvas_cache.clear();
    // The overlay follows the selection.
    update_pipeline(editor)
}

pub fn handle_delete_mask(editor: &mut RawEditor, index: usize) -> Task<Message> {
    let count = editor.current_edit_params.mask_count as usize;
    if index >= count {
        return Task::none();
    }

    // Shift the remaining masks down over the deleted slot; strokes follow
    // their masks.
    editor.current_edit_params.masks.copy_within(index + 1..count, index);
    editor.current_edit_params.masks[count - 1] = MaskParams::default();
    editor.current_edit_params.mask_count = (count - 1) as u32;
    editor.current_edit_params.brush.remove_mask(index);
    editor.color_picking_mask = None;
    editor.brush_armed = false;

    editor.selected_mask = match editor.selected_mask {
        Some(s) if s == index => None,
        Some(s) if s > index => Some(s - 1),
        other => other,
    };

    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

pub fn handle_toggle_mask_enabled(
    editor: &mut RawEditor,
    index: usize,
    enabled: bool,
) -> Task<Message> {
    if index >= editor.current_edit_params.mask_count as usize {
        return Task::none();
    }
    editor.current_edit_params.masks[index].enabled = u32::from(enabled);
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

pub fn handle_toggle_mask_invert(editor: &mut RawEditor, inverted: bool) -> Task<Message> {
    let Some(index) = editor.selected_mask else {
        return Task::none();
    };
    if index >= editor.current_edit_params.mask_count as usize {
        return Task::none();
    }
    editor.current_edit_params.masks[index].invert = u32::from(inverted);
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

/// Slider change for the selected mask. Undo commit comes from the slider's
/// on_release(CommitEdit), same as the global sliders.
pub fn handle_mask_adjustment_changed(
    editor: &mut RawEditor,
    which: MaskAdjustment,
    value: f32,
) -> Task<Message> {
    let Some(index) = editor.selected_mask else {
        return Task::none();
    };
    if index >= editor.current_edit_params.mask_count as usize {
        return Task::none();
    }
    let mask = &mut editor.current_edit_params.masks[index];
    match which {
        MaskAdjustment::Exposure => mask.exposure = value,
        MaskAdjustment::Contrast => mask.contrast = value,
        MaskAdjustment::Saturation => mask.saturation = value,
        MaskAdjustment::Warmth => mask.warmth = value,
        MaskAdjustment::Highlights => mask.highlights = value,
        MaskAdjustment::Shadows => mask.shadows = value,
        MaskAdjustment::Feather => mask.feather = value.clamp(0.0, 1.0),
        MaskAdjustment::Tint => mask.tint = value.clamp(-1.0, 1.0),
        MaskAdjustment::Rotation => mask.rotation = value.clamp(-180.0, 180.0),
        MaskAdjustment::Hue => mask.hue = value.clamp(-1.0, 1.0),
        MaskAdjustment::LumaLo => mask.luma_lo = value.clamp(0.0, mask.luma_hi),
        MaskAdjustment::LumaHi => mask.luma_hi = value.clamp(mask.luma_lo, 1.0),
        MaskAdjustment::LumaSmooth => mask.luma_smooth = value.clamp(0.0, 0.5),
        MaskAdjustment::ColorTolerance => mask.color_tolerance = value.clamp(0.0, 1.0),
    }
    update_pipeline(editor)
}

/// Arm painting into the selected (non-brush) mask, or disarm it.
pub fn handle_toggle_brush_armed(editor: &mut RawEditor) -> Task<Message> {
    if editor.selected_mask.is_none() {
        return Task::none();
    }
    editor.brush_armed = !editor.brush_armed;
    editor.color_picking_mask = None;
    editor.canvas_cache.clear();
    Task::none()
}

/// Full-image display aspect (w / h): brush radii are isotropic in it.
fn display_aspect(editor: &RawEditor) -> f32 {
    editor
        .image_resources
        .as_ref()
        .map(|r| {
            let (w, h) = r.oriented_dims(editor.current_edit_params.quarter_turns);
            w as f32 / h.max(1) as f32
        })
        .unwrap_or(1.5)
}

pub fn handle_brush_stroke(editor: &mut RawEditor, ev: StrokeEvent) -> Task<Message> {
    let Some(index) = editor.selected_mask else {
        return Task::none();
    };
    if index >= editor.current_edit_params.mask_count as usize || !editor.brush_armed {
        return Task::none();
    }
    let aspect = display_aspect(editor);
    match ev {
        StrokeEvent::Begin(u, v) => {
            let b = editor.brush_settings;
            // Alt flips paint/erase for the duration of the stroke.
            let erase = b.erase != editor.last_modifiers.alt();
            let brush = &mut editor.current_edit_params.brush;
            if brush.begin(index as u32, erase, b.size, b.feather, b.flow).is_none() {
                editor.status = format!(
                    "Brush limit reached ({} strokes / {} points) — clear a mask's brush to free space",
                    crate::core::brush::MAX_STROKES,
                    crate::core::brush::MAX_POINTS,
                );
                return Task::none();
            }
            brush.extend([u, v], aspect);
            editor.is_painting = true;
            update_pipeline(editor)
        }
        StrokeEvent::Move(u, v) => {
            if !editor.is_painting {
                return Task::none();
            }
            let before = editor.current_edit_params.brush.point_count;
            if !editor.current_edit_params.brush.extend([u, v], aspect) {
                editor.status = "Brush point limit reached".to_string();
            }
            if editor.current_edit_params.brush.point_count == before {
                return Task::none();
            }
            update_pipeline(editor)
        }
        StrokeEvent::End => {
            if !editor.is_painting {
                return Task::none();
            }
            editor.is_painting = false;
            editor.current_edit_params.brush.finish(aspect);
            let task = update_pipeline(editor);
            editor.commit_current_state();
            task
        }
    }
}

pub fn handle_clear_mask_brush(editor: &mut RawEditor) -> Task<Message> {
    let Some(index) = editor.selected_mask else {
        return Task::none();
    };
    editor.current_edit_params.brush.clear_mask(index);
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

fn selected_mask_mut(editor: &mut RawEditor) -> Option<&mut MaskParams> {
    let i = editor.selected_mask?;
    (i < editor.current_edit_params.mask_count as usize)
        .then(|| &mut editor.current_edit_params.masks[i])
}

pub fn handle_toggle_luma_range(editor: &mut RawEditor, on: bool) -> Task<Message> {
    let Some(m) = selected_mask_mut(editor) else {
        return Task::none();
    };
    if on {
        m.enable_luma_range();
    } else {
        m.luma_range = 0;
    }
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

pub fn handle_toggle_color_range(editor: &mut RawEditor, on: bool) -> Task<Message> {
    let selected = editor.selected_mask;
    let Some(m) = selected_mask_mut(editor) else {
        return Task::none();
    };
    if !on {
        m.color_range = 0;
        editor.color_picking_mask = None;
        let task = update_pipeline(editor);
        editor.commit_current_state();
        return task;
    }
    // Nothing to compare against yet: arm the eyedropper; the range turns on
    // with the pick.
    editor.color_picking_mask = selected;
    editor.brush_armed = false;
    editor.status = "Color range: click the colour to select".to_string();
    Task::none()
}

/// Colour-range eyedropper click. Samples the raw mosaic (exactly what the
/// shader's camera-native chromaticity sees), not the rendered preview,
/// which already carries every adjustment — this mask's own included.
pub fn handle_mask_color_picked(editor: &mut RawEditor, u: f32, v: f32) -> Task<Message> {
    let Some(index) = editor.color_picking_mask.take() else {
        return Task::none();
    };
    let Some(image_id) = editor.selected_image_id else {
        return Task::none();
    };
    let (rgb, _clipped, raw) = match crate::app::handlers::develop::sample_raw_rgb(editor, image_id, u, v) {
        Ok(s) => s,
        Err(crate::app::handlers::develop::RawSampleError::Missing(msg)) => {
            editor.status = format!("Color range: {msg}");
            return Task::none();
        }
    };
    // Same units as the demosaic output: black-subtracted, unit range.
    let mean_black = raw.black_levels.iter().sum::<u32>() as f32 / 4.0;
    let range = (raw.white_level as f32 - mean_black).max(1.0);
    let target = crate::core::types::mask_chroma(rgb.map(|c| c / range));
    if index >= editor.current_edit_params.mask_count as usize {
        return Task::none();
    }
    let m = &mut editor.current_edit_params.masks[index];
    m.color_target = target;
    m.color_range = 1;
    if m.color_tolerance <= 0.0 {
        m.color_tolerance = 0.3;
    }
    editor.status.clear();
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
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

    /// A headless editor with an image "selected", so undo history exists.
    fn editor() -> RawEditor {
        let mut e = RawEditor::new().0;
        e.selected_image_id = Some(1);
        e.get_current_history();
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
        assert_eq!(e.current_edit_params.mask_count, 1);
        assert!(e.brush_armed, "a new brush mask is ready to paint");

        let _ = update(&mut e, Message::BrushStroke(StrokeEvent::Begin(0.2, 0.5)));
        assert!(e.is_painting);
        assert_eq!(e.current_edit_params.mask_overlay, 1, "overlay shows while painting");
        for i in 1..=20 {
            let _ = update(&mut e, Message::BrushStroke(StrokeEvent::Move(0.2 + i as f32 * 0.02, 0.5)));
        }
        let _ = update(&mut e, Message::BrushStroke(StrokeEvent::End));
        assert!(!e.is_painting);
        assert_eq!(e.current_edit_params.mask_overlay, 0, "overlay off again after the stroke");

        let b = e.current_edit_params.brush;
        assert_eq!(b.stroke_count, 1);
        assert!(!b.strokes[0].erase);
        assert_eq!(b.strokes[0].len, 2, "a straight stroke simplifies to its ends");

        let _ = update(&mut e, Message::Undo);
        assert_eq!(e.current_edit_params.brush.stroke_count, 0, "undo removes the stroke");
        assert_eq!(e.current_edit_params.mask_count, 1, "…but not the mask");
    }

    #[test]
    fn alt_inverts_paint_and_erase() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_BRUSH));
        e.last_modifiers = iced::keyboard::Modifiers::ALT;
        paint(&mut e, (0.2, 0.2), (0.4, 0.2));
        assert!(e.current_edit_params.brush.strokes[0].erase);
        e.last_modifiers = iced::keyboard::Modifiers::default();
        let _ = update(&mut e, Message::SetBrushErase(true));
        paint(&mut e, (0.2, 0.3), (0.4, 0.3));
        assert!(e.current_edit_params.brush.strokes[1].erase);
    }

    #[test]
    fn painting_needs_an_armed_brush() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_RANGE));
        assert!(!e.brush_armed, "range masks do not paint until asked");
        paint(&mut e, (0.2, 0.2), (0.4, 0.2));
        assert!(e.current_edit_params.brush.is_empty());
        let _ = update(&mut e, Message::ToggleBrushArmed);
        paint(&mut e, (0.2, 0.2), (0.4, 0.2));
        assert_eq!(e.current_edit_params.brush.stroke_count, 1);
    }

    #[test]
    fn deleting_a_mask_takes_its_strokes_and_renumbers_the_rest() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_BRUSH));
        paint(&mut e, (0.1, 0.1), (0.3, 0.1));
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_BRUSH));
        paint(&mut e, (0.1, 0.8), (0.3, 0.8));
        let _ = update(&mut e, Message::DeleteMask(0));
        let b = e.current_edit_params.brush;
        assert_eq!(e.current_edit_params.mask_count, 1);
        assert_eq!(b.stroke_count, 1);
        assert_eq!(b.strokes[0].mask, 0, "the surviving stroke follows its mask down");
        assert_eq!(b.points_of(&b.strokes[0])[0], [0.1, 0.8]);
    }

    #[test]
    fn the_overlay_is_tool_state_not_an_edit() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(MASK_RANGE));
        let _ = update(&mut e, Message::ToggleMaskOverlay(true));
        assert_eq!(e.current_edit_params.mask_overlay, 1);
        let json = e.current_edit_params.to_json().unwrap();
        assert!(!json.contains("mask_overlay"), "never persisted");
        let history = e.history_map.get(&1).unwrap().0.len();
        let _ = update(&mut e, Message::CommitEdit);
        assert_eq!(e.history_map.get(&1).unwrap().0.len(), history, "toggling it is not an undo step");
    }

    #[test]
    fn luma_range_starts_neutral() {
        let mut e = editor();
        let _ = update(&mut e, Message::ToggleMaskPlacement(0));
        let _ = update(&mut e, Message::MaskPlacementStarted(0.5, 0.5));
        let _ = update(&mut e, Message::ToggleMaskLumaRange(true));
        let m = e.current_edit_params.masks[0];
        assert_eq!((m.luma_range, m.luma_lo, m.luma_hi), (1, 0.0, 1.0));
    }
}
