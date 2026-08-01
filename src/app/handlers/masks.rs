// Local adjustment masks: create/select/delete masks and edit their
// per-mask adjustments. Mask geometry lives in full-image display UV (the
// same space as the crop rectangle); the color shader evaluates the masks
// analytically in Step 10.5.

use crate::app::handlers::develop::update_pipeline;
use crate::app::message::{MaskAdjustment, Message};
use crate::app::state::{DragMode, MaskTool, RawEditor};
use crate::core::types::{MaskParams, MAX_MASKS};
use crate::ui::preview_renderer::MaskHandle;
use iced::Task;

/// Toggle mask placement mode (0 = linear, 1 = radial). Mutually exclusive
/// with crop mode and the WB picker.
pub fn handle_toggle_mask_placement(editor: &mut RawEditor, mask_type: u32) -> Task<Message> {
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

    editor.is_wb_picking = false;
    if editor.is_cropping {
        // Leaving crop mode also clears its GPU flag (same as the WB picker).
        editor.is_cropping = false;
        editor.drag_mode = DragMode::None;
        editor.current_edit_params.is_cropping = 0;
    }
    editor.mask_tool = target;
    editor.status = if mask_type == 0 {
        "Linear mask: drag across the image to place the gradient".to_string()
    } else {
        "Radial mask: drag on the image to place the ellipse".to_string()
    };
    update_pipeline(editor)
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
    editor.canvas_cache.clear();
    Task::none()
}

pub fn handle_delete_mask(editor: &mut RawEditor, index: usize) -> Task<Message> {
    let count = editor.current_edit_params.mask_count as usize;
    if index >= count {
        return Task::none();
    }

    // Shift the remaining masks down over the deleted slot.
    editor.current_edit_params.masks.copy_within(index + 1..count, index);
    editor.current_edit_params.masks[count - 1] = MaskParams::default();
    editor.current_edit_params.mask_count = (count - 1) as u32;

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
    }
    update_pipeline(editor)
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
    // zoom-to-cursor, pan, get_image_screen_bounds, and the drag math itself —
    // re-derives the image rect from it via core::viewport::image_rect.
    // Assigning the image rect here corrupted all of those for the rest of the
    // session, and made the drag pivot around a phantom center on any
    // letterboxed image.
    Task::none()
}
