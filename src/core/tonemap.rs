//! Selectable tone rendering: scene-linear → display-linear.
//!
//! Tone mapping is the stage that decides how the open-ended scene-linear
//! signal (which routinely exceeds 1.0 after white balance and highlight
//! reconstruction) is squeezed into the [0,1] a display can show. It is
//! deliberately SEPARATE from colour rendering: a DCP characterises the
//! *sensor* (ForwardMatrix + HueSatMap) and that stays in force whichever
//! operator is chosen here — only the profile's own `ProfileToneCurve` is
//! what a non-`Camera` selection replaces.
//!
//! ## Why these live on the CPU
//!
//! Every operator below is a scalar function of one channel, so the whole set
//! bakes into the 1D curve texture the DCP path already uploads and samples
//! (`sample_tone_curve` in `gpu/shaders.rs`). That buys three things:
//!
//!  * **Zero per-pixel cost.** Switching operators changes texture contents,
//!    not shader work. There is no branch in the hot path.
//!  * **They are testable.** Shader code is not; these are ordinary Rust
//!    functions with unit tests below.
//!  * **Hue-preserving application for free.** The shader applies the LUT
//!    through Adobe's RGBTone (`dcp_rgb_tone`), which curves the max and min
//!    channels and interpolates the middle. That is strictly better than the
//!    naive per-channel application these operators normally get in games,
//!    which widens channel spread and oversaturates.
//!
//! Operators that are NOT scalar — Stephen Hill's ACES fit, AgX — apply 3×3
//! matrices around the curve to deliberately shift chroma, so they cannot be
//! baked this way and are not offered here.
//!
//! ## Output space
//!
//! Every function in this module takes **scene-linear** and returns
//! **display-linear** (i.e. still linear, pre-sRGB-encode) in [0,1]. The
//! shader applies the sRGB transfer function at the very end. This is the
//! trap that produced a ~1.3 EV overbright once already: Adobe's
//! `ProfileToneCurve` is the odd one out, mapping linear input to
//! *gamma-encoded* output, so `bake` linearises it back — see
//! `ToneMapper::Camera`'s arm.

use serde::{Deserialize, Serialize};

/// Top of the scene-linear domain the baked LUT covers, in stops above 1.0.
///
/// 16.0 is four stops of headroom over diffuse white, which comfortably
/// covers what survives white balance amplification plus Step 3's highlight
/// reconstruction. Values beyond it clamp to the LUT's last entry.
///
/// **Mirrored in `gpu/shaders.rs` as `TONE_LUT_MAX`** — the test
/// `shader_tone_lut_constants_match_rust` asserts the two agree.
pub const TONE_LUT_MAX: f32 = 16.0;

/// Samples in the baked curve texture.
///
/// Larger than the 1024 this used to be because the domain is now ~16× wider.
/// The log encoding below spends them unevenly and in our favour: ~1000 land
/// below 1.0 (about what the old uniform table gave the same range) and ~586
/// below 0.5, where tone curves are steepest and banding would show first.
/// 4096 × 2 bytes = 8 KB, and stays well inside wgpu's default
/// `max_texture_dimension_1d` of 8192.
pub const TONE_LUT_SIZE: usize = 4096;

/// Scene-linear → LUT coordinate in [0,1].
///
/// Log rather than linear so the table's resolution follows where the eye
/// (and the curve's curvature) actually is. Monotonic, `encode(0) == 0`,
/// `encode(TONE_LUT_MAX) == 1`.
pub fn tone_lut_encode(x: f32) -> f32 {
    (1.0 + x.max(0.0)).log2() / (1.0 + TONE_LUT_MAX).log2()
}

/// Inverse of [`tone_lut_encode`]. Used by the bake to find which
/// scene-linear value each LUT entry stands for.
pub fn tone_lut_decode(t: f32) -> f32 {
    ((1.0 + TONE_LUT_MAX).log2() * t.clamp(0.0, 1.0)).exp2() - 1.0
}

/// Which tone rendering the user selected.
///
/// Serialised by name, so adding or reordering variants cannot silently
/// re-map anyone's saved edits.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToneMapper {
    /// The camera's own rendering: the DCP `ProfileToneCurve` at
    /// `profile_curve` strength, or — for profiles that embed no curve, and
    /// for images with no profile at all — the built-in filmic S-curve.
    ///
    /// The default, and the only variant whose output is calibrated against
    /// real in-camera JPEGs. Everything else is a look, not a match.
    #[default]
    Camera,
    /// The built-in filmic S-curve (midtone lift + Reinhard shoulder),
    /// forced on even when the profile has its own curve.
    Filmic,
    /// Extended Reinhard, normalised so `TONE_LUT_MAX` maps exactly to 1.0.
    ///
    /// Included as a reference point, not a recommendation: it is famously
    /// flat through the midtones, which is the whole reason filmic curves
    /// exist. Useful for judging what the other operators are doing.
    Reinhard,
    /// Hable's "Uncharted 2" filmic curve, at its published constants.
    Hable,
    /// The ACES curve as everyone actually ships it — Krzysztof Narkowicz's
    /// rational fit to the RRT+ODT.
    ///
    /// Note this is NOT the full ACES transform. The real RRT carries a glow
    /// module, a red modifier, and hue-dependent behaviour (the "notorious
    /// six") that cannot be expressed as a scalar curve, and which is not
    /// especially kind to skin tones in a photo editor. This fit is the
    /// tone response only.
    AcesFitted,
    /// Hajime Uchimura's Gran Turismo curve (CEDEC 2017).
    ///
    /// The most interesting one here for photography: with `contrast` at 1.0
    /// its midsection is exactly linear, so it shapes the toe and the
    /// shoulder while leaving middle grey — and therefore the profile's
    /// calibration and the exposure slider — untouched. Its shape is exposed
    /// through [`GtParams`].
    Gt,
}

impl ToneMapper {
    /// Every variant, in UI order.
    pub const ALL: [ToneMapper; 6] = [
        ToneMapper::Camera,
        ToneMapper::Filmic,
        ToneMapper::Reinhard,
        ToneMapper::Hable,
        ToneMapper::AcesFitted,
        ToneMapper::Gt,
    ];

    /// Short label for the selector buttons.
    pub fn label(self) -> &'static str {
        match self {
            ToneMapper::Camera => "Camera",
            ToneMapper::Filmic => "Filmic",
            ToneMapper::Reinhard => "Reinhard",
            ToneMapper::Hable => "Hable",
            ToneMapper::AcesFitted => "ACES",
            ToneMapper::Gt => "GT",
        }
    }

    /// Stable id handed to the shader in `GpuEditParams::tone_mapper`.
    ///
    /// The shader only needs to distinguish `Camera` (0) from everything
    /// else — the operator's actual shape reaches it through the baked LUT,
    /// not through this value. See `GpuEditParams::tone_mapper`.
    pub fn shader_id(self) -> u32 {
        match self {
            ToneMapper::Camera => 0,
            ToneMapper::Filmic => 1,
            ToneMapper::Reinhard => 2,
            ToneMapper::Hable => 3,
            ToneMapper::AcesFitted => 4,
            ToneMapper::Gt => 5,
        }
    }

    /// True when this operator is baked into the curve LUT and applied by the
    /// shader's tone-curve stage.
    ///
    /// `Camera` is the exception in both directions: with a profile curve it
    /// bakes one (from the profile, not from this module), and without one it
    /// falls through to the shader's own filmic block instead.
    pub fn is_baked_operator(self) -> bool {
        self != ToneMapper::Camera
    }
}

impl std::fmt::Display for ToneMapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Shape parameters for [`ToneMapper::Gt`], following Uchimura's notation.
///
/// `P` (max brightness) is pinned at 1.0 and `b` (black offset) at 0.0: this
/// is an SDR pipeline that must land on a display white of exactly 1.0, and a
/// nonzero black offset would lift true black off zero, which is the job of
/// the Blacks slider rather than the tone curve.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct GtParams {
    /// `a` — slope of the linear midsection. 1.0 keeps middle grey exactly
    /// where the profile and the exposure slider put it; above 1.0 trades
    /// that calibration for punch.
    pub contrast: f32,
    /// `m` — scene-linear value where the toe ends and the linear section
    /// begins.
    pub linear_start: f32,
    /// `l` — how much of the remaining range stays linear before the
    /// shoulder takes over. 0 rolls off immediately; 1 stays linear to white.
    pub linear_length: f32,
    /// `c` — toe curvature. 1.0 is a straight toe into black; higher values
    /// hold the shadows down harder before releasing them.
    pub black_tightness: f32,
}

impl Default for GtParams {
    /// Uchimura's published defaults.
    fn default() -> Self {
        Self {
            contrast: 1.0,
            linear_start: 0.22,
            linear_length: 0.4,
            black_tightness: 1.33,
        }
    }
}

impl GtParams {
    /// Clamp to the ranges the sliders offer. `linear_start` must stay
    /// strictly positive — it is a divisor in the toe term.
    pub fn sanitised(self) -> Self {
        Self {
            contrast: self.contrast.clamp(0.1, 3.0),
            linear_start: self.linear_start.clamp(0.01, 0.9),
            linear_length: self.linear_length.clamp(0.0, 1.0),
            black_tightness: self.black_tightness.clamp(0.5, 4.0),
        }
    }
}

/// Everything the baked tone LUT depends on.
///
/// Grouped into one `PartialEq` value so the two dirty-checks that gate
/// re-baking and re-uploading (`develop::resolve_wb_and_dcp`'s memo and
/// `ImageResources::update_uniforms`) can compare it wholesale instead of
/// enumerating fields and eventually forgetting one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneSettings {
    pub mapper: ToneMapper,
    /// Only meaningful for [`ToneMapper::Camera`]; carried regardless so the
    /// key changes when the user moves the slider back and forth across a
    /// mapper switch.
    pub profile_curve: f32,
    pub gt: GtParams,
}

impl ToneSettings {
    pub fn from_params(params: &crate::core::types::EditParams) -> Self {
        Self {
            mapper: params.tone_mapper,
            profile_curve: params.profile_curve,
            gt: params.gt.sanitised(),
        }
    }
}

// ── The operators ────────────────────────────────────────────────────────────
//
// All take scene-linear, return display-linear in [0,1].

fn smoothstep01(x: f32, e0: f32, e1: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Extended Reinhard with the white point at [`TONE_LUT_MAX`], so the top of
/// the LUT domain maps exactly to display white.
pub fn reinhard(x: f32) -> f32 {
    let x = x.max(0.0);
    let w2 = TONE_LUT_MAX * TONE_LUT_MAX;
    (x * (1.0 + x / w2) / (1.0 + x)).clamp(0.0, 1.0)
}

fn hable_partial(x: f32) -> f32 {
    const A: f32 = 0.15;
    const B: f32 = 0.50;
    const C: f32 = 0.10;
    const D: f32 = 0.20;
    const E: f32 = 0.02;
    const F: f32 = 0.30;
    ((x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F)) - E / F
}

/// Hable's Uncharted 2 filmic curve, including the exposure bias of 2.0 and
/// the linear white point of 11.2 he published with it.
pub fn hable(x: f32) -> f32 {
    const LINEAR_WHITE: f32 = 11.2;
    const EXPOSURE_BIAS: f32 = 2.0;
    let num = hable_partial(x.max(0.0) * EXPOSURE_BIAS);
    let den = hable_partial(LINEAR_WHITE);
    (num / den).clamp(0.0, 1.0)
}

/// Narkowicz's rational fit to the ACES RRT + sRGB ODT.
pub fn aces_fitted(x: f32) -> f32 {
    const A: f32 = 2.51;
    const B: f32 = 0.03;
    const C: f32 = 2.43;
    const D: f32 = 0.59;
    const E: f32 = 0.14;
    let x = x.max(0.0);
    ((x * (A * x + B)) / (x * (C * x + D) + E)).clamp(0.0, 1.0)
}

/// Uchimura's Gran Turismo curve: a toe, a straight midsection, and an
/// exponential shoulder, blended by weights that make the joins C1-continuous.
pub fn uchimura(x: f32, gt: GtParams) -> f32 {
    let GtParams { contrast: a, linear_start: m, linear_length: l, black_tightness: c } =
        gt.sanitised();
    // P is pinned at display white; see GtParams.
    const P: f32 = 1.0;

    let x = x.max(0.0);
    // Length of the linear section, and where it ends.
    let l0 = ((P - m) * l) / a;
    let s0 = m + l0;
    let s1 = m + a * l0;
    // Shoulder decay chosen so the shoulder meets the linear section with
    // matching slope.
    let cc = (a * P) / (P - s1).max(1e-6);

    // Region weights: toe below m, shoulder above s0, linear in between.
    let w0 = 1.0 - smoothstep01(x, 0.0, m);
    let w2 = if x < s0 { 0.0 } else { 1.0 };
    let w1 = 1.0 - w0 - w2;

    let toe = m * (x / m).powf(c);
    let shoulder = P - (P - s1) * (-cc * (x - s0) / P).exp();
    let linear = m + a * (x - m);

    (toe * w0 + linear * w1 + shoulder * w2).clamp(0.0, 1.0)
}

/// Dispatch: scene-linear → display-linear for any operator that this module
/// owns.
///
/// [`ToneMapper::Camera`] is absent by construction — its curve comes from the
/// DCP profile (or from the shader's filmic block), not from here. Callers
/// must check [`ToneMapper::is_baked_operator`] first; a `Camera` argument
/// falls back to identity rather than panicking, so a missed check shows up as
/// "no tone curve" rather than a crash mid-render.
pub fn apply(mapper: ToneMapper, x: f32, gt: GtParams) -> f32 {
    match mapper {
        ToneMapper::Camera => x.clamp(0.0, 1.0),
        ToneMapper::Filmic => filmic(x),
        ToneMapper::Reinhard => reinhard(x),
        ToneMapper::Hable => hable(x),
        ToneMapper::AcesFitted => aces_fitted(x),
        ToneMapper::Gt => uchimura(x, gt),
    }
}

/// The built-in filmic curve, matching the shader's Step 15 block exactly:
/// a midtone lift of 0.08 followed by a Reinhard shoulder from 0.65.
///
/// Duplicated here rather than shared because the shader applies it
/// per-pixel-vector on the fallback path while this bakes it into the LUT;
/// the two must produce the same numbers, which `filmic_matches_shader_form`
/// pins down.
pub fn filmic(x: f32) -> f32 {
    const TONE_LIFT: f32 = 0.08;
    const THRESHOLD: f32 = 0.65;
    const HEADROOM: f32 = 1.0 - THRESHOLD;
    let x = x.max(0.0);
    let lifted = x * ((1.0 + TONE_LIFT) - TONE_LIFT * x);
    if lifted <= THRESHOLD {
        lifted.clamp(0.0, 1.0)
    } else {
        let over = lifted - THRESHOLD;
        (THRESHOLD + (HEADROOM * over) / (over + HEADROOM)).clamp(0.0, 1.0)
    }
}

/// Bake one of this module's operators into the curve LUT.
///
/// Entry `i` holds the operator evaluated at the scene-linear value that LUT
/// coordinate `i / (N-1)` stands for — see [`tone_lut_decode`]. The result is
/// display-linear, which is what the shader's tone-curve stage expects.
pub fn bake(mapper: ToneMapper, gt: GtParams) -> Vec<f32> {
    (0..TONE_LUT_SIZE)
        .map(|i| {
            let t = i as f32 / (TONE_LUT_SIZE - 1) as f32;
            apply(mapper, tone_lut_decode(t), gt)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPERATORS: [ToneMapper; 4] = [
        ToneMapper::Reinhard,
        ToneMapper::Hable,
        ToneMapper::AcesFitted,
        ToneMapper::Gt,
    ];

    #[test]
    fn encode_decode_round_trips() {
        for x in [0.0f32, 0.001, 0.18, 0.5, 1.0, 4.0, TONE_LUT_MAX] {
            let back = tone_lut_decode(tone_lut_encode(x));
            assert!((back - x).abs() < 1e-3, "{x} -> {back}");
        }
    }

    #[test]
    fn encode_spans_the_unit_interval_over_the_domain() {
        assert!(tone_lut_encode(0.0).abs() < 1e-6);
        assert!((tone_lut_encode(TONE_LUT_MAX) - 1.0).abs() < 1e-6);
    }

    /// The whole point of the log encoding: more table below middle grey than
    /// a uniform [0, TONE_LUT_MAX] ramp would give, which would spend 94% of
    /// its entries on highlights nobody looks at.
    #[test]
    fn encoding_favours_the_shadows() {
        let below_one = tone_lut_encode(1.0);
        assert!(
            below_one > 0.2,
            "only {:.1}% of the table covers [0,1]",
            below_one * 100.0
        );
        // A uniform ramp would have given 1/16 = 6.25%.
        assert!(below_one > 4.0 * (1.0 / (1.0 + TONE_LUT_MAX)));
    }

    #[test]
    fn every_operator_maps_black_to_black() {
        for m in OPERATORS {
            assert!(
                apply(m, 0.0, GtParams::default()).abs() < 1e-5,
                "{m} lifted black"
            );
        }
        assert!(filmic(0.0).abs() < 1e-6);
    }

    #[test]
    fn every_operator_is_monotonic_and_bounded() {
        for m in OPERATORS {
            let mut prev = -1.0f32;
            for i in 0..=512 {
                let x = tone_lut_decode(i as f32 / 512.0);
                let y = apply(m, x, GtParams::default());
                assert!((0.0..=1.0).contains(&y), "{m} out of range at {x}: {y}");
                assert!(y >= prev - 1e-5, "{m} not monotonic at {x}: {prev} -> {y}");
                prev = y;
            }
        }
    }

    #[test]
    fn every_operator_reaches_near_white_at_the_top_of_the_domain() {
        for m in OPERATORS {
            let y = apply(m, TONE_LUT_MAX, GtParams::default());
            assert!(y > 0.9, "{m} only reached {y} at the top of the domain");
        }
    }

    /// GT's selling point for a photo editor: at contrast 1.0 the midsection
    /// is exactly linear, so middle grey passes through untouched and the
    /// profile's calibration (and the exposure slider) still mean what they
    /// meant. Reinhard, by contrast, drags it well down — which is exactly
    /// why it is documented as a reference point rather than a look.
    #[test]
    fn gt_preserves_middle_grey_where_reinhard_does_not() {
        let grey = 0.18;
        let gt = uchimura(grey, GtParams::default());
        assert!(
            (gt - grey).abs() < 0.01,
            "GT moved middle grey {grey} -> {gt}"
        );
        assert!(
            reinhard(grey) < grey * 0.9,
            "expected Reinhard to darken middle grey"
        );
    }

    #[test]
    fn gt_contrast_steepens_the_midtones() {
        let punchy = GtParams { contrast: 1.6, ..GtParams::default() };
        // Above the linear start, more contrast must lift; below it, hold down.
        assert!(uchimura(0.5, punchy) > uchimura(0.5, GtParams::default()));
        assert!(uchimura(0.05, punchy) <= uchimura(0.05, GtParams::default()) + 1e-6);
    }

    #[test]
    fn gt_black_tightness_deepens_the_toe() {
        let tight = GtParams { black_tightness: 2.5, ..GtParams::default() };
        assert!(uchimura(0.05, tight) < uchimura(0.05, GtParams::default()));
    }

    /// `sanitised` guards a division by `linear_start` in the toe term and a
    /// `powf` on its ratio — a zero or negative value would produce NaN and
    /// poison the whole LUT.
    #[test]
    fn gt_survives_degenerate_parameters() {
        let bad = GtParams {
            contrast: 0.0,
            linear_start: 0.0,
            linear_length: -5.0,
            black_tightness: 0.0,
        };
        for i in 0..64 {
            let y = uchimura(tone_lut_decode(i as f32 / 63.0), bad);
            assert!(y.is_finite(), "NaN/inf at entry {i}");
            assert!((0.0..=1.0).contains(&y));
        }
    }

    /// The CPU bake must reproduce the shader's Step 15 block, or selecting
    /// "Filmic" would render differently from leaving it on a profile that
    /// embeds no curve.
    #[test]
    fn filmic_matches_shader_form() {
        // Recomputed here in the shader's exact order of operations.
        for x in [0.0f32, 0.1, 0.18, 0.5, 0.64, 0.65, 0.9, 1.0, 3.0] {
            let lift = 0.08f32;
            let lifted = x * ((1.0 + lift) - lift * x);
            let expected = if lifted <= 0.65 {
                lifted
            } else {
                let over = lifted - 0.65;
                0.65 + (0.35 * over) / (over + 0.35)
            };
            assert!(
                (filmic(x) - expected.clamp(0.0, 1.0)).abs() < 1e-6,
                "filmic({x})"
            );
        }
    }

    #[test]
    fn bake_produces_a_full_monotonic_table() {
        for m in OPERATORS {
            let lut = bake(m, GtParams::default());
            assert_eq!(lut.len(), TONE_LUT_SIZE);
            assert!(lut.iter().all(|v| v.is_finite()));
            assert!(
                lut.windows(2).all(|w| w[1] >= w[0] - 1e-5),
                "{m} baked non-monotonic"
            );
        }
    }

    #[test]
    fn camera_is_the_default_and_is_not_a_baked_operator() {
        assert_eq!(ToneMapper::default(), ToneMapper::Camera);
        assert!(!ToneMapper::Camera.is_baked_operator());
        for m in OPERATORS {
            assert!(m.is_baked_operator(), "{m} should bake");
        }
        assert!(ToneMapper::Filmic.is_baked_operator());
    }

    #[test]
    fn shader_ids_are_distinct_and_camera_is_zero() {
        assert_eq!(ToneMapper::Camera.shader_id(), 0);
        let mut seen = Vec::new();
        for m in ToneMapper::ALL {
            assert!(!seen.contains(&m.shader_id()), "duplicate id for {m}");
            seen.push(m.shader_id());
        }
    }

    /// Serialised by name: renaming or reordering variants must not silently
    /// re-map saved edits onto a different look.
    #[test]
    fn tone_mapper_serialises_by_name() {
        let json = serde_json::to_string(&ToneMapper::Gt).unwrap();
        assert_eq!(json, "\"Gt\"");
        let back: ToneMapper = serde_json::from_str("\"AcesFitted\"").unwrap();
        assert_eq!(back, ToneMapper::AcesFitted);
    }
}
