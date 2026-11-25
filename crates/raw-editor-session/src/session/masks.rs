//! Local adjustment masks: create/select/delete masks, edit their
//! per-mask adjustments, paint brush strokes, pick a colour range. Mask
//! geometry lives in full-image display UV (the same space as the crop
//! rectangle); the colour shader evaluates the masks analytically in Step 10.5.

use super::{Changed, Effects, MaskHandle, RawSampleError, Session, StrokeEvent};
use crate::session::MaskTool;
use crate::core::types::{MaskParams, MASK_BRUSH, MASK_RANGE, MAX_MASKS};

/// Which per-mask adjustment a slider targets (see MaskAdjustmentChanged)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskAdjustment {
    Exposure,
    Contrast,
    Saturation,
    Warmth,
    Highlights,
    Shadows,
    Feather,
    Tint,
    Rotation,
    Hue,
    LumaLo,
    LumaHi,
    LumaSmooth,
    ColorTolerance,
}

impl Session {
    /// Toggle mask placement mode (0 = linear, 1 = radial), or create a brush
    /// (2) or range (3) mask outright — those have no geometry to drag out.
    /// Mutually exclusive with the other tools.
    pub fn toggle_mask_placement(&mut self, mask_type: u32) -> Effects {
        if mask_type == MASK_BRUSH || mask_type == MASK_RANGE {
            return self.add_mask_now(mask_type);
        }
        let target = if mask_type == 0 { MaskTool::PlacingLinear } else { MaskTool::PlacingRadial };
        if self.mask_tool == target {
            self.mask_tool = MaskTool::Inactive;
            self.status.clear();
            return Effects::none();
        }

        self.leave_other_tools();
        self.brush_armed = false;
        self.mask_tool = target;
        self.status = if mask_type == 0 {
            "Linear mask: drag across the image to place the gradient".to_string()
        } else {
            "Radial mask: drag on the image to place the ellipse".to_string()
        };
        self.update_pipeline()
    }

    fn leave_other_tools(&mut self) {
        self.is_wb_picking = false;
        self.is_leveling = false;
        self.color_picking_mask = None;
        // update_pipeline clears the shader's crop flag to match.
        self.is_cropping = false;
    }

    /// Create a brush or range mask, select it, and arm the brush for a brush mask.
    fn add_mask_now(&mut self, mask_type: u32) -> Effects {
        let count = self.current_edit_params.mask_count as usize;
        if count >= MAX_MASKS {
            self.status = format!("Mask limit reached ({MAX_MASKS})");
            return Effects::none();
        }
        self.leave_other_tools();
        self.mask_tool = MaskTool::Inactive;
        self.current_edit_params.masks[count] =
            if mask_type == MASK_BRUSH { MaskParams::new_brush() } else { MaskParams::new_range() };
        self.current_edit_params.mask_count = (count + 1) as u32;
        self.selected_mask = Some(count);
        self.brush_armed = mask_type == MASK_BRUSH;
        self.status = if mask_type == MASK_BRUSH {
            "Brush: paint on the image (hold Alt to erase)".to_string()
        } else {
            "Range mask: narrow it with the luminance and colour ranges".to_string()
        };
        let effects = self.update_pipeline();
        self.commit_current_state();
        effects
    }

    /// First press of the placement drag: create the mask at the pressed point
    /// and hand the rest of the gesture to the handle drag, so press-drag sizes
    /// the new mask in one motion. `(u, v)` is full-image display UV. Returns
    /// the handle the gesture continues with (none: no mask was placed).
    pub fn start_mask_placement(&mut self, u: f32, v: f32) -> (Effects, Option<MaskHandle>) {
        let count = self.current_edit_params.mask_count as usize;
        if count >= MAX_MASKS {
            self.mask_tool = MaskTool::Inactive;
            self.status = format!("Mask limit reached ({MAX_MASKS})");
            return (Effects::none(), None);
        }

        let (mask, drag_handle) = match self.mask_tool {
            MaskTool::PlacingLinear => (MaskParams::new_linear(u, v, u, v), MaskHandle::LinearEnd),
            MaskTool::PlacingRadial => (MaskParams::new_radial(u, v, 0.001, 0.001), MaskHandle::RadiusUniform),
            MaskTool::Inactive => return (Effects::none(), None),
        };

        self.current_edit_params.masks[count] = mask;
        self.current_edit_params.mask_count = (count + 1) as u32;
        self.selected_mask = Some(count);
        self.mask_tool = MaskTool::Inactive;
        self.status.clear();
        (self.update_pipeline(), Some(drag_handle))
    }

    pub fn select_mask(&mut self, index: Option<usize>) -> Effects {
        let count = self.current_edit_params.mask_count as usize;
        self.selected_mask = index.filter(|&i| i < count);
        // Selecting from the list shouldn't leave a stale placement mode active.
        self.mask_tool = MaskTool::Inactive;
        self.color_picking_mask = None;
        if self.selected_mask.is_some() {
            // Its handles take the clicks now.
            self.is_leveling = false;
        }
        // A brush mask is only ever edited by painting; other masks paint only
        // when explicitly armed.
        self.brush_armed = self
            .selected_mask
            .is_some_and(|i| self.current_edit_params.masks[i].mask_type == MASK_BRUSH);
        self.changed.insert(Changed::OVERLAY);
        // The overlay follows the selection.
        self.update_pipeline()
    }

    pub fn delete_mask(&mut self, index: usize) -> Effects {
        let count = self.current_edit_params.mask_count as usize;
        if index >= count {
            return Effects::none();
        }

        // Shift the remaining masks down over the deleted slot; strokes follow
        // their masks.
        self.current_edit_params.masks.copy_within(index + 1..count, index);
        self.current_edit_params.masks[count - 1] = MaskParams::default();
        self.current_edit_params.mask_count = (count - 1) as u32;
        self.current_edit_params.brush.remove_mask(index);
        self.color_picking_mask = None;
        self.brush_armed = false;

        // An open AI-select session names its mask by slot: deleting that mask
        // ends the session, deleting one below it moves the session down with it.
        // (Done/Cancel take the session before they call this, so they are
        // unaffected.)
        match self.ai_select.as_ref().map(|s| s.slot) {
            Some(slot) if slot == index => {
                self.ai_select = None;
                self.status = String::new();
            }
            Some(slot) if slot > index => {
                if let Some(s) = self.ai_select.as_mut() {
                    s.slot = slot - 1;
                }
            }
            _ => {}
        }

        self.selected_mask = match self.selected_mask {
            Some(s) if s == index => None,
            Some(s) if s > index => Some(s - 1),
            other => other,
        };

        let effects = self.update_pipeline();
        self.commit_current_state();
        effects
    }

    pub fn set_mask_enabled(&mut self, index: usize, enabled: bool) -> Effects {
        if index >= self.current_edit_params.mask_count as usize {
            return Effects::none();
        }
        self.edit_and_commit(|p| p.masks[index].enabled = u32::from(enabled))
    }

    pub fn set_mask_inverted(&mut self, inverted: bool) -> Effects {
        let Some(index) = self.live_selected_mask() else {
            return Effects::none();
        };
        self.edit_and_commit(|p| p.masks[index].invert = u32::from(inverted))
    }

    /// The selected mask, if it is one of the live masks.
    fn live_selected_mask(&self) -> Option<usize> {
        self.selected_mask.filter(|&i| i < self.current_edit_params.mask_count as usize)
    }

    /// Slider change for the selected mask. The undo step comes from the
    /// slider's release, as for the global sliders.
    pub fn set_mask_adjustment(&mut self, which: MaskAdjustment, value: f32) -> Effects {
        let Some(index) = self.live_selected_mask() else {
            return Effects::none();
        };
        self.edit(|p| {
            let mask = &mut p.masks[index];
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
                MaskAdjustment::LumaLo => mask.luma_lo = value.min(mask.luma_hi).max(0.0),
                MaskAdjustment::LumaHi => mask.luma_hi = value.max(mask.luma_lo).min(1.0),
                MaskAdjustment::LumaSmooth => mask.luma_smooth = value.clamp(0.0, 0.5),
                MaskAdjustment::ColorTolerance => mask.color_tolerance = value.clamp(0.0, 1.0),
            }
        })
    }

    /// Arm painting into the selected (non-brush) mask, or disarm it.
    pub fn toggle_brush_armed(&mut self) {
        if self.selected_mask.is_none() {
            return;
        }
        self.brush_armed = !self.brush_armed;
        self.color_picking_mask = None;
        self.changed.insert(Changed::OVERLAY);
    }

    /// Full-image display aspect (w / h): brush radii are isotropic in it.
    fn display_aspect(&self) -> f32 {
        self.image_resources
            .as_ref()
            .map(|r| {
                let (w, h) = r.oriented_dims(self.current_edit_params.quarter_turns);
                w as f32 / h.max(1) as f32
            })
            .unwrap_or(1.5)
    }

    /// A canvas point (full-image display UV) in the frame the brush layer is
    /// stored in: the layer is read at the source position the picture shows, so
    /// it follows Straighten and lens correction (`core::viewport::frame_to_layer_uv`).
    fn layer_uv(&self, u: f32, v: f32) -> [f32; 2] {
        let geo = self.selected_image_id.and_then(|id| self.source_geometry(id));
        let Some(geo) = geo else { return [u, v] };
        let orientation = crate::color::compose_orientation(geo.exif_orientation, self.current_edit_params.quarter_turns);
        let (lu, lv) = crate::core::viewport::frame_to_layer_uv(
            &self.current_edit_params,
            &geo.lens,
            orientation,
            geo.width,
            geo.height,
            u,
            v,
        );
        [lu, lv]
    }

    /// One phase of a brush stroke. `flip_erase` (Alt held) flips paint and
    /// erase for the stroke it begins.
    pub fn brush_stroke(&mut self, ev: StrokeEvent, flip_erase: bool) -> Effects {
        let live = self.live_selected_mask().filter(|_| self.brush_armed);
        let Some(index) = live else {
            // Disarmed (or deselected) mid-stroke: the release must still end it,
            // or `is_painting` stays set and the stroke is never committed.
            if matches!(ev, StrokeEvent::End) {
                return self.end_stroke();
            }
            return Effects::none();
        };
        let aspect = self.display_aspect();
        match ev {
            StrokeEvent::Begin(u, v) => {
                let [u, v] = self.layer_uv(u, v);
                let b = self.brush_settings;
                let erase = b.erase != flip_erase;
                let brush = &mut self.current_edit_params.brush;
                if brush.begin(index as u32, erase, b.size, b.feather, b.flow).is_none() {
                    self.status = format!(
                        "Brush limit reached ({} strokes / {} points) — clear a mask's brush to free space",
                        crate::core::brush::MAX_STROKES,
                        crate::core::brush::MAX_POINTS,
                    );
                    return Effects::none();
                }
                brush.extend([u, v], aspect);
                self.is_painting = true;
                self.update_pipeline()
            }
            StrokeEvent::Move(u, v) => {
                let [u, v] = self.layer_uv(u, v);
                if !self.is_painting {
                    return Effects::none();
                }
                let before = self.current_edit_params.brush.point_count;
                if !self.current_edit_params.brush.extend([u, v], aspect) {
                    self.status = "Brush point limit reached".to_string();
                }
                if self.current_edit_params.brush.point_count == before {
                    return Effects::none();
                }
                self.update_pipeline()
            }
            StrokeEvent::End => self.end_stroke(),
        }
    }

    /// Finish the stroke in progress (if any) and commit it.
    fn end_stroke(&mut self) -> Effects {
        if !self.is_painting {
            return Effects::none();
        }
        let aspect = self.display_aspect();
        self.is_painting = false;
        self.current_edit_params.brush.finish(aspect);
        let effects = self.update_pipeline();
        self.commit_current_state();
        effects
    }

    pub fn clear_mask_brush(&mut self) -> Effects {
        let Some(index) = self.selected_mask else {
            return Effects::none();
        };
        self.edit_and_commit(|p| p.brush.clear_mask(index))
    }

    pub fn set_luma_range(&mut self, on: bool) -> Effects {
        let Some(index) = self.live_selected_mask() else {
            return Effects::none();
        };
        self.edit_and_commit(|p| {
            let m = &mut p.masks[index];
            if on {
                m.enable_luma_range();
            } else {
                m.luma_range = 0;
            }
        })
    }

    pub fn set_color_range(&mut self, on: bool) -> Effects {
        let Some(index) = self.live_selected_mask() else {
            return Effects::none();
        };
        if !on {
            self.color_picking_mask = None;
            return self.edit_and_commit(|p| p.masks[index].color_range = 0);
        }
        // Nothing to compare against yet: arm the eyedropper; the range turns on
        // with the pick.
        self.color_picking_mask = self.selected_mask;
        self.brush_armed = false;
        self.status = "Color range: click the colour to select".to_string();
        Effects::none()
    }

    /// Colour-range eyedropper click. Samples the raw mosaic (exactly what the
    /// shader's camera-native chromaticity sees), not the rendered preview,
    /// which already carries every adjustment — this mask's own included.
    pub fn mask_color_picked(&mut self, u: f32, v: f32) -> Effects {
        let Some(index) = self.color_picking_mask.take() else {
            return Effects::none();
        };
        let Some(image_id) = self.selected_image_id else {
            return Effects::none();
        };
        let (rgb, _clipped, raw) = match self.sample_raw_rgb(image_id, u, v) {
            Ok(s) => s,
            Err(RawSampleError::Missing(msg)) => {
                self.status = format!("Color range: {msg}");
                return Effects::none();
            }
        };
        // Same units as the demosaic output: black-subtracted, unit range.
        let mean_black = super::develop::mean_black(&raw, self.current_edit_params.black_offsets);
        let range = (raw.white_level as f32 - mean_black).max(1.0);
        let target = crate::core::types::mask_chroma(rgb.map(|c| c / range));
        if index >= self.current_edit_params.mask_count as usize {
            return Effects::none();
        }
        self.status.clear();
        self.edit_and_commit(|p| {
            let m = &mut p.masks[index];
            m.color_target = target;
            m.color_range = 1;
            if m.color_tolerance <= 0.0 {
                m.color_tolerance = 0.3;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::testkit::{deliver, open_in_develop, session_with_images};
    use crate::session::SessionEvent;

    /// Image 1 open with its RAW decoded (big enough for the eyedropper's
    /// 16×16 sample), one radial mask selected.
    fn session() -> Session {
        let mut s = session_with_images(1);
        let _ = open_in_develop(&mut s, 1);
        let disc = crate::ml::segment::test_support::disc_raw(60, 40);
        let _ = deliver(&mut s, SessionEvent::RawDataLoaded(1, Ok(disc)));
        s.current_edit_params.masks[0] = MaskParams { mask_type: 1, enabled: 1, ..MaskParams::default() };
        s.current_edit_params.mask_count = 1;
        s.commit_current_state();
        let _ = s.select_mask(Some(0));
        s
    }

    /// Turning colour range on arms the eyedropper; the pick, sampled from
    /// the RAW, turns it on with a usable tolerance, as one undo step.
    #[test]
    fn colour_range_turns_on_with_the_pick() {
        let mut s = session();
        assert!(s.set_color_range(true).is_empty());
        assert_eq!(s.color_picking_mask, Some(0));
        assert_eq!(s.current_edit_params.masks[0].color_range, 0, "on before anything was picked");

        let _ = s.mask_color_picked(0.5, 0.5);
        let m = s.current_edit_params.masks[0];
        assert_eq!((m.color_range, m.color_tolerance), (1, 0.3), "status: {}", s.status);
        assert!(m.color_target.iter().all(|c| c.is_finite()), "{:?}", m.color_target);
        assert_eq!(s.color_picking_mask, None);
        assert!(s.mask_color_picked(0.5, 0.5).is_empty(), "a second click picked again");

        let _ = s.undo();
        assert_eq!(s.current_edit_params.masks[0].color_range, 0, "the pick was not an undo step");
        let _ = s.redo();
        let _ = s.set_color_range(false);
        assert_eq!(s.current_edit_params.masks[0].color_range, 0);
    }

    #[test]
    fn a_pick_without_the_raw_says_why_and_changes_nothing() {
        let mut s = session_with_images(1);
        let _ = open_in_develop(&mut s, 1);
        s.current_edit_params.masks[0] = MaskParams { mask_type: 1, ..MaskParams::default() };
        s.current_edit_params.mask_count = 1;
        let _ = s.select_mask(Some(0));
        let _ = s.set_color_range(true);
        let _ = s.mask_color_picked(0.5, 0.5);
        assert!(s.status.starts_with("Color range: "), "{}", s.status);
        assert_eq!(s.current_edit_params.masks[0].color_range, 0);
        assert_eq!(s.color_picking_mask, None, "the eyedropper stayed armed");
    }

    /// The luminance range's ends push against each other instead of crossing.
    #[test]
    fn the_luminance_range_never_inverts() {
        let mut s = session();
        let _ = s.set_mask_adjustment(MaskAdjustment::LumaHi, 0.4);
        let _ = s.set_mask_adjustment(MaskAdjustment::LumaLo, 0.9);
        let m = s.current_edit_params.masks[0];
        assert_eq!((m.luma_lo, m.luma_hi), (0.4, 0.4));
        let _ = s.set_mask_adjustment(MaskAdjustment::LumaHi, -1.0);
        assert_eq!(s.current_edit_params.masks[0].luma_hi, 0.4);
        let _ = s.set_mask_adjustment(MaskAdjustment::Rotation, 270.0);
        assert_eq!(s.current_edit_params.masks[0].rotation, 180.0);
    }

    #[test]
    fn selecting_a_brush_mask_arms_painting_and_others_do_not() {
        let mut s = session();
        s.current_edit_params.masks[1] = MaskParams { mask_type: MASK_BRUSH, ..MaskParams::default() };
        s.current_edit_params.mask_count = 2;
        s.is_leveling = true;
        let _ = s.select_mask(Some(1));
        assert!(s.brush_armed);
        assert!(!s.is_leveling, "the level tool kept the clicks");
        let _ = s.select_mask(Some(0));
        assert!(!s.brush_armed);
        let _ = s.select_mask(Some(5));
        assert_eq!(s.selected_mask, None, "selected a mask that doesn't exist");
    }

    /// Edits to a slot past the live masks are ignored, not written into a
    /// cleared slot that a new mask would inherit.
    #[test]
    fn a_slot_past_the_masks_is_left_alone() {
        let mut s = session();
        let before = s.current_edit_params;
        assert!(s.set_mask_enabled(3, false).is_empty());
        s.selected_mask = Some(3);
        assert!(s.set_mask_adjustment(MaskAdjustment::Exposure, 1.0).is_empty());
        assert!(s.set_color_range(true).is_empty());
        assert_eq!(s.current_edit_params, before);
    }
}
