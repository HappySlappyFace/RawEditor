//! Develop: edits and their history, the render loop (one render in flight,
//! one pending; uniform pushes parked while a render runs), the before/after
//! peek, the tools (crop, level, WB picker), copy/paste, and the view (zoom,
//! pan) with the drags on it.

use std::sync::Arc;

use super::{Changed, Effects, Session, SessionEvent};
use crate::session::AppTab;
use crate::session::{EditorReadiness, MaskTool};
use crate::core::tonemap::ToneSettings;
use crate::core::types::{CopyCategories, EditParams};

/// Phase 67: Crop handles
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CropHandle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// Phase 70: Dragging the crop area itself
    Body,
}

/// Draggable handles of the selected local adjustment mask
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaskHandle {
    /// Linear: ramp start pin (weight 1)
    LinearStart,
    /// Linear: ramp end pin (weight 0)
    LinearEnd,
    /// Radial: center pin (also moves the whole linear mask if hit on the line)
    Center,
    /// Radial: horizontal radius handle
    RadiusX,
    /// Radial: vertical radius handle
    RadiusY,
    /// Radial only, used transiently during initial placement: grows both
    /// radii together (aspect-corrected) so a single drag makes a circle
    /// instead of a flat horizontal sliver. Never hit-tested afterward —
    /// once placed, RadiusX/RadiusY resize a single axis as normal.
    RadiusUniform,
    /// Radial only: rotates the ellipse. Dragged via angle (atan2 around the
    /// screen-space center), not a UV delta like the other handles.
    Rotation,
}

/// One phase of a brush stroke on the preview, in full-image display UV.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StrokeEvent {
    Begin(f32, f32),
    Move(f32, f32),
    End,
}

/// Which GT (Uchimura) shape parameter a slider targets. Mirrors the
/// `MaskAdjustment` pattern: one message for a family of sliders that all
/// land in the same handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GtParam {
    Contrast,
    LinearStart,
    LinearLength,
    BlackTightness,
}

/// Which group of adjustments a Copy Settings checkbox targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyCategory {
    Tone,
    Color,
    WhiteBalance,
    Presence,
    Noise,
    Detail,
    Geometry,
    Profile,
    Masks,
    Lens,
    BlackLevels,
}

/// Everything a completed develop render hands back to the UI thread.
#[derive(Debug, Clone)]
pub struct RenderOutput {
    /// Image the render was made from. A render still in flight when the
    /// user navigates must not land on the next image's viewport.
    pub image_id: i64,
    /// The on-screen window's pixels (RGBA), `dims` in size.
    pub display: std::sync::Arc<[u8]>,
    pub dims: (u32, u32),
    /// The window `display` covers. Travels with the pixels so the viewport
    /// never draws a buffer against a window it wasn't rendered for.
    pub view_rect: [f32; 4],
    /// Small whole-image render (RGBA), `overview_dims` in size: histogram
    /// source and edited-proxy candidate.
    pub overview: std::sync::Arc<[u8]>,
    pub overview_dims: (u32, u32),
    pub histogram: crate::core::histogram::HistogramData,
    /// The params rendered, when they are the user's real edit — `None` for
    /// the before/after peek and the crop tool's full-sensor visualisation,
    /// neither of which may stand in for the edited image.
    pub edit: Option<crate::core::types::EditParams>,
    pub upload_ms: f32,
    pub render_ms: f32,
    pub update_ms: f32,
    /// The profiler's breakdown (`profiling` builds; empty otherwise).
    pub detail: crate::core::profiler::Detail,
}

/// The groups of edits the Develop sidebar's sections own (its reset buttons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditSection {
    Light,
    ToneCurve,
    ToneMapping,
    Colour,
    Presence,
    Detail,
    Lens,
    /// Crop, Straighten and quarter turns.
    Geometry,
    Masks,
}

impl EditSection {
    /// `p`'s edits in this section, set back to their defaults. Geometry is
    /// not here: undoing quarter turns needs the image's shape
    /// (`Session::reset_section`). Masks have no reset: that would delete them.
    pub fn reset(self, p: &mut EditParams) {
        let d = EditParams::default();
        match self {
            EditSection::Light => {
                p.exposure = d.exposure;
                p.contrast = d.contrast;
                p.highlights = d.highlights;
                p.shadows = d.shadows;
                p.whites = d.whites;
                p.blacks = d.blacks;
            }
            EditSection::ToneCurve => p.curves = d.curves,
            EditSection::ToneMapping => {
                p.tone_mapper = d.tone_mapper;
                p.profile_curve = d.profile_curve;
                p.gt = d.gt;
            }
            EditSection::Colour => {
                p.temperature = d.temperature;
                p.tint = d.tint;
                p.vibrance = d.vibrance;
                p.saturation = d.saturation;
            }
            EditSection::Presence => {
                p.texture = d.texture;
                p.clarity = d.clarity;
                p.dehaze = d.dehaze;
            }
            EditSection::Detail => {
                p.luma_noise = d.luma_noise;
                p.luma_detail = d.luma_detail;
                p.color_noise = d.color_noise;
                p.ai_denoise = d.ai_denoise;
                p.sharpening = d.sharpening;
                p.sharpen_masking = d.sharpen_masking;
            }
            EditSection::Lens => {
                p.lens = d.lens;
                p.defringe = d.defringe;
            }
            EditSection::Geometry | EditSection::Masks => {}
        }
    }

    /// Does `p` hold any edits in this section?
    pub fn edited(self, p: &EditParams) -> bool {
        let d = EditParams::default();
        match self {
            EditSection::Geometry => p.quarter_turns != 0 || p.rotation != d.rotation || p.crop != d.crop,
            EditSection::Masks => p.mask_count > 0,
            _ => {
                let mut reset = *p;
                self.reset(&mut reset);
                reset != *p
            }
        }
    }
}

/// What mapping a click to the source position needs about an image: its
/// lens context, EXIF orientation and full sensor size.
pub struct SourceGeometry {
    pub lens: Arc<crate::core::lens::LensContext>,
    pub exif_orientation: u32,
    pub width: u32,
    pub height: u32,
}

/// Why a raw sample could not be taken.
pub enum RawSampleError {
    /// No usable raw data (not in memory, or too small).
    Missing(&'static str),
}

/// Mean black level over the four sites including the user's offsets: the
/// zero of the unit range `sample_raw_rgb`'s values divide by.
pub fn mean_black(raw: &crate::raw::loader::RawDataResult, offsets: [f32; 4]) -> f32 {
    raw.black_levels.iter().zip(offsets).map(|(&b, o)| b as f32 + o).sum::<f32>() / 4.0
}

/// Human-readable summary of a selection, for the status line.
fn describe_categories(c: &CopyCategories) -> String {
    let all = CopyCategories::all();
    if *c == all {
        return "all settings".to_string();
    }
    let mut parts = Vec::new();
    if c.tone { parts.push("tone"); }
    if c.color { parts.push("color"); }
    if c.white_balance { parts.push("WB"); }
    if c.noise { parts.push("noise"); }
    if c.detail { parts.push("detail"); }
    if c.geometry { parts.push("crop"); }
    if c.profile { parts.push("profile"); }
    if c.masks { parts.push("masks"); }
    if c.lens { parts.push("lens"); }
    if c.black_levels { parts.push("black levels"); }
    if parts.is_empty() {
        "nothing".to_string()
    } else {
        parts.join(", ")
    }
}

/// Tolerance for reusing a memoized DCP interpolation. `kelvin` comes out of
/// `solve_wb`, which is deterministic, so identical slider state reproduces it
/// bit-for-bit; the epsilon only absorbs noise, it does not coarsen the slider.
const DCP_MEMO_KELVIN_EPS: f32 = 0.01;

/// True when a memo recorded at `(memo_kelvin, memo_tone)` is still valid for
/// `(kelvin, tone)`.
///
/// Tone settings compare exactly rather than by epsilon: they come straight
/// off the UI as the same f32s each time, and an epsilon on a *set* of shape
/// parameters is a good way to miss a change to one of them.
pub fn dcp_memo_is_valid(
    memo_kelvin: f32,
    memo_tone: ToneSettings,
    kelvin: f32,
    tone: ToneSettings,
) -> bool {
    (memo_kelvin - kelvin).abs() < DCP_MEMO_KELVIN_EPS && memo_tone == tone
}

impl Session {
    /// Resolve the DCP interpolation and WB override for the editor's current
    /// slider state. Shared by every path that refreshes uniforms (slider edits,
    /// image-ready, preview upgrades).
    ///
    /// The interpolation depends only on `(kelvin, ToneSettings)`, but this is
    /// called from `update_pipeline` on every slider tick — including sliders that
    /// cannot affect it. It is memoized in `self.dcp_memo` because rebuilding the
    /// HueSatMap LUT and re-baking the tone curve is a multi-allocation,
    /// `powf`-heavy job that `update_uniforms` would then throw away via its own
    /// dirty-check.
    ///
    /// The two dirty-checks are independent and cannot disagree harmfully: both are
    /// functions of the same slider state, so a miss on either side merely repeats
    /// work that produces the same result.
    ///
    /// `params` is passed in rather than read from `editor` because the
    /// before/after peek renders a DIFFERENT set of params than the ones the user
    /// is editing. Reading `self.current_edit_params` here would give the peek
    /// default exposure with slider-derived white balance and tone curve — a
    /// hybrid of neither state.
    pub fn resolve_wb_and_dcp(
        &self,
        params: &crate::core::types::EditParams,
    ) -> (Option<std::sync::Arc<crate::raw::dcp::InterpolatedProfile>>, Option<[f32; 4]>) {
        let as_shot = self.as_shot_wb.unwrap_or((5000.0, 0.0));
        let fallback_matrix = self
            .current_metadata
            .as_ref()
            .map(|m| m.color_matrix)
            .unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

        let (kelvin, _tint, wb_override) = crate::color::solve_wb(
            params,
            as_shot,
            self.current_dcp_profile.as_deref(),
            fallback_matrix,
        );

        let Some(dcp) = self.current_dcp_profile.as_ref() else {
            return (None, wb_override);
        };
        let tone = ToneSettings::from_params(params);

        if let Ok(memo) = self.dcp_memo.try_borrow() {
            if let Some((k, t, profile, source)) = memo.as_ref() {
                if std::sync::Arc::ptr_eq(source, dcp) && dcp_memo_is_valid(*k, *t, kelvin, tone) {
                    return (Some(profile.clone()), wb_override);
                }
            }
        }

        let interpolated = std::sync::Arc::new(crate::raw::dcp::interpolate_at_temperature(
            dcp, kelvin, tone,
        ));
        if let Ok(mut memo) = self.dcp_memo.try_borrow_mut() {
            *memo = Some((kelvin, tone, interpolated.clone(), dcp.clone()));
        }

        (Some(interpolated), wb_override)
    }

    /// Window, target size and magnification for the current view
    /// (`core::viewport::render_target`).
    /// What a develop render of the current view needs: window, target size,
    /// and how far the screen magnifies the texture's source pixels. The source
    /// is the develop texture itself (`image_display_dims`), so a subsampled
    /// preview reports the magnification the full-resolution rebuild would fix.
    pub fn render_target(&self) -> crate::core::viewport::RenderTarget {
        let (disp_w, disp_h) = self.image_display_dims();
        let (vw, vh) = self.viewport_size;
        crate::core::viewport::render_target(
            disp_w,
            disp_h,
            vw,
            vh,
            self.zoom,
            self.pan_offset,
            self.scale_factor,
            self.is_cropping,
        )
    }
}

/// Editing: every change to the open image's edit goes through
/// [`Session::update_pipeline`], which marks it unsaved and renders it.
impl Session {
    /// Change the edit and render it. The undo step comes from the gesture's
    /// end ([`Session::commit_edit`]).
    pub fn edit(&mut self, change: impl FnOnce(&mut EditParams)) -> Effects {
        change(&mut self.current_edit_params);
        self.update_pipeline()
    }

    /// Change the edit, render it, and make it an undo step at once — for a
    /// discrete change (a button, a checkbox), which has no release.
    pub fn edit_and_commit(&mut self, change: impl FnOnce(&mut EditParams)) -> Effects {
        let effects = self.edit(change);
        self.commit_current_state();
        effects
    }

    /// Switch the tone rendering operator. Commits at once: a choice, not a drag.
    pub fn set_tone_mapper(&mut self, mapper: crate::core::tonemap::ToneMapper) -> Effects {
        if self.current_edit_params.tone_mapper == mapper {
            return Effects::none();
        }
        self.edit_and_commit(|p| p.tone_mapper = mapper)
    }

    /// Adjust one GT curve shape parameter. Clamped to the same ranges
    /// `GtParams::sanitised` enforces, so the stored value and the baked curve
    /// never disagree about what the slider meant.
    pub fn set_gt_param(&mut self, param: GtParam, v: f32) -> Effects {
        self.edit(|p| {
            let gt = &mut p.gt;
            match param {
                GtParam::Contrast => gt.contrast = v,
                GtParam::LinearStart => gt.linear_start = v,
                GtParam::LinearLength => gt.linear_length = v,
                GtParam::BlackTightness => gt.black_tightness = v,
            }
            *gt = gt.sanitised();
        })
    }

    /// ⟲ / ⟳: a quarter turn of the whole frame. The crop, masks and brush
    /// strokes turn with it (`EditParams::turn_quarter`) and so does the detail
    /// loupe's target, which is a point in the same (cropped) view; the view
    /// returns to fit, since a pan offset is a fraction of the old shape.
    pub fn turn_image(&mut self, clockwise: bool) -> Effects {
        // The turn needs the open image's own shape; resources left over from
        // another image (Library with a different selection) would scale its
        // brush strokes by the wrong aspect.
        let Some(res) = self
            .image_resources
            .as_ref()
            .filter(|r| Some(r.image_id) == self.selected_image_id)
        else {
            // Not silent: say why nothing turned.
            self.status = "Turn: the image is still loading".to_string();
            return Effects::none();
        };
        let (w, h) = res.oriented_dims(self.current_edit_params.quarter_turns);
        self.current_edit_params.turn_quarter(clockwise, w as f32 / h.max(1) as f32);
        // The crop turned rigidly with the frame, so a point in crop-local UV
        // turns the same way inside it.
        let [u, v] = self.loupe.center;
        self.loupe.center = if clockwise { [1.0 - v, u] } else { [v, 1.0 - u] };
        self.zoom = 1.0;
        self.pan_offset = cgmath::Vector2::new(0.0, 0.0);
        let effects = self.update_pipeline();
        self.commit_current_state();
        effects
    }

    /// The end of a gesture (every slider's release): an undo step, saved.
    pub fn commit_edit(&mut self) -> Effects {
        // Every slider's on_release maps here, so this is the primary persistence
        // point now that update_pipeline only marks the state dirty.
        // commit_current_state flushes.
        self.commit_current_state();
        self.finish_stage_drag()
    }

    /// A noise-reduction drag just ended: apply what the main view held back
    /// (`session::loupe`); or, if a mid-drag pause already did, from the
    /// subsampled preview (`Session::stage_drag`), render it at full resolution.
    pub fn finish_stage_drag(&mut self) -> Effects {
        if self.held_noise.is_some() {
            // `stage_drag` is already clear, so this render is full resolution.
            self.release_noise()
        } else if self.display_from_drag_preview {
            self.throttled_render()
        } else {
            Effects::none()
        }
    }

    pub fn undo(&mut self) -> Effects {
        if let Some((stack, index)) = self.get_current_history() {
            if *index > 0 {
                *index -= 1;
                self.current_edit_params = stack[*index];
                // Undo/redo don't commit to history (they move within it), so they
                // don't get commit_current_state's flush — persist explicitly.
                let effects = self.update_pipeline();
                self.flush_edits();
                return effects;
            }
        }
        Effects::none()
    }

    pub fn redo(&mut self) -> Effects {
        if let Some((stack, index)) = self.get_current_history() {
            if *index < stack.len() - 1 {
                *index += 1;
                self.current_edit_params = stack[*index];
                // See `undo`.
                let effects = self.update_pipeline();
                self.flush_edits();
                return effects;
            }
        }
        Effects::none()
    }

    /// Set a section's edits back to their defaults, as one undo step.
    pub fn reset_section(&mut self, section: EditSection) -> Effects {
        if !section.edited(&self.current_edit_params) {
            return Effects::none();
        }
        if section == EditSection::Geometry {
            // Turn back one quarter at a time, as the user turned it, so the
            // crop, masks and strokes come back with the frame.
            if let Some(res) = self.image_resources.as_ref().filter(|r| Some(r.image_id) == self.selected_image_id) {
                while self.current_edit_params.quarter_turns != 0 {
                    let (w, h) = res.oriented_dims(self.current_edit_params.quarter_turns);
                    self.current_edit_params.turn_quarter(false, w as f32 / h.max(1) as f32);
                }
            }
            let d = EditParams::default();
            self.current_edit_params.rotation = d.rotation;
            self.current_edit_params.crop = d.crop;
            self.loupe.center = [0.5, 0.5];
            self.zoom = 1.0;
            self.pan_offset = cgmath::Vector2::new(0.0, 0.0);
        } else {
            section.reset(&mut self.current_edit_params);
        }
        let effects = self.update_pipeline();
        self.commit_current_state();
        effects
    }

    /// Reset All: the edit back to defaults, as one undo step.
    pub fn reset_edits(&mut self) -> Effects {
        self.current_edit_params.reset();
        // The masks just got wiped; drop the selection/tool that pointed at them.
        self.selected_mask = None;
        self.mask_tool = MaskTool::Inactive;
        self.brush_armed = false;
        self.color_picking_mask = None;
        self.ai_select = None;
        self.status = String::new();
        // No `delete_edits` here: `update_pipeline` marks the (default) params
        // dirty and `commit_current_state` flushes them, which would write the
        // row straight back.
        let effects = self.update_pipeline();
        self.commit_current_state();
        effects
    }

    /// Before the copy-categories picker opens: is there an image to copy
    /// from? Masks can't be copied if there are none — the tick is dropped
    /// rather than letting the user confirm a category that would silently
    /// do nothing.
    pub fn prepare_copy(&mut self) -> bool {
        if self.selected_image_id.is_none() {
            return false;
        }
        if self.current_edit_params.mask_count == 0 {
            self.copy_categories.masks = false;
        }
        true
    }

    /// Copy the open image's edit: `categories`, or the ones last chosen.
    ///
    /// Both halves are stored — see `EditClipboard`. Filtering the params here
    /// would make "not copied" indistinguishable from "copied as the default
    /// value" at paste time.
    pub fn copy_settings(&mut self, categories: Option<CopyCategories>) {
        if self.selected_image_id.is_none() {
            return;
        }
        let categories = categories.unwrap_or(self.copy_categories);
        if categories.is_empty() {
            return;
        }
        self.edit_clipboard = Some(crate::core::types::EditClipboard {
            params: self.current_edit_params,
            categories,
            source_image: self.selected_image_id,
        });
        self.status = format!("Copied {}", describe_categories(&categories));
    }

    /// Tick or untick one category of what Copy takes.
    pub fn set_copy_category(&mut self, category: CopyCategory, value: bool) {
        let c = &mut self.copy_categories;
        match category {
            CopyCategory::Tone => c.tone = value,
            CopyCategory::Color => c.color = value,
            CopyCategory::WhiteBalance => c.white_balance = value,
            CopyCategory::Presence => c.presence = value,
            CopyCategory::Noise => c.noise = value,
            CopyCategory::Detail => c.detail = value,
            CopyCategory::Geometry => c.geometry = value,
            CopyCategory::Profile => c.profile = value,
            CopyCategory::Masks => c.masks = value,
            CopyCategory::Lens => c.lens = value,
            CopyCategory::BlackLevels => c.black_levels = value,
        }
    }

    pub fn set_all_copy_categories(&mut self, all: bool) {
        self.copy_categories = if all { CopyCategories::all() } else { CopyCategories::none() };
        // Never leave Masks ticked on an image that has none.
        if self.current_edit_params.mask_count == 0 {
            self.copy_categories.masks = false;
        }
    }

    /// Ctrl+V — MERGE the clipboard's selected categories onto the target(s).
    ///
    /// A merge, not a replace: only the categories the user ticked at copy time
    /// are written, and everything else on the target survives untouched.
    pub fn paste_settings(&mut self) -> Effects {
        let Some(clip) = self.edit_clipboard else {
            return Effects::none();
        };
        if clip.categories.is_empty() {
            return Effects::none();
        }

        // AI masks name bitmaps of the image they were made on.
        let for_image = |id: Option<i64>| {
            if id == clip.source_image { clip.params } else { clip.params.without_ai_masks() }
        };
        // Masks pasted without geometry follow the destination's own turn.
        let (dw, dh) = self.image_display_dims();
        let aspect = dw as f32 / dh.max(1) as f32;
        let for_target = |id: Option<i64>, dst_turns: u32| {
            crate::core::types::turned_for_paste(&for_image(id), clip.categories, dst_turns, aspect)
        };
        let src_for_current = for_target(self.selected_image_id, self.current_edit_params.quarter_turns);
        crate::core::types::apply_categories(&mut self.current_edit_params, &src_for_current, clip.categories);

        // A paste that lowered mask_count can leave `selected_mask` pointing past
        // the end of the live masks, exactly as reset_edits guards against.
        let live_masks = self.current_edit_params.mask_count as usize;
        if self.selected_mask.is_some_and(|i| i >= live_masks) {
            self.selected_mask = None;
            self.mask_tool = MaskTool::Inactive;
        }

        // Other images in the selection: load each one's own params and merge into
        // them. Writing `current_edit_params` wholesale (as this used to) would
        // overwrite the categories the user chose NOT to copy.
        // Written in one transaction: a paste onto a few hundred images used to
        // commit once per image on the UI thread.
        let mut others = 0usize;
        if let Some(lib) = &self.library {
            let merged: Vec<(i64, EditParams)> = self
                .multi_selection
                .iter()
                .filter(|&&id| Some(id) != self.selected_image_id)
                .filter_map(|&id| {
                    // An unreadable saved edit must not be merged into defaults
                    // and written back: skip the image and keep what it has.
                    let mut target = match lib.load_saved_edit(id) {
                        Ok(saved) => saved.unwrap_or_default(),
                        Err(e) => {
                            tracing::error!("Paste skipped image {id}: unreadable saved edit: {e}");
                            return None;
                        }
                    };
                    let src = for_target(Some(id), target.quarter_turns);
                    crate::core::types::apply_categories(&mut target, &src, clip.categories);
                    Some((id, target))
                })
                .collect();
            match lib.save_edit_params_many(&merged) {
                Ok(n) => others = n,
                Err(e) => tracing::error!("Failed to paste onto {} image(s): {e}", merged.len()),
            }
            // Their edits changed unrendered: their edited thumbnails are stale.
            let pasted: Vec<i64> = merged.iter().map(|(id, _)| *id).collect();
            self.invalidate_proxies(&pasted);
            // An image already visited has an undo stack that knows nothing of
            // the paste: put it on, so Undo there steps back to before it.
            // (Unvisited images seed their stack from the saved edits.) A paste
            // that left an image as it was adds no step, as for any commit
            // (`commit_current_state`), or the next Undo there does nothing.
            for (id, params) in &merged {
                if let Some((stack, index)) = self.history_map.get_mut(id) {
                    let unchanged = stack
                        .get(*index)
                        .is_some_and(|top| top.without_tool_state() == params.without_tool_state());
                    if unchanged {
                        continue;
                    }
                    stack.truncate(*index + 1);
                    stack.push(*params);
                    *index += 1;
                    if stack.len() > crate::session::HISTORY_LIMIT {
                        let excess = stack.len() - crate::session::HISTORY_LIMIT;
                        stack.drain(..excess);
                        *index -= excess;
                    }
                }
            }
        }

        self.status = if others > 0 {
            format!("Pasted {} to {} images", describe_categories(&clip.categories), others + 1)
        } else {
            format!("Pasted {}", describe_categories(&clip.categories))
        };

        let effects = self.update_pipeline();
        self.commit_current_state();
        effects
    }
}

/// Tools: one at a time, since a click or drag on the image can only mean
/// one thing.
impl Session {
    /// Arm/disarm the level tool (Develop only).
    pub fn toggle_level_tool(&mut self) -> Effects {
        if self.current_tab != AppTab::Develop {
            return Effects::none();
        }
        self.is_leveling = !self.is_leveling;
        if self.is_leveling {
            self.is_cropping = false;
            self.is_wb_picking = false;
            self.mask_tool = MaskTool::Inactive;
            self.selected_mask = None;
            self.brush_armed = false;
            self.color_picking_mask = None;
            self.loupe.targeting = false;
            self.status = "Level: drag along a line that should be level or upright".to_string();
        } else {
            self.status.clear();
        }
        self.update_pipeline()
    }

    /// The level line was drawn: straighten so it shows level (or upright). A
    /// line too short to aim with keeps the tool armed for another try.
    pub fn level_line(&mut self, dx: f32, dy: f32) -> Effects {
        let current = self.current_edit_params.rotation;
        let Some(angle) = crate::core::viewport::straighten_from_line(current, dx, dy) else {
            self.status = "Level: drag a longer line along the horizon or an upright".to_string();
            return Effects::none();
        };
        self.is_leveling = false;
        self.status = format!("Straightened by {:+.1}°", angle - current);
        self.edit_and_commit(|p| p.rotation = angle)
    }

    /// Crop mode on/off (Develop only). Without the tab check, pressing C in
    /// Library flipped `is_cropping` and ran a full update_pipeline with
    /// nothing on screen to show for it — the user only met the consequences
    /// on their next visit to Develop.
    pub fn toggle_crop(&mut self) -> Effects {
        if self.current_tab != AppTab::Develop {
            return Effects::none();
        }
        self.is_wb_picking = false; // tools are mutually exclusive
        self.is_leveling = false;
        self.mask_tool = MaskTool::Inactive;
        self.selected_mask = None;
        self.brush_armed = false;
        self.color_picking_mask = None;
        self.loupe.targeting = false;
        self.is_cropping = !self.is_cropping;
        // update_pipeline mirrors is_cropping into the params for the shader.
        self.update_pipeline()
    }

    /// WB eyedropper on/off (Develop only).
    pub fn toggle_wb_picker(&mut self) -> Effects {
        if self.current_tab != AppTab::Develop {
            return Effects::none();
        }
        self.is_wb_picking = !self.is_wb_picking;
        if self.is_wb_picking {
            // update_pipeline below clears the shader's crop flag to match.
            self.is_cropping = false;
            self.mask_tool = MaskTool::Inactive;
            self.selected_mask = None;
            self.brush_armed = false;
            self.color_picking_mask = None;
            self.loupe.targeting = false;
            self.is_leveling = false;
            self.status = "WB picker: click a neutral grey/white area".to_string();
        } else {
            self.status.clear();
        }
        self.update_pipeline()
    }

    /// [`SourceGeometry`] of `image_id`, from its GPU resources (always there
    /// while it is shown in Develop) or else the RAW cache. Never only the RAW
    /// cache: an entry can be evicted while the image is open, and the brush and
    /// AI-select mappings then silently fell back to the un-straightened frame.
    pub fn source_geometry(&self, image_id: i64) -> Option<SourceGeometry> {
        if let Some(res) = self.image_resources.as_ref().filter(|r| r.image_id == image_id) {
            return Some(SourceGeometry {
                lens: res.lens.clone(),
                exif_orientation: res.orientation,
                width: res.original_width,
                height: res.original_height,
            });
        }
        self.raw_cache.peek(&image_id).map(|raw| SourceGeometry {
            lens: raw.lens_context.clone(),
            exif_orientation: raw.orientation,
            width: raw.width,
            height: raw.height,
        })
    }

    /// Mean black-subtracted camera (R, G, B) of the raw mosaic in a 16×16
    /// window at `(u, v)` — display-space UV within the rendered (cropped) image,
    /// as the overlay's eyedroppers report it — plus whether any site clipped.
    ///
    /// Shared by the WB picker and the colour-range mask picker. The UV goes
    /// through the same crop remap and EXIF orientation as the fragment shader.
    pub fn sample_raw_rgb(
        &self,
        image_id: i64,
        u: f32,
        v: f32,
    ) -> Result<([f32; 3], bool, Arc<crate::raw::loader::RawDataResult>), RawSampleError> {
        let Some(raw) = self.raw_for(image_id) else {
            return Err(RawSampleError::Missing("raw data not in memory (reopen the image)"));
        };
        // The 16×16 sample window below needs room on both sides; `clamp(8, w-9)`
        // panics outright on anything narrower.
        if raw.data.is_empty() || raw.width < 17 || raw.height < 17 {
            return Err(RawSampleError::Missing("no raw pixels available"));
        }

        // Display UV → sensor UV through the colour pass's own chain: crop remap,
        // Straighten, EXIF orientation plus the user's quarter turns, lens remap.
        let orientation = crate::color::compose_orientation(raw.orientation, self.current_edit_params.quarter_turns);
        let (su, sv) = crate::core::viewport::display_to_source_uv(
            &self.current_edit_params,
            &raw.lens_context,
            orientation,
            raw.width,
            raw.height,
            u,
            v,
        );

        // Sample a 16×16 sensor window centred on the pick, per CFA channel.
        let w = raw.width as i64;
        let h = raw.height as i64;
        let cx = ((su * w as f32) as i64).clamp(8, w - 9);
        let cy = ((sv * h as f32) as i64).clamp(8, h - 9);

        // Each site's own black level plus the user's offset, exactly as the
        // demosaic uploads them. (Reads `raw.data`: with AI denoise on, the shader
        // sees the blended mosaic and this the original; a 16x16 mean hardly
        // notices.)
        let offsets = self.current_edit_params.black_offsets;
        let slots = crate::gpu::demosaic::tile_black_slots(raw.cfa_pattern);
        let site_black = |x: i64, y: i64| {
            let slot = slots[((y % 2) * 2 + (x % 2)) as usize];
            raw.black_levels[slot] as f32 + offsets[slot]
        };
        let clip_threshold = (raw.white_level as f32 * 0.98) as u16;

        let mut sums = [0.0f64; 3]; // R, G, B
        let mut counts = [0u32; 3];
        let mut clipped = false;
        for y in (cy - 8)..(cy + 8) {
            for x in (cx - 8)..(cx + 8) {
                let value = raw.data[(y * w + x) as usize];
                clipped |= value >= clip_threshold;
                // CFA channel for (x%2, y%2) per pattern: 0=RGGB 1=GRBG 2=GBRG 3=BGGR
                let (px, py) = ((x % 2) as u32, (y % 2) as u32);
                let channel = match (raw.cfa_pattern, py, px) {
                    (0, 0, 0) | (1, 0, 1) | (2, 1, 0) | (3, 1, 1) => 0, // R
                    (0, 1, 1) | (1, 1, 0) | (2, 0, 1) | (3, 0, 0) => 2, // B
                    _ => 1,                                             // G (both sites)
                };
                sums[channel] += (value as f32 - site_black(x, y)).max(0.0) as f64;
                counts[channel] += 1;
            }
        }
        let mean = |c: usize| (sums[c] / counts[c].max(1) as f64) as f32;
        Ok(([mean(0), mean(1), mean(2)], clipped, raw))
    }

    /// WB eyedropper: `(u, v)` is the display-space UV of the click within the
    /// rendered image. Samples the raw Bayer mosaic around that point, derives the
    /// neutral's WB multipliers, converts to Kelvin/tint, and sets the sliders.
    pub fn wb_picked(&mut self, u: f32, v: f32) -> Effects {
        // On a failed pick (clipped/dark area) the tool stays active so the user
        // can simply click elsewhere; it exits on success or on missing data.
        let Some(image_id) = self.selected_image_id else {
            self.is_wb_picking = false;
            return Effects::none();
        };
        let (rgb, clipped, raw) = match self.sample_raw_rgb(image_id, u, v) {
            Ok(sample) => sample,
            Err(RawSampleError::Missing(msg)) => {
                self.is_wb_picking = false;
                self.status = format!("WB picker: {msg}");
                return Effects::none();
            }
        };
        if clipped {
            self.status = "WB picker: area is clipped — pick a darker neutral".to_string();
            return Effects::none();
        }
        let [r, g, b] = rgb;
        if r < 1.0 || g < 1.0 || b < 1.0 {
            self.status = "WB picker: area too dark for a reliable neutral".to_string();
            return Effects::none();
        }

        // Neutral → multipliers → (Kelvin, tint) through the camera matrix.
        let wb_picked = [(g / r).clamp(0.05, 20.0), 1.0, (g / b).clamp(0.05, 20.0), 1.0];
        let matrices = match self.current_dcp_profile.as_deref() {
            Some(p) => crate::color::CameraMatrices::Dcp(p),
            None => crate::color::CameraMatrices::Single(raw.color_matrix),
        };
        let (kelvin, tint) = crate::color::wb_to_kelvin_tint(wb_picked, &matrices);

        // Absolute Kelvin/tint → slider offsets around the as-shot anchor
        // (inverse of EditParams::kelvin_from_anchor / tint_from_anchor).
        let (anchor_kelvin, anchor_tint) = self.as_shot_wb.unwrap_or((5000.0, 0.0));
        let temperature = EditParams::temperature_for_kelvin(anchor_kelvin, kelvin);
        let tint_offset = EditParams::tint_slider_for(anchor_tint, tint);

        self.is_wb_picking = false;
        self.current_edit_params.temperature = temperature;
        self.current_edit_params.tint = tint_offset;
        // Report what the sliders now render, which differs from the pick when
        // it was out of their range and got clamped.
        let set = &self.current_edit_params;
        self.status = format!(
            "WB picked: {:.0}K, tint {:+.0}",
            set.kelvin_from_anchor(anchor_kelvin),
            set.tint_from_anchor(anchor_tint)
        );

        let effects = self.update_pipeline();
        self.commit_current_state();
        effects
    }
}

/// The render loop. One render is in flight at a time and at most one more
/// is pending (`is_rendering` / `pending_render`); a uniform push that
/// arrives mid-render is parked (`deferred_push`) and applied before the
/// next frame, so the uniform block has one writer.
impl Session {
    /// The edit changed: mark it unsaved and render it.
    pub fn update_pipeline(&mut self) -> Effects {
        // Deferred, not written here: this runs once per mouse-move during a
        // slider or mask drag, and a synchronous SQLite write on that path cost
        // an fsync per event on the UI thread. `flush_edits` at the commit points
        // (CommitEdit, drag release, image/tab switch, close) does the write.
        self.mark_edits_dirty();
        self.sync_tool_flags();
        // EditParams is Copy, so this sidesteps the borrow conflict for free.
        let params = self.view_params();
        self.push_uniforms_and_render(&params)
    }

    /// Write `params` (with the resolved DCP and slider WB) to the uniforms. Only
    /// while no render is in flight — see `Session::deferred_push`.
    fn push_uniforms(&self, params: &EditParams) {
        if let (Some(ctx), Some(resources)) = (&self.gpu_context, &self.image_resources) {
            let (interpolated, wb_override) = self.resolve_wb_and_dcp(params);
            resources.update_uniforms(ctx, params, interpolated.as_deref(), wb_override);
        }
    }

    /// Apply a push that arrived mid-render. Runs at the start of every render,
    /// when the previous one has finished, so the uniform block has one writer.
    fn apply_deferred_push(&mut self) {
        let Some((id, params)) = self.deferred_push.take() else { return };
        if self.image_resources.as_ref().map(|r| r.image_id) == Some(id) {
            self.push_uniforms(&params);
        }
    }

    /// Push `params` to the GPU and render with them.
    ///
    /// Deliberately does NOT mark edits dirty — the caller decides whether these
    /// params represent the user's real state. The before/after peek renders
    /// params that must never be persisted.
    pub fn push_uniforms_and_render(&mut self, params: &EditParams) -> Effects {
        // `params` belong to the SELECTED image, but the loaded GPU resources can
        // still be another one's — Library and Cull change the selection without
        // loading. Rendering them together (an undo or reset pressed in Library)
        // drew the previous Develop image with this image's edits, and Develop
        // then showed that frame when the user went back to it.
        if self.image_resources.as_ref().map(|r| r.image_id) != self.selected_image_id {
            return Effects::none();
        }
        // A different camera profile can't be pushed as uniforms (its LUT sizes
        // a texture): undo, paste or reset changed it — rebuild instead.
        // Likewise a different AI denoise amount: it is baked into the mosaic.
        if self.camera_profile_stale() || self.ai_denoise_stale() {
            return self.reload_selected_image();
        }
        if self.is_rendering {
            // A render task is between its own uniform writes and submits: writing
            // the block from here could land `view_rect = FULL` in its main pass,
            // or change the params between its stages and its colour pass. Park
            // the push; `render` applies it before the next frame.
            if let Some(id) = self.selected_image_id {
                self.deferred_push = Some((id, *params));
            }
            self.changed.insert(Changed::OVERLAY);
        } else {
            self.push_uniforms(params);
        }

        // The detail loupe follows every edit (it costs ~0.3 ms when the noise
        // settings haven't changed).
        let loupe = self.request_loupe();
        Effects::batch([self.throttled_render(), loupe])
    }

    /// The open image's resources were built with a different AI denoise
    /// amount than its edit now has (Apply, Remove, undo, paste…): rebuild.
    pub fn ai_denoise_stale(&self) -> bool {
        self.image_resources
            .as_ref()
            .is_some_and(|r| r.ai_denoise.0 != self.current_edit_params.ai_denoise)
    }

    /// Start a render unless one is already in flight, in which case mark a
    /// follow-up. At most one running plus one pending — without this, a stream of
    /// scroll or drag events spawns a GPU task each, piling up readback buffers.
    pub fn throttled_render(&mut self) -> Effects {
        if self.surface_drawn {
            self.changed.insert(Changed::FRAME);
            return Effects::none();
        }
        if self.is_rendering {
            if self.pending_render {
                // Folded into the render already waiting.
                self.profiler.next.bump(crate::core::profiler::Count::Merged);
            }
            self.pending_render = true;
            Effects::none()
        } else {
            self.is_rendering = true;
            self.pending_render = false;
            self.render()
        }
    }

    /// Render the current view. Callers hold the throttle (`is_rendering`);
    /// [`Session::throttled_render`] is the way in.
    pub fn render(&mut self) -> Effects {
        let t_update_start = std::time::Instant::now();
        let mut detail = std::mem::take(&mut self.profiler.next);
        detail.start();
        self.apply_deferred_push();

        // Oriented display dims of what is actually on screen — crop-aware, so the
        // render target takes the CROPPED aspect. Must be the same source the
        // viewport widget and the input math use, or the texture is drawn at a
        // different shape than it was rendered for.
        let (disp_w, disp_h) = self.image_display_dims();

        let (Some(ctx), Some(resources)) = (self.gpu_context.clone(), self.image_resources.clone()) else {
            // GPU context not ready — release the lock so the next slider event can retry.
            self.is_rendering = false;
            self.pending_render = false;
            return Effects::none();
        };

        // Render only the slice of the image that is actually on screen, at
        // the resolution it is displayed at. Sizing the target to the whole
        // image at zoom-scaled resolution made its area grow as zoom^2 — at
        // 100% on a 24MP file that meant rendering, reading back and
        // re-uploading ~24M pixels to display ~2M. (Crop mode reveals the
        // full sensor and disables zoom/pan, so it renders the whole image.)
        let target = self.render_target();
        let view_rect = target.view_rect;
        let (target_w, target_h) = (target.width, target.height);

        // Only the REQUESTED window moves here. `rendered_view_rect` — what the
        // viewport actually draws against — is published in `render_finished`,
        // together with the bytes it describes. Updating it now would place the
        // still-current texture at the incoming window's position for the
        // length of the render, which looks like the image jumping.
        self.requested_view_rect = view_rect;
        tracing::debug!(
            "render window: zoom={:.2} view_rect=[{:.3},{:.3},{:.3},{:.3}] target={}x{}",
            self.zoom,
            view_rect[0],
            view_rect[1],
            view_rect[2],
            view_rect[3],
            target_w,
            target_h,
        );
        let update_ms = t_update_start.elapsed().as_secs_f32() * 1000.0;
        detail.add_ms(crate::core::profiler::Span::Update, update_ms);

        let overview_dims = crate::core::viewport::overview_dims(disp_w, disp_h);
        let image_id = resources.image_id;
        // Only a render of the user's real edit may become the image's proxy:
        // the before/after peek shows other params, and crop mode shows the
        // whole sensor with the discarded area dimmed. The uniforms hold
        // `current_edit_params` whenever neither is active — every change to
        // them is pushed to the GPU as it happens.
        // The mask overlay tints the render, so it disqualifies too.
        // So does one whose noise reduction is held back mid-drag.
        let edit = (!self.before_peek
            && !self.is_cropping
            && self.current_edit_params.mask_overlay == 0
            && self.held_noise.is_none())
        .then_some(self.current_edit_params);

        // A subsampled texture shown larger than its own pixels is soft
        // however it is rendered: fetch full resolution. Asked here, by the
        // render itself, rather than by the zoom handler, so it also covers
        // a view that is already zoomed in when an image loads (zoom is kept
        // across images and tabs), a window resize, and HiDPI screens.
        let upgrade = if target.magnification > crate::core::viewport::FULL_RES_MAGNIFICATION
            && self.current_tab == AppTab::Develop
        {
            self.trigger_full_res_upgrade()
        } else {
            Effects::none()
        };

        // After a full-resolution upgrade the subsampled preview is kept
        // (`preview_resources`). Render from it whenever full resolution adds
        // nothing: the overview always (384px), and the display while the
        // view does not magnify the preview — or while a noise-reduction
        // slider is dragged, whose stage re-runs over every SOURCE pixel per
        // tick. The release re-renders at full resolution (`finish_stage_drag`).
        let preview = self.preview_resources.clone().filter(|p| p.image_id == resources.image_id);
        let (display_res, overview_res) = match preview {
            Some(preview) => {
                // Same state as the full-resolution set: its uniforms are the
                // ones every edit path pushes to.
                let params = resources.pushed_params();
                let (interpolated, wb_override) = self.resolve_wb_and_dcp(&params);
                preview.update_uniforms(&ctx, &params, interpolated.as_deref(), wb_override);
                let preview_magnification =
                    target.magnification * resources.width as f32 / preview.width.max(1) as f32;
                let fits = preview_magnification <= crate::core::viewport::FULL_RES_MAGNIFICATION;
                self.display_from_drag_preview = !fits && self.stage_drag;
                let display = if fits || self.stage_drag { preview.clone() } else { resources };
                if !(fits || self.stage_drag) {
                    detail.bump(crate::core::profiler::Count::FullRes);
                }
                (display, preview)
            }
            None => {
                self.display_from_drag_preview = false;
                (resources.clone(), resources)
            }
        };

        let render = Effects::perform("render", async move {
            // Display window and the whole-image overview share one GPU
            // wait; the overview is ~100k pixels, so there is no reason to
            // report it separately from the render.
            let render = crate::gpu::render_functions::render_display_and_overview(
                &ctx,
                &display_res,
                &overview_res,
                target_w,
                target_h,
                view_rect,
                overview_dims,
                detail,
            )
            .await;
            SessionEvent::RenderFinished(Box::new(RenderOutput {
                image_id,
                display: render.display,
                dims: (target_w, target_h),
                view_rect,
                overview: render.overview,
                overview_dims,
                histogram: render.histogram,
                edit,
                upload_ms: render.encode_ms,
                render_ms: render.gpu_ms,
                update_ms,
                detail: render.detail,
            }))
        });
        Effects::batch([render, upgrade])
    }

    // The single landing point for a completed develop render: stores the new
    // preview, applies the histogram, keeps the overview as the image's edited
    // proxy, and releases the render throttle.
    //
    // This used to be split into RenderPreview (release the throttle) then
    // HistogramReady (the slow part), because the histogram scanned the entire
    // display buffer on the CPU. The dedicated small overview pass made that scan
    // trivial, so the split — and the extra message round-trip — no longer earn
    // their keep.
    pub fn render_finished(&mut self, out: RenderOutput) -> Effects {
        if self.gpu_context.as_ref().is_some_and(|c| c.is_lost()) {
            return self.gpu_lost();
        }
        // A render of the image the user has since navigated away from: drop the
        // pixels, but still release the throttle below so the next render runs.
        let current = self.image_resources.as_ref().map(|r| r.image_id);
        if current == Some(out.image_id) && !out.display.is_empty() {
            self.rendered_preview_bytes = Some(out.display);
            self.rendered_preview_dims = out.dims;
            // Buffer and window swap in the same frame — see the note in
            // `render` about why this is not published earlier.
            self.rendered_view_rect = out.view_rect;
            self.bump_preview_generation();
            *self.histogram_data.borrow_mut() = out.histogram;
            self.changed.insert(Changed::HISTOGRAM | Changed::OVERLAY);
        }
        // The overview is a faithful render of `out.image_id` even when the user
        // has already moved on — which is exactly when `persist` may be waiting
        // for it (see `Session::resolve_awaiting_proxy`).
        let awaited = self.proxy_awaiting.contains_key(&out.image_id);
        if let Some(params) = out.edit.filter(|_| current == Some(out.image_id) || awaited) {
            self.capture_proxy(out.image_id, out.overview, out.overview_dims, params);
        }
        let save = self.resolve_awaiting_proxy(out.image_id, out.edit);

        let mut detail = out.detail;
        detail.finish();
        self.profiler.push_frame(crate::core::profiler::ProfilerFrame {
            update_ms: out.update_ms,
            upload_ms: out.upload_ms,
            render_ms: out.render_ms,
            total_ms: out.update_ms + out.upload_ms + out.render_ms,
            detail,
        });
        self.changed.insert(Changed::PROFILER);

        // A new image, a full-resolution swap or a DPI change re-render the main
        // view without pushing params; the loupe catches up here.
        let loupe = self.refresh_loupe_if_stale();

        self.is_rendering = false;
        if self.pending_render {
            self.is_rendering = true;
            self.pending_render = false;
            return Effects::batch([save, loupe, self.render()]);
        }
        Effects::batch([save, loupe])
    }

    /// The GPU device is gone (driver reset, suspend/resume, eGPU unplug): every
    /// readback would fail from here on. Drop the context and the resources built
    /// on it, so the next load creates a fresh one (`shared_context` would
    /// otherwise hand back the dead one from `gpu_init`), and reload the
    /// selected image.
    pub fn gpu_lost(&mut self) -> Effects {
        tracing::error!("GPU device lost; rebuilding the GPU context");
        self.status = "The GPU was reset; reloading the image.".into();
        self.gpu_context = None;
        self.gpu_init = Arc::new(tokio::sync::OnceCell::new());
        self.image_resources = None;
        self.preview_resources = None;
        self.is_rendering = false;
        self.pending_render = false;
        self.deferred_push = None;
        self.reload_selected_image()
    }

    /// Render the params the viewport should currently be showing — the
    /// before-reference while peeking, the user's real edits otherwise.
    ///
    /// Every path that needs to *leave* the peek goes through here rather than
    /// poking `before_peek` directly, so the flag and the uploaded uniforms can
    /// never disagree.
    pub fn refresh_for_peek_state(&mut self) -> Effects {
        let params = if self.before_peek {
            self.current_edit_params.before_reference()
        } else {
            self.view_params()
        };
        self.push_uniforms_and_render(&params)
    }

    /// Leave the before/after peek if it is active, re-rendering the real edit.
    ///
    /// Called from anywhere the peek could otherwise get stranded — tab change,
    /// image change, modal open, window focus loss. Nothing to do → no effects,
    /// so it is safe to call unconditionally.
    pub fn clear_before_peek(&mut self) -> Effects {
        if !self.before_peek {
            return Effects::none();
        }
        self.before_peek = false;
        self.refresh_for_peek_state()
    }

    /// Hold-to-peek at the unedited original (Space).
    ///
    /// Hold rather than toggle: releasing the key always returns you to your edit,
    /// so you cannot get stranded looking at the original wondering why the
    /// sliders do nothing. Only in Develop, on a loaded image.
    pub fn set_before_peek(&mut self, active: bool) -> Effects {
        // Key auto-repeat delivers KeyPressed continuously while Space is held.
        // Without this guard every repeat would queue another GPU render.
        if self.before_peek == active {
            return Effects::none();
        }
        if active
            && (self.current_tab != AppTab::Develop
                || !matches!(self.editor_readiness, EditorReadiness::Ready(_)))
        {
            return Effects::none();
        }
        self.before_peek = active;
        self.refresh_for_peek_state()
    }
}

/// The view: zoom and pan (in fitted-size fractions, `core::viewport`), and
/// the drags on the image's handles. Positions are in the viewport's logical
/// pixels.
impl Session {
    /// Zoom by a wheel step `d` about `at` (kept fixed under the cursor).
    pub fn zoom_at(&mut self, d: f32, at: [f32; 2]) -> Effects {
        if self.is_cropping {
            return Effects::none();
        }
        let step = |zoom: f32| if d > 0.0 { zoom * (1.0 + d * 0.8) } else { zoom / (1.0 + (-d * 0.8)) }.clamp(0.1, 10.0);
        if self.image_resources.is_some() {
            let old_zoom = self.zoom;
            let (dw, dh) = self.image_display_dims();
            let (vw, vh) = self.viewport_size;
            let (fw, fh) = crate::core::viewport::fitted_size(dw, dh, vw, vh);
            let ib = crate::core::viewport::image_rect(dw, dh, vw, vh, old_zoom, self.pan_offset);

            // Fraction of the CURRENT on-screen image rect under the cursor —
            // this is the point we want to keep fixed on screen as zoom changes.
            let tx = ((at[0] - ib.x) / ib.width.max(1.0)).clamp(0.0, 1.0);
            let ty = ((at[1] - ib.y) / ib.height.max(1.0)).clamp(0.0, 1.0);

            let new_zoom = step(old_zoom);
            self.zoom = new_zoom;

            // Closed-form inversion of image_rect: solve pan so that the point at
            // fraction (tx, ty) of the rect lands back under the cursor at the
            // new zoom. From image_rect: p.x = vw/2 - fw*zoom/2 + offset.x*fw + tx*fw*zoom
            //   => offset.x = (p.x - vw/2)/fw - zoom*(tx - 0.5)
            self.pan_offset.x = (at[0] - vw / 2.0) / fw.max(1.0) - new_zoom * (tx - 0.5);
            self.pan_offset.y = (at[1] - vh / 2.0) / fh.max(1.0) - new_zoom * (ty - 0.5);
        } else {
            self.zoom = step(self.zoom);
        }
        self.changed.insert(Changed::OVERLAY);

        // Re-render at zoom-appropriate resolution (which upgrades to the full-res
        // debayer once the subsampled texture is past its useful range).
        if self.current_tab == AppTab::Develop && matches!(self.editor_readiness, EditorReadiness::Ready(_)) {
            // Zoom always re-renders: the window and its resolution both change.
            // Throttled so a stream of scroll events can't pile up GPU tasks.
            // The render itself asks for full resolution once the view needs it.
            return self.throttled_render();
        }
        Effects::none()
    }

    /// A pointer move of `delta` viewport pixels as a pan offset: fitted-size
    /// fractions, with no zoom term (see `core::viewport::image_rect`), so
    /// panning by it tracks the cursor 1:1 at any zoom.
    pub fn pan_fraction(&self, delta: [f32; 2]) -> [f32; 2] {
        let (vw, vh) = self.viewport_size;
        let (dw, dh) = self.image_display_dims();
        let (fw, fh) = crate::core::viewport::fitted_size(dw, dh, vw, vh);
        [
            if fw > 0.0 { delta[0] / fw } else { 0.0 },
            if fh > 0.0 { delta[1] / fh } else { 0.0 },
        ]
    }

    /// Pan by `d` (fitted-size fractions).
    pub fn pan(&mut self, d: [f32; 2]) -> Effects {
        if self.is_cropping {
            return Effects::none();
        }
        self.pan_offset.x += d[0];
        self.pan_offset.y += d[1];
        self.changed.insert(Changed::OVERLAY);

        // The pan itself stays free — the viewport shader just re-projects the
        // existing texture. But that texture now only covers `rendered_view_rect`,
        // so once the user drags past the overscan margin there is nothing left to
        // project and the window has to be re-rendered.
        if self.needs_render_for_view() {
            return self.throttled_render();
        }
        Effects::none()
    }

    /// True when the on-screen region has moved outside what the current preview
    /// buffer covers. Always false at fit, where the buffer holds the whole image.
    fn needs_render_for_view(&self) -> bool {
        if self.current_tab != AppTab::Develop || !matches!(self.editor_readiness, EditorReadiness::Ready(_)) {
            return false;
        }
        let (dw, dh) = self.image_display_dims();
        let (vw, vh) = self.viewport_size;
        // Overscan 1.0: ask only what is strictly visible right now. The rendered
        // window already carries the margin, so this asks "have we run out of
        // margin?" rather than "has anything moved at all?".
        let needed = crate::core::viewport::visible_view_rect(dw, dh, vw, vh, self.zoom, self.pan_offset, 1.0);
        // Against the REQUESTED window: if a render covering this area is already
        // on its way, asking for another one would just churn.
        !crate::core::viewport::view_rect_contains(self.requested_view_rect, needed)
    }

    /// A touch gesture: scale the zoom by `scale` about `centroid`, then
    /// move by `pan` (viewport pixels), so the point under the fingers stays
    /// under them. Zoom stays between fit and 10×, and the pan within the
    /// image's edges (`core::viewport::clamp_pan`). Starts no render: for
    /// front ends that draw on their own clock (`surface_drawn`), which see
    /// `Changed::FRAME`.
    pub fn transform_view(&mut self, scale: f32, centroid: [f32; 2], pan: [f32; 2]) {
        if self.is_cropping || !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let (dw, dh) = self.image_display_dims();
        let (vw, vh) = self.viewport_size;
        let (fw, fh) = crate::core::viewport::fitted_size(dw, dh, vw, vh);
        let ib = crate::core::viewport::image_rect(dw, dh, vw, vh, self.zoom, self.pan_offset);
        // The image point under the centroid, as a fraction of the image.
        let tx = (centroid[0] - ib.x) / ib.width.max(1.0);
        let ty = (centroid[1] - ib.y) / ib.height.max(1.0);
        let zoom = (self.zoom * scale).clamp(1.0, 10.0);
        // Inverting image_rect (see `zoom_at`): that point lands at centroid + pan.
        let offset = cgmath::Vector2::new(
            (centroid[0] + pan[0] - vw / 2.0) / fw.max(1.0) - zoom * (tx - 0.5),
            (centroid[1] + pan[1] - vh / 2.0) / fh.max(1.0) - zoom * (ty - 0.5),
        );
        self.zoom = zoom;
        self.pan_offset = crate::core::viewport::clamp_pan(dw, dh, vw, vh, zoom, offset);
        self.changed.insert(Changed::OVERLAY | Changed::FRAME);
    }

    /// Fit the image inside the viewport less `insets` (left, top, right,
    /// bottom, viewport pixels), never larger than fit. For the crop tool on
    /// a phone, whose screen edges belong to system gestures: its handles
    /// sit at the image's edges. [`Session::reset_view`] returns to fit.
    /// Starts no render (`Changed::FRAME`, as `transform_view`).
    pub fn inset_view(&mut self, insets: [f32; 4]) {
        let [l, t, r, b] = insets.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
        let (dw, dh) = self.image_display_dims();
        let (vw, vh) = self.viewport_size;
        let (fw, fh) = crate::core::viewport::fitted_size(dw, dh, vw, vh);
        if fw <= 0.0 || fh <= 0.0 {
            return;
        }
        let zoom = ((vw - l - r) / fw).min((vh - t - b) / fh).clamp(0.1, 1.0);
        // image_rect centres the image at vw/2 + offset·fw: move it to the
        // middle of the inset area.
        self.zoom = zoom;
        self.pan_offset = cgmath::Vector2::new((l - r) / 2.0 / fw, (t - b) / 2.0 / fh);
        self.changed.insert(Changed::OVERLAY | Changed::FRAME);
    }

    /// Back to fit.
    pub fn reset_view(&mut self) -> Effects {
        self.zoom = 1.0;
        self.pan_offset = cgmath::Vector2::new(0.0, 0.0);
        self.changed.insert(Changed::OVERLAY);
        // Must re-render: the buffer holds whatever window the previous zoom
        // needed, so without this the viewport would draw that small slice
        // correctly positioned inside the now fit-sized image — the rest blank.
        self.throttled_render()
    }

    /// A crop or mask handle drag ended: an undo step, and the uniforms the
    /// drag pushed brought up to date with it.
    pub fn end_handle_drag(&mut self) {
        self.commit_current_state();
        if let (Some(ctx), Some(res)) = (&self.gpu_context, &self.image_resources) {
            let params = self.view_params();
            let (interpolated, wb_override) = self.resolve_wb_and_dcp(&params);
            res.update_uniforms(ctx, &params, interpolated.as_deref(), wb_override);
        }
    }

    /// Drag crop handle `h` from `from` to `to`.
    pub fn drag_crop(&mut self, h: CropHandle, from: [f32; 2], to: [f32; 2]) {
        self.changed.insert(Changed::OVERLAY);
        let Some(resources) = &self.image_resources else {
            return;
        };

        let (dw, dh) = self.image_display_dims();
        let (bw, bh) = self.viewport_size;
        let (fw, fh) = crate::core::viewport::fitted_size(dw, dh, bw, bh);

        let zw = fw * self.zoom;
        let zh = fh * self.zoom;

        // Scale delta from screen pixels to normalized image [0,1]
        let dx = (to[0] - from[0]) / zw;
        let dy = (to[1] - from[1]) / zh;

        let c = self.current_edit_params.crop;
        let (mut l, mut t, mut r, mut b) = (c[0], c[1], c[0] + c[2], c[1] + c[3]);

        match h {
            CropHandle::TopLeft => { l += dx; t += dy; }
            CropHandle::TopRight => { t += dy; r += dx; }
            CropHandle::BottomLeft => { l += dx; b += dy; }
            CropHandle::BottomRight => { r += dx; b += dy; }
            CropHandle::Body => {
                l += dx; t += dy; r += dx; b += dy;
                if l < 0.0 { r -= l; l = 0.0; }
                if r > 1.0 { l -= r - 1.0; r = 1.0; }
                if t < 0.0 { b -= t; t = 0.0; }
                if b > 1.0 { t -= b - 1.0; b = 1.0; }
            }
        }

        if h != CropHandle::Body {
            let min = 0.01;
            match h {
                CropHandle::TopLeft => { l = l.min(r - min).max(0.0); t = t.min(b - min).max(0.0); }
                CropHandle::TopRight => { r = r.max(l + min).min(1.0); t = t.min(b - min).max(0.0); }
                CropHandle::BottomLeft => { l = l.min(r - min).max(0.0); b = b.max(t + min).min(1.0); }
                CropHandle::BottomRight => { r = r.max(l + min).min(1.0); b = b.max(t + min).min(1.0); }
                _ => {}
            }
        }

        let l = l.clamp(0.0, 1.0);
        let t = t.clamp(0.0, 1.0);
        let r = r.clamp(0.0, 1.0);
        let b = b.clamp(0.0, 1.0);

        self.current_edit_params.crop = [l, t, (r - l).max(0.0), (b - t).max(0.0)];
        // The release commits via commit_current_state, whose flush only writes
        // when dirty. Nothing else marks a crop drag dirty, so every drag after
        // the first in one crop session was dropped on close. (The field, not
        // `mark_edits_dirty()`: `resources` still borrows `self.image_resources`.)
        self.edits_dirty = true;

        if let Some(ctx) = &self.gpu_context {
            let params = self.view_params();
            let (interpolated, wb_override) = self.resolve_wb_and_dcp(&params);
            resources.update_uniforms(ctx, &params, interpolated.as_deref(), wb_override);
        }
    }

    /// Drag handle `h` of the selected mask from `from` to `to`. Unlike crop
    /// (drawn over the image), a mask's effect is in the render: it re-renders
    /// live, throttled.
    pub fn drag_mask(&mut self, h: MaskHandle, from: [f32; 2], to: [f32; 2]) -> Effects {
        self.move_mask_handle(h, from, to);
        self.changed.insert(Changed::OVERLAY);
        self.update_pipeline()
    }

    fn move_mask_handle(&mut self, h: MaskHandle, from: [f32; 2], to: [f32; 2]) {
        let Some(index) = self.selected_mask else {
            return;
        };
        if index >= self.current_edit_params.mask_count as usize || self.image_resources.is_none() {
            return;
        }

        // Same letterbox/zoom math as drag_crop, sharing the same dims
        // source as every other on-screen geometry consumer.
        let (dw, dh) = self.image_display_dims();
        let (bw, bh) = self.viewport_size;
        let (fw, fh) = crate::core::viewport::fitted_size(dw, dh, bw, bh);
        let zoom = self.zoom;
        let pan_offset = self.pan_offset;

        let zw = fw * zoom;
        let zh = fh * zoom;

        // Screen deltas map to visible-area UV; mask geometry lives in full-image
        // UV (the crop sub-rect is what's displayed), so scale by the crop extent.
        let crop = self.current_edit_params.crop;
        let du = (to[0] - from[0]) / zw.max(1.0) * crop[2];
        let dv = (to[1] - from[1]) / zh.max(1.0) * crop[3];
        // dw/dh are already the oriented display dims (image_display_dims()), so
        // no separate orientation swap is needed here — unlike the shader, which
        // starts from raw sensor dims. The crop term matters: the displayed image
        // is the crop sub-rect stretched to the full target, so under a crop whose
        // aspect differs from the frame's the two UV axes carry different
        // pixel-per-unit scales (see core::viewport::mask_uv_aspect).
        let aspect = crate::core::viewport::mask_uv_aspect(dw, dh, crop);

        let m = &mut self.current_edit_params.masks[index];
        match h {
            MaskHandle::LinearStart => {
                m.ax += du;
                m.ay += dv;
            }
            MaskHandle::LinearEnd => {
                m.bx += du;
                m.by += dv;
            }
            MaskHandle::Center => {
                // Radial: move the center. Linear: move the whole gradient.
                // Translation commutes with rotation, so no rotation-aware math
                // is needed here even for a rotated ellipse.
                m.ax += du;
                m.ay += dv;
                if m.mask_type == 0 {
                    m.bx += du;
                    m.by += dv;
                }
            }
            MaskHandle::RadiusX | MaskHandle::RadiusY => {
                // The handles themselves are drawn rotated with the ellipse
                // (see mask_handle_positions), so dragging must project the
                // screen delta onto the ellipse's own (rotated) local axes —
                // otherwise the handle would visually stop responding to drags
                // once rotated away from the image axes. Same isotropic
                // un-distort/rotate/redistort technique as mask_weight's
                // aspect correction, just for a delta vector instead of a point.
                let rot = m.rotation.to_radians();
                let (rc, rs) = (rot.cos(), rot.sin());
                let iso_du = du * aspect;
                let iso_dv = dv;
                let local_dx = (iso_du * rc + iso_dv * rs) / aspect.max(1e-6);
                let local_dy = -iso_du * rs + iso_dv * rc;
                if h == MaskHandle::RadiusX {
                    m.bx = (m.bx + local_dx).max(0.005);
                } else {
                    m.by = (m.by + local_dy).max(0.005);
                }
            }
            MaskHandle::RadiusUniform => {
                // Grow both radii from the same horizontal delta so a single
                // drag makes a circle by default. UV isn't square-pixel — a
                // radius of `r` physical pixels is `r/width` in u but `r/height`
                // in v — so the v radius must be scaled by width/height to look
                // circular on screen rather than squashed/stretched.
                m.bx = (m.bx + du).max(0.005);
                m.by = (m.by + du * aspect).max(0.005);
            }
            MaskHandle::Rotation => {
                // Angle-based, not delta-based: compute the ellipse's
                // screen-space center via the same shared transform/crop-remap
                // CropOverlay's uv_to_screen uses, then rotate by the change in
                // atan2 angle from center to cursor between the last and
                // current positions. Matches iced's Elliptical.rotation
                // convention (clockwise for positive angles, screen Y down).
                let ib = crate::core::viewport::image_rect(dw, dh, bw, bh, zoom, pan_offset);
                let to_screen = |u: f32, v: f32| {
                    let vu = (u - crop[0]) / crop[2].max(1e-6);
                    let vv = (v - crop[1]) / crop[3].max(1e-6);
                    [ib.x + vu * ib.width, ib.y + vv * ib.height]
                };
                let center = to_screen(m.ax, m.ay);
                let a0 = (from[1] - center[1]).atan2(from[0] - center[0]);
                let a1 = (to[1] - center[1]).atan2(to[0] - center[0]);
                // atan2 returns (-pi, pi], so the raw difference jumps by ~360deg
                // whenever the cursor crosses the -x ray from the center. Wrap the
                // DELTA (not just the sum) or a 0.6deg move reads as -359.4deg.
                let delta_deg = crate::core::viewport::wrap_degrees((a1 - a0).to_degrees());
                // Wrap rather than clamp: angles are cyclic, and clamping detached
                // the handle from the cursor once a drag passed +/-180deg.
                m.rotation = crate::core::viewport::wrap_degrees(m.rotation + delta_deg);
            }
        }
    }
}

/// One frame for a front end that draws straight onto a surface
/// (`gpu::render_functions::render_to_target`) rather than through the
/// read-back render loop: what to draw with, and where.
pub struct SurfaceFrame {
    pub context: Arc<crate::gpu::shared::SharedContext>,
    /// What the display pass draws.
    pub display: Arc<crate::gpu::shared::ImageResources>,
    /// What the overview is rendered from; `None` while the last one is
    /// still on its way back, so at most one is in flight.
    pub overview: Option<Arc<crate::gpu::shared::ImageResources>>,
    /// The visible slice and where it goes on the surface.
    pub placement: Option<([f32; 4], crate::core::viewport::Rect)>,
    pub overview_dims: (u32, u32),
    image_id: i64,
    edit: Option<EditParams>,
    /// The view magnifies a subsampled texture: ask for full resolution.
    soft: bool,
}

/// A surface frame's overview on its way back from the GPU, with what it
/// was rendered from: the proxy it becomes must match that edit, not
/// whatever the user has moved to since.
pub struct SurfaceOverview {
    pending: crate::gpu::render_functions::PendingOverview,
    context: Arc<crate::gpu::shared::SharedContext>,
    image_id: i64,
    edit: Option<EditParams>,
    dims: (u32, u32),
}

impl std::fmt::Debug for SurfaceOverview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceOverview").field("image_id", &self.image_id).field("dims", &self.dims).finish()
    }
}

impl Session {
    /// The frame to draw now on a `surface_w`×`surface_h` surface (physical
    /// pixels), with the current edit pushed to the GPU. `None` until an image
    /// is ready. The caller renders it with `render_to_target` and hands the
    /// result to [`Session::surface_frame_finished`].
    ///
    /// For a front end that renders on vsync from one thread: the push
    /// happens here, with no render in flight, so the uniforms keep one
    /// writer without the read-back loop's `deferred_push`.
    pub fn surface_frame(&mut self, surface_w: u32, surface_h: u32) -> Option<SurfaceFrame> {
        let context = self.gpu_context.clone()?;
        let resources = self.image_resources.clone()?;
        if Some(resources.image_id) != self.selected_image_id {
            return None;
        }
        // The peek draws the unedited photo (as `refresh_for_peek_state`);
        // every frame pushes, so reading only the edit here undid it.
        let params = if self.before_peek {
            self.current_edit_params.before_reference()
        } else {
            self.view_params()
        };
        let (interpolated, wb_override) = self.resolve_wb_and_dcp(&params);
        resources.update_uniforms(&context, &params, interpolated.as_deref(), wb_override);
        let (disp_w, disp_h) = self.image_display_dims();
        let placement = crate::core::viewport::screen_placement(
            disp_w,
            disp_h,
            surface_w as f32,
            surface_h as f32,
            self.zoom,
            self.pan_offset,
        );
        // Surface pixels per source pixel of the open set, as `render_target`
        // has it (the surface is in physical pixels: no scale factor).
        let magnification = placement.map_or(0.0, |(uv, rect)| {
            (rect.width / (uv[2] * disp_w as f32).max(1.0)).max(rect.height / (uv[3] * disp_h as f32).max(1.0))
        });
        let soft = magnification > crate::core::viewport::FULL_RES_MAGNIFICATION;
        // After a full-resolution upgrade the subsampled set renders the
        // overview, and the display too unless the view magnifies it or a
        // noise slider is mid-drag, as in `render`.
        let preview = self.preview_resources.clone().filter(|p| p.image_id == resources.image_id);
        if let Some(preview) = &preview {
            preview.update_uniforms(&context, &params, interpolated.as_deref(), wb_override);
        }
        let display = match &preview {
            Some(preview) => {
                let preview_magnification = magnification * resources.width as f32 / preview.width.max(1) as f32;
                if preview_magnification <= crate::core::viewport::FULL_RES_MAGNIFICATION || self.stage_drag {
                    preview.clone()
                } else {
                    resources.clone()
                }
            }
            None => resources.clone(),
        };
        // Same rule as `render`: only the user's real edit may become the proxy.
        let edit = (!self.before_peek
            && !self.is_cropping
            && self.current_edit_params.mask_overlay == 0
            && self.held_noise.is_none())
        .then_some(self.current_edit_params);
        // One overview in flight at a time: a frame drawn while the last is
        // still on the GPU skips it (and its pass).
        let overview = self.surface_overview.is_none().then(|| preview.unwrap_or_else(|| resources.clone()));
        Some(SurfaceFrame {
            context,
            overview,
            display,
            placement,
            overview_dims: crate::core::viewport::overview_dims(disp_w, disp_h),
            image_id: resources.image_id,
            edit,
            soft,
        })
    }

    /// A [`SurfaceFrame`] was drawn: keep its overview to collect later
    /// ([`Session::surface_overview_poll`]), and ask for full resolution if
    /// the view showed a subsampled texture magnified.
    pub fn surface_frame_finished(
        &mut self,
        frame: &SurfaceFrame,
        out: crate::gpu::render_functions::SurfaceRender,
    ) -> Effects {
        if frame.context.is_lost() {
            return self.gpu_lost();
        }
        if let Some(pending) = out.overview {
            self.surface_overview = Some(SurfaceOverview {
                pending,
                context: frame.context.clone(),
                image_id: frame.image_id,
                edit: frame.edit,
                dims: frame.overview_dims,
            });
        }
        let current = self.image_resources.as_ref().map(|r| r.image_id);
        let mut detail = out.detail;
        detail.finish();
        self.profiler.push_frame(crate::core::profiler::ProfilerFrame {
            update_ms: 0.0,
            upload_ms: out.encode_ms,
            render_ms: 0.0,
            total_ms: out.encode_ms,
            detail,
        });
        self.changed.insert(Changed::PROFILER);
        // A subsampled texture shown magnified: fetch full resolution (once;
        // `trigger_full_res_upgrade` guards repeats), as `render` does.
        if frame.soft && current == Some(frame.image_id) && self.current_tab == AppTab::Develop {
            self.trigger_full_res_upgrade()
        } else {
            Effects::none()
        }
    }

    /// An overview is on its way back: the front end should call
    /// [`Session::surface_overview_poll`] soon, even with nothing to draw.
    pub fn surface_overview_pending(&self) -> bool {
        self.surface_overview.is_some()
    }

    /// Collect the in-flight overview if the GPU has finished it, without
    /// waiting: the histogram, and the edited proxy (as `render_finished`
    /// keeps them for the read-back loop).
    pub fn surface_overview_poll(&mut self) -> Effects {
        let Some(overview) = &mut self.surface_overview else { return Effects::none() };
        let Some(read) = overview.pending.try_read(&overview.context) else { return Effects::none() };
        let SurfaceOverview { image_id, edit, dims, .. } = *overview;
        self.surface_overview = None;
        self.surface_overview_landed(image_id, edit, dims, read)
    }

    /// Wait for the in-flight overview and keep it: for a front end that is
    /// leaving, or measuring.
    pub fn surface_overview_wait(&mut self) -> Effects {
        let Some(SurfaceOverview { pending, context, image_id, edit, dims }) = self.surface_overview.take() else {
            return Effects::none();
        };
        let read = pending.wait(&context);
        self.surface_overview_landed(image_id, edit, dims, read)
    }

    fn surface_overview_landed(
        &mut self,
        image_id: i64,
        edit: Option<EditParams>,
        dims: (u32, u32),
        read: Result<crate::gpu::render_functions::Overview, String>,
    ) -> Effects {
        let overview = match read {
            Ok(overview) => overview,
            Err(e) => {
                tracing::warn!("no overview for image {image_id}: {e}");
                return Effects::none();
            }
        };
        let current = self.image_resources.as_ref().map(|r| r.image_id);
        if current == Some(image_id) {
            *self.histogram_data.borrow_mut() = overview.histogram;
            self.changed.insert(Changed::HISTOGRAM);
        }
        let awaited = self.proxy_awaiting.contains_key(&image_id);
        if let Some(params) = edit.filter(|_| current == Some(image_id) || awaited) {
            self.capture_proxy(image_id, overview.pixels, dims, params);
        }
        self.resolve_awaiting_proxy(image_id, edit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tonemap::{GtParams, ToneMapper};

    fn tone(curve: f32) -> ToneSettings {
        ToneSettings { mapper: ToneMapper::Camera, profile_curve: curve, gt: GtParams::default() }
    }

    #[test]
    fn dcp_memo_hits_on_identical_state() {
        assert!(dcp_memo_is_valid(5500.0, tone(0.33), 5500.0, tone(0.33)));
    }

    #[test]
    fn dcp_memo_misses_when_temperature_moves() {
        // One Kelvin is far below what any slider step produces, and must
        // still invalidate — the memo must never coarsen the temperature.
        assert!(!dcp_memo_is_valid(5500.0, tone(0.33), 5501.0, tone(0.33)));
    }

    #[test]
    fn dcp_memo_misses_when_profile_curve_moves() {
        assert!(!dcp_memo_is_valid(5500.0, tone(0.33), 5500.0, tone(0.34)));
    }

    #[test]
    fn dcp_memo_absorbs_kelvin_noise_only() {
        // Recomputing solve_wb from identical inputs is deterministic, so the
        // epsilon exists purely to tolerate last-bit drift, not real changes.
        assert!(dcp_memo_is_valid(5500.0, tone(0.33), 5500.0 + 1e-4, tone(0.33)));
    }

    /// The memo caches a baked tone LUT, so switching operators MUST miss —
    /// otherwise the new selection would not reach the GPU until something
    /// else happened to move the temperature.
    #[test]
    fn dcp_memo_misses_when_the_tone_operator_changes() {
        let mut gt = tone(0.33);
        gt.mapper = ToneMapper::Gt;
        assert!(!dcp_memo_is_valid(5500.0, tone(0.33), 5500.0, gt));
    }

    /// Same for the GT shape sliders — they re-bake the curve, and nothing
    /// else in the key moves when the user drags one.
    #[test]
    fn dcp_memo_misses_when_gt_shape_changes() {
        let mut a = tone(0.33);
        a.mapper = ToneMapper::Gt;
        let mut b = a;
        b.gt.contrast = 1.4;
        assert!(!dcp_memo_is_valid(5500.0, a, 5500.0, b));
    }
}

/// Zoom, pan and the handle drags, in viewport pixels. They take the image's
/// shape from its GPU resources, so these need a GPU (lavapipe is enough) and
/// are `#[ignore]`d like the rest: `cargo test -p raw-editor-session
/// view_tests -- --ignored --test-threads=1`.
#[cfg(test)]
mod view_tests {
    use super::*;
    use crate::core::types::MaskParams;
    use crate::core::viewport::{fitted_size, image_rect};
    use crate::gpu::shared::{ImageResources, SharedContext};
    use std::sync::Arc;

    const VIEWPORT: (f32, f32) = (800.0, 600.0);

    /// A 300×200 image open and ready in Develop on an 800×600 viewport, so
    /// it fits as 800×533⅓ at zoom 1.
    fn session() -> Session {
        let ctx = Arc::new(block_on(SharedContext::new()).expect("gpu"));
        let (w, h) = (300, 200);
        let res = ImageResources::new(
            &ctx, 1, &vec![2000u16; (w * h) as usize], w, h, &EditParams::default(), [2.0, 1.0, 1.5, 1.0],
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], 0, [0; 4], 4095, None, None, 1, Default::default(),
        )
        .expect("resources");
        let mut s = crate::session::testkit::session_with_images(1);
        s.current_tab = AppTab::Develop;
        s.selected_image_id = Some(1);
        s.editor_readiness = EditorReadiness::Ready(1);
        s.gpu_context = Some(ctx);
        s.image_resources = Some(Arc::new(res));
        s.viewport_size = VIEWPORT;
        s.scale_factor = 1.0;
        s
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    fn fitted(s: &Session) -> (f32, f32) {
        let (dw, dh) = s.image_display_dims();
        fitted_size(dw, dh, VIEWPORT.0, VIEWPORT.1)
    }

    /// The fraction of the image under screen point `at`.
    fn under(s: &Session, at: [f32; 2]) -> [f32; 2] {
        let (dw, dh) = s.image_display_dims();
        let r = image_rect(dw, dh, VIEWPORT.0, VIEWPORT.1, s.zoom, s.pan_offset);
        [(at[0] - r.x) / r.width, (at[1] - r.y) / r.height]
    }

    #[test]
    #[ignore]
    fn zooming_keeps_the_point_under_the_cursor_still() {
        let mut s = session();
        let at = [620.0, 210.0];
        let before = under(&s, at);
        for d in [1.0, 1.0, -0.5, 2.0] {
            s.is_rendering = false;
            let _ = s.zoom_at(d, at);
            let now = under(&s, at);
            assert!(
                (now[0] - before[0]).abs() < 1e-4 && (now[1] - before[1]).abs() < 1e-4,
                "drifted to {now:?} from {before:?} at zoom {}",
                s.zoom
            );
        }
        assert!(s.zoom > 1.0);
    }

    #[test]
    #[ignore]
    fn a_pinch_keeps_the_point_under_the_fingers() {
        let mut s = session();
        let at = [500.0, 260.0];
        let before = under(&s, at);
        for scale in [1.5, 1.3, 0.9] {
            s.transform_view(scale, at, [0.0, 0.0]);
            let now = under(&s, at);
            assert!(
                (now[0] - before[0]).abs() < 1e-4 && (now[1] - before[1]).abs() < 1e-4,
                "drifted to {now:?} from {before:?} at zoom {}",
                s.zoom
            );
        }
        // A drag moves that point with the fingers.
        s.transform_view(1.0, at, [-30.0, 12.0]);
        let moved = under(&s, [at[0] - 30.0, at[1] + 12.0]);
        assert!((moved[0] - before[0]).abs() < 1e-4 && (moved[1] - before[1]).abs() < 1e-4, "{moved:?}");
        // Pinching in past fit stops at fit, centred.
        s.transform_view(0.01, at, [0.0, 0.0]);
        assert_eq!((s.zoom, s.pan_offset.x, s.pan_offset.y), (1.0, 0.0, 0.0));
        assert!(s.changed.contains(Changed::FRAME));
    }

    /// The crop tool inset from the screen's edges: the whole frame fits
    /// inside the insets, centred in what they leave, and a corner drag
    /// still moves the crop by exactly the finger's distance on screen.
    #[test]
    #[ignore]
    fn an_inset_crop_view_fits_inside_the_insets_and_drags_follow_the_finger() {
        let mut s = session();
        let _ = s.toggle_crop();
        let insets = [40.0, 100.0, 40.0, 20.0];
        s.inset_view(insets);
        let (dw, dh) = s.image_display_dims();
        let r = image_rect(dw, dh, VIEWPORT.0, VIEWPORT.1, s.zoom, s.pan_offset);
        let eps = 1e-3;
        assert!(r.x >= insets[0] - eps && r.y >= insets[1] - eps, "{r:?}");
        assert!(r.x + r.width <= VIEWPORT.0 - insets[2] + eps, "{r:?}");
        assert!(r.y + r.height <= VIEWPORT.1 - insets[3] + eps, "{r:?}");
        let (cx, cy) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
        assert!((cx - 400.0).abs() < eps && (cy - (100.0 + 480.0 / 2.0)).abs() < eps, "not centred: {cx}, {cy}");
        assert!(s.changed.contains(Changed::FRAME));

        let corner = [r.x + r.width, r.y + r.height];
        s.drag_crop(CropHandle::BottomRight, corner, [corner[0] - 30.0, corner[1] - 12.0]);
        let c = s.current_edit_params.crop;
        assert!((c[2] - (1.0 - 30.0 / r.width)).abs() < 1e-4, "{c:?}");
        assert!((c[3] - (1.0 - 12.0 / r.height)).abs() < 1e-4, "{c:?}");

        // Insets wider than fit never enlarge it.
        s.inset_view([0.0; 4]);
        assert_eq!((s.zoom, s.pan_offset.x, s.pan_offset.y), (1.0, 0.0, 0.0));
    }

    /// A front end drawing on its own clock gets a FRAME signal where the
    /// session would otherwise start a read-back render, which would fight
    /// its frames for the image's uniforms.
    #[test]
    #[ignore]
    fn a_surface_drawn_session_starts_no_read_back_render() {
        let mut s = session();
        s.surface_drawn = true;
        let fx = s.push_uniforms_and_render(&EditParams::default());
        assert!(!fx.labels().any(|l| l == "render"), "{fx:?}");
        assert!(!s.is_rendering);
        assert!(s.changed.contains(Changed::FRAME));
    }

    /// A surface frame never waits for its overview: the next frame skips
    /// the overview while one is in flight, and it lands later as the
    /// histogram.
    #[test]
    #[ignore]
    fn a_surface_overview_lands_a_frame_later_one_at_a_time() {
        let mut s = session();
        let draw = |s: &mut Session| {
            let frame = s.surface_frame(400, 300).expect("a frame");
            let texture = frame.context.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 400, height: 300, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let target = crate::gpu::render_functions::DisplayTarget {
                view: &view,
                format: wgpu::TextureFormat::Rgba8Unorm,
                size: (400, 300),
                placement: frame.placement,
                background: wgpu::Color::BLACK,
                headroom: 1.0,
            };
            let asked = frame.overview.is_some();
            let out = crate::gpu::render_functions::render_to_target(
                &frame.context,
                &frame.display,
                frame.overview.as_deref(),
                target,
                frame.overview_dims,
                Default::default(),
                || {},
            )
            .expect("render");
            let _ = s.surface_frame_finished(&frame, out);
            asked
        };
        *s.histogram_data.borrow_mut() = [[0; 256]; 3];
        assert!(draw(&mut s), "the first frame asks for an overview");
        assert!(s.surface_overview_pending());
        assert!(!draw(&mut s), "a frame with one in flight skips it");
        let _ = s.surface_overview_wait();
        assert!(!s.surface_overview_pending());
        let counted: u32 = s.histogram_data.borrow()[1].iter().sum();
        assert!(counted > 0, "the overview landed as the histogram");
        assert!(draw(&mut s), "and the next frame asks again");
    }

    #[test]
    #[ignore]
    fn zoom_is_bounded_and_frozen_while_cropping() {
        let mut s = session();
        for _ in 0..40 {
            s.is_rendering = false;
            let _ = s.zoom_at(1.0, [400.0, 300.0]);
        }
        assert_eq!(s.zoom, 10.0);
        for _ in 0..80 {
            s.is_rendering = false;
            let _ = s.zoom_at(-1.0, [400.0, 300.0]);
        }
        assert_eq!(s.zoom, 0.1);

        let mut s = session();
        s.is_cropping = true;
        assert!(s.zoom_at(1.0, [400.0, 300.0]).is_empty());
        assert!(s.pan([0.1, 0.0]).is_empty());
        assert_eq!((s.zoom, s.pan_offset.x), (1.0, 0.0));
    }

    /// Panning re-projects the rendered window for free, and asks for a new
    /// render only once the view runs past its overscan margin.
    #[test]
    #[ignore]
    fn panning_renders_only_past_the_rendered_margin() {
        let mut s = session();
        let _ = s.zoom_at(2.0, [400.0, 300.0]);
        assert!(s.is_rendering, "zooming did not render");
        s.is_rendering = false; // that render finished

        let step = s.pan_fraction([8.0, 0.0]);
        let (fw, _) = fitted(&s);
        assert!((step[0] - 8.0 / fw).abs() < 1e-6, "a pixel of drag is not a pixel of pan");
        assert!(s.pan(step).is_empty(), "a small pan re-rendered");
        assert!(!s.is_rendering);

        let _ = s.pan(s.pan_fraction([-300.0, 0.0]));
        assert!(s.is_rendering, "panned off the rendered window without a render");
    }

    #[test]
    #[ignore]
    fn reset_view_goes_back_to_fit_and_renders() {
        let mut s = session();
        let _ = s.zoom_at(2.0, [100.0, 100.0]);
        s.is_rendering = false;
        let _ = s.reset_view();
        assert_eq!((s.zoom, s.pan_offset.x, s.pan_offset.y), (1.0, 0.0, 0.0));
        assert!(s.is_rendering);
    }

    /// Crop handles move by the dragged fraction of the shown image, never
    /// invert or leave it, and the body moves the crop whole.
    #[test]
    #[ignore]
    fn crop_handles_follow_the_drag_and_stay_on_the_image() {
        let mut s = session();
        s.is_cropping = true;
        let (fw, fh) = fitted(&s);
        s.drag_crop(CropHandle::BottomRight, [0.0, 0.0], [-fw * 0.25, -fh * 0.5]);
        let c = s.current_edit_params.crop;
        assert!((c[2] - 0.75).abs() < 1e-5 && (c[3] - 0.5).abs() < 1e-5, "{c:?}");
        assert!(s.edits_dirty, "the drag would not be saved");

        s.drag_crop(CropHandle::TopLeft, [0.0, 0.0], [fw * 5.0, fh * 5.0]);
        let c = s.current_edit_params.crop;
        assert!(c[2] >= 0.0099 && c[3] >= 0.0099, "the crop collapsed or inverted: {c:?}");

        let mut s = session();
        s.is_cropping = true;
        s.current_edit_params.crop = [0.25, 0.25, 0.5, 0.5];
        s.drag_crop(CropHandle::Body, [0.0, 0.0], [fw, -fh]);
        let c = s.current_edit_params.crop;
        assert_eq!(c, [0.5, 0.0, 0.5, 0.5], "the body left the image or changed size");
    }

    #[test]
    #[ignore]
    fn mask_handles_move_their_own_points() {
        let mut s = session();
        let (fw, fh) = fitted(&s);
        s.current_edit_params.masks[0] = MaskParams { mask_type: 0, ax: 0.2, ay: 0.2, bx: 0.6, by: 0.6, ..MaskParams::default() };
        s.current_edit_params.mask_count = 1;
        s.selected_mask = Some(0);

        s.move_mask_handle(MaskHandle::Center, [0.0, 0.0], [fw * 0.1, fh * 0.1]);
        let m = s.current_edit_params.masks[0];
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(close(m.ax, 0.3) && close(m.ay, 0.3) && close(m.bx, 0.7) && close(m.by, 0.7), "{m:?}");

        s.move_mask_handle(MaskHandle::LinearEnd, [0.0, 0.0], [-fw * 0.2, 0.0]);
        let m = s.current_edit_params.masks[0];
        assert!(close(m.ax, 0.3) && close(m.bx, 0.5), "the end handle moved the start: {m:?}");

        // Radial: a radius never reaches zero, and the uniform handle makes a
        // circle on screen (equal pixels on both axes).
        s.current_edit_params.masks[0] = MaskParams { mask_type: 1, ax: 0.5, ay: 0.5, bx: 0.1, by: 0.1, ..MaskParams::default() };
        s.move_mask_handle(MaskHandle::RadiusX, [0.0, 0.0], [-fw, 0.0]);
        assert_eq!(s.current_edit_params.masks[0].bx, 0.005);
        s.current_edit_params.masks[0].bx = 0.1;
        s.current_edit_params.masks[0].by = 0.1 * 1.5;
        s.move_mask_handle(MaskHandle::RadiusUniform, [0.0, 0.0], [fw * 0.1, 0.0]);
        let m = s.current_edit_params.masks[0];
        let (dw, dh) = s.image_display_dims();
        assert!(close(m.bx * dw as f32, m.by * dh as f32), "not a circle: {m:?}");

        // No mask selected, or a slot past the count: nothing moves.
        s.selected_mask = Some(3);
        let before = s.current_edit_params;
        s.move_mask_handle(MaskHandle::Center, [0.0, 0.0], [50.0, 50.0]);
        assert_eq!(s.current_edit_params, before);
    }
}
