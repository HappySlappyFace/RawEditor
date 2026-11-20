//! Develop: what the render needs from the open image (its white balance and
//! DCP interpolation, the render target for the current view).

use super::Session;
use crate::core::tonemap::ToneSettings;

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
