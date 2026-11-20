//! Mask handlers that used to panic or strand state on odd input.

use crate::app::message::{MaskAdjustment, Message, StrokeEvent};
use crate::app::testkit::*;
use crate::core::types::MaskParams;

/// `f32::clamp` panics when min > max; a saved edit (or a drag from a bad
/// state) can hold `luma_hi < 0`.
#[test]
fn luma_sliders_do_not_panic_on_an_inverted_band() {
    let mut e = develop_with_images(1);
    let mut m = MaskParams::new_range();
    m.luma_lo = 0.9;
    m.luma_hi = -0.5;
    e.session.current_edit_params.masks[0] = m;
    e.session.current_edit_params.mask_count = 1;
    e.session.selected_mask = Some(0);
    send(&mut e, Message::MaskAdjustmentChanged(MaskAdjustment::LumaLo, 0.3));
    send(&mut e, Message::MaskAdjustmentChanged(MaskAdjustment::LumaHi, 0.2));
    let m = e.session.current_edit_params.masks[0];
    assert!((0.0..=1.0).contains(&m.luma_lo) && (0.0..=1.0).contains(&m.luma_hi));
}

/// Disarming the brush mid-stroke must not leave `is_painting` set: the
/// release still ends and commits the stroke.
#[test]
fn a_stroke_ends_even_if_the_brush_was_disarmed_mid_stroke() {
    let mut e = develop_with_images(1);
    e.session.current_edit_params.masks[0] = MaskParams::new_brush();
    e.session.current_edit_params.mask_count = 1;
    e.session.selected_mask = Some(0);
    e.brush_armed = true;
    send(&mut e, Message::BrushStroke(StrokeEvent::Begin(0.5, 0.5)));
    assert!(e.session.is_painting);
    e.brush_armed = false;
    send(&mut e, Message::BrushStroke(StrokeEvent::End));
    assert!(!e.session.is_painting, "the stroke was never finished");
    assert!(!saved_edits(&e, 1).brush.is_empty() || e.session.current_edit_params.brush.stroke_count == 1);
}
