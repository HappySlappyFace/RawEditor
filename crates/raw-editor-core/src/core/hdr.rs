//! HDR: the SDR rendering, brightened where the scene was bright.
//!
//! An HDR display can show light above SDR white: `headroom` is how far
//! (2.0 = one stop brighter than white). The colour pass renders exactly
//! as for SDR — every edit, the tone curve, the gamut map, the point curves
//! — and then multiplies the result by a GAIN chosen from the pixel's
//! scene-linear level at the tone stage, before any curve compressed it:
//!
//! - up to one stop above middle grey the gain is 1, so shadows and
//!   midtones are the SDR image, unchanged;
//! - from there it rises smoothly (in stops) to the full headroom, reached
//!   four stops further up.
//!
//! A gain is a multiply, so hue and saturation are the SDR rendering's; the
//! HDR view cannot drift from it, it can only be brighter. And the gain is
//! exactly what an Ultra HDR file stores beside its SDR image (the gain
//! map), so an export reproduces the screen.
//!
//! The colour pass (`fs_main_hdr`) mirrors [`gain`]; its constants are
//! guarded by `shader_hdr_constants_match_rust`.

/// Middle grey, scene-linear: where the ramp's stops are counted from.
pub const HDR_GREY: f32 = 0.18;
/// Stops above grey where the gain starts to rise.
pub const HDR_RAMP_START_EV: f32 = 1.0;
/// Stops above grey where it reaches the full headroom.
pub const HDR_RAMP_END_EV: f32 = 5.0;

/// The headroom an Ultra HDR export is made for (its gain map's maximum):
/// the most the phone app shows, two stops above SDR white. A display with
/// less applies part of it.
pub const EXPORT_HEADROOM: f32 = 4.0;

/// A gain as an Ultra HDR gain map stores it: `log2(gain) / log2(headroom)`,
/// 0 for none, 1 for the full headroom (the colour pass's `fs_main_gain`).
pub fn encode_gain(gain: f32, headroom: f32) -> f32 {
    if headroom <= 1.0 {
        return 0.0;
    }
    (gain.max(1.0).log2() / headroom.log2()).clamp(0.0, 1.0)
}

/// The gain for a pixel whose scene-linear level (peak channel at the tone
/// stage) is `level`, on a display with `headroom` (≥ 1; 1 is SDR, and
/// gives 1 everywhere).
pub fn gain(level: f32, headroom: f32) -> f32 {
    let headroom = headroom.max(1.0);
    let ev = (level.max(1e-6) / HDR_GREY).log2();
    1.0 + (headroom - 1.0) * smoothstep(HDR_RAMP_START_EV, HDR_RAMP_END_EV, ev)
}

/// WGSL's `smoothstep`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdr_headroom_changes_nothing() {
        for level in [0.0, 0.01, 0.18, 1.0, 10.0, 1000.0] {
            assert_eq!(gain(level, 1.0), 1.0);
            // Below 1 is not a display; treated as SDR.
            assert_eq!(gain(level, 0.5), 1.0);
        }
    }

    #[test]
    fn shadows_and_midtones_keep_the_sdr_rendering() {
        let at_ramp_start = HDR_GREY * HDR_RAMP_START_EV.exp2();
        for level in [0.0, 0.001, 0.05, HDR_GREY, at_ramp_start] {
            assert_eq!(gain(level, 4.0), 1.0, "level {level}");
        }
    }

    #[test]
    fn highlights_reach_the_full_headroom_and_no_further() {
        let at_ramp_end = HDR_GREY * HDR_RAMP_END_EV.exp2();
        assert_eq!(gain(at_ramp_end, 4.0), 4.0);
        assert_eq!(gain(at_ramp_end * 100.0, 4.0), 4.0);
    }

    #[test]
    fn encoded_gain_spans_none_to_the_full_headroom() {
        assert_eq!(encode_gain(1.0, 4.0), 0.0);
        assert_eq!(encode_gain(2.0, 4.0), 0.5);
        assert_eq!(encode_gain(4.0, 4.0), 1.0);
        assert_eq!(encode_gain(3.0, 1.0), 0.0);
    }

    #[test]
    fn gain_never_falls_as_the_scene_brightens() {
        let mut last = 0.0;
        for i in 0..=400 {
            let level = HDR_GREY * ((i as f32) / 40.0 - 2.0).exp2();
            let g = gain(level, 5.0);
            assert!(g >= last, "gain fell at level {level}: {last} → {g}");
            last = g;
        }
    }
}
