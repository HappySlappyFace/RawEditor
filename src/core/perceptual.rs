//! Perceptual colour controls — Contrast, Whites/Blacks, Saturation,
//! Vibrance and the mask Hue — in Oklab (Björn Ottosson, 2020).
//!
//! This is the CPU mirror of the colour pass's perceptual block
//! (`gpu/shaders.rs`, the `ok_*` / `perceptual_*` functions): the shader is
//! what renders, these are what the unit tests check. Like
//! `core::highlights`, the two only agree while the constants below match
//! their WGSL twins — `shader_perceptual_constants_match_rust` guards that.
//!
//! ## Why Oklab, and why only here
//! Light adds and multiplies linearly, so everything that models light —
//! white balance, highlight reconstruction, dehaze, exposure, the DCP matrix
//! — stays in linear RGB. These controls are different: they are judged by
//! eye, after the tone curve, and linear RGB is a poor model of the eye.
//! Mixing towards Rec.709 luma (the old Saturation) keeps the RGB "hue" but
//! not the perceived one — a blue pushed or pulled drifts purple — and it
//! sends channels negative, which the clamp downstream turns into neon.
//! Oklab was fitted to CAM16 lightness/chroma and IPT hue data so that hue
//! stays put along a line of constant (a, b) direction, at the cost of two
//! 3×3 multiplies and three cube roots. Its own author scopes it to SDR
//! image processing, which is exactly this stage. (JzAzBz and ICtCp target
//! HDR and absolute luminance; they would cost PQ curves for nothing here.)
//!
//! ## One identity carries most of it
//! Oklab is homogeneous: scaling linear RGB by `t³` scales (L, a, b) by `t`.
//! So a change of lightness that keeps Oklab *saturation* `s = C/L` and hue
//! — what every tone control here wants — is just a multiply of the linear
//! colour by `(L'/L)³`. Contrast and Whites/Blacks never leave RGB; they
//! only need L. And because `rgb ≥ 0` is a cone through black, the most
//! saturated colour sRGB can hold at a hue is a fixed `s_max(h)`
//! (`max_saturation`, Ottosson's), independent of L: the chroma controls
//! respect it by construction instead of clipping afterwards.

/// Ottosson's linear sRGB → LMS, **row-major** (the codebase convention).
pub const OK_M1: [f32; 9] = [
    0.412_221_46, 0.536_332_55, 0.051_445_995,
    0.211_903_5, 0.680_699_5, 0.107_396_96,
    0.088_302_46, 0.281_718_85, 0.629_978_7,
];

/// Cube-rooted LMS → Oklab (L, a, b), row-major.
pub const OK_M2: [f32; 9] = [
    0.210_454_26, 0.793_617_8, -0.004_072_047,
    1.977_998_5, -2.428_592_2, 0.450_593_7,
    0.025_904_037, 0.782_771_77, -0.808_675_77,
];

/// Oklab → cube-rooted LMS, row-major (the inverse of `OK_M2`).
pub const OK_M2_INV: [f32; 9] = [
    1.0, 0.396_337_78, 0.215_803_76,
    1.0, -0.105_561_346, -0.063_854_17,
    1.0, -0.089_484_18, -1.291_485_5,
];

/// LMS → linear sRGB, row-major (the inverse of `OK_M1`).
pub const OK_M1_INV: [f32; 9] = [
    4.076_741_7, -3.307_711_6, 0.230_969_94,
    -1.268_438, 2.609_757_4, -0.341_319_38,
    -0.004_196_086_3, -0.703_418_6, 1.707_614_7,
];

/// Oklab L of scene middle grey (linear 0.18): `cbrt(0.18)`. Contrast
/// pivots here, the same pivot the old linear stretch used.
///
/// **Mirrored in `gpu/shaders.rs` as `CONTRAST_PIVOT_L`.**
pub const CONTRAST_PIVOT_L: f32 = 0.564_621_6;

/// Fraction of the gamut-limit saturation where a chroma boost starts to
/// roll off (see `soft_limit`). Below it a boost is exact.
///
/// **Mirrored as `SAT_KNEE`.**
pub const SAT_KNEE: f32 = 0.8;

/// Helmholtz–Kohlrausch compensation: saturated colours look brighter than
/// a grey of the same luminance, so a chroma boost alone also reads as a
/// brightness boost (and a desaturation as darkening). Lightness is moved
/// against the chroma change by `HK_STRENGTH · w(h) · (1 − L) · ΔC`, the
/// Fairchild–Pirrotta form. In Oklab units the full Fairchild–Pirrotta
/// coefficient is ≈ 10; it is taken at 30% because brightness-matching
/// studies find colour appearance models overestimate the effect, and
/// darktable's experience with JzAzBz is that resaturating while darkening
/// too much reads as muddy.
///
/// **Mirrored as `HK_STRENGTH`.**
pub const HK_STRENGTH: f32 = 3.0;

/// Oklab hue direction of skin, (cos 60°, sin 60°). Measured across light
/// to dark skin tones, Oklab hue runs ≈ 47–75°.
///
/// **Mirrored as `SKIN_DIR`.**
pub const SKIN_DIR: [f32; 2] = [0.5, 0.866_025_4];

/// Vibrance protection ramps in from `cos 40°` to full at `cos 15°` of hue
/// distance from `SKIN_DIR`. **Mirrored as `SKIN_COS_LO` / `SKIN_COS_HI`.**
pub const SKIN_COS_LO: f32 = 0.766_044_4;
pub const SKIN_COS_HI: f32 = 0.965_925_8;

/// How much of a vibrance *boost* skin is spared (Lightroom's vibrance is
/// known for leaving faces alone). **Mirrored as `SKIN_PROTECT`.**
pub const SKIN_PROTECT: f32 = 0.6;

fn apply(m: &[f32; 9], v: [f32; 3]) -> [f32; 3] {
    crate::color::mat3_apply(m, v)
}

/// Real cube root, defined for negative input (`f32::cbrt` already is).
fn cbrt3(v: [f32; 3]) -> [f32; 3] {
    [v[0].cbrt(), v[1].cbrt(), v[2].cbrt()]
}

pub fn linear_srgb_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    apply(&OK_M2, cbrt3(apply(&OK_M1, rgb)))
}

pub fn oklab_to_linear_srgb(lab: [f32; 3]) -> [f32; 3] {
    let lms_ = apply(&OK_M2_INV, lab);
    apply(&OK_M1_INV, [lms_[0].powi(3), lms_[1].powi(3), lms_[2].powi(3)])
}

/// Oklab L alone (one row of `OK_M2`).
pub fn oklab_l(rgb: [f32; 3]) -> f32 {
    let lms_ = cbrt3(apply(&OK_M1, rgb));
    OK_M2[0] * lms_[0] + OK_M2[1] * lms_[1] + OK_M2[2] * lms_[2]
}

/// The largest Oklab saturation `s = C/L` linear sRGB can hold along hue
/// direction `(a, b)` (a unit vector) before a channel goes negative.
///
/// Ottosson's `compute_max_saturation`, verbatim: a polynomial fit per
/// limiting channel, polished by one Halley step (error < 1e-6 except for
/// a few blues, where the boundary turns sharply). Exact for any L because
/// `rgb ≥ 0` is a cone through black.
pub fn max_saturation(a: f32, b: f32) -> f32 {
    let (k0, k1, k2, k3, k4, wl, wm, ws) = if -1.881_703_3 * a - 0.809_364_9 * b > 1.0 {
        // Red is the channel that runs out.
        (1.190_862_8, 1.765_767_3, 0.596_626_4, 0.755_152, 0.567_712_4, 4.076_741_7, -3.307_711_6, 0.230_969_94)
    } else if 1.814_441_1 * a - 1.194_452_8 * b > 1.0 {
        // Green.
        (0.739_565_15, -0.459_544_04, 0.082_854_27, 0.125_410_7, 0.145_032_04, -1.268_438, 2.609_757_4, -0.341_319_38)
    } else {
        // Blue.
        (1.357_336_5, -0.009_157_99, -1.151_302_1, -0.505_596_06, 0.006_921_67, -0.004_196_086_3, -0.703_418_6, 1.707_614_7)
    };
    let mut s = k0 + k1 * a + k2 * b + k3 * a * a + k4 * a * b;

    let k_l = 0.396_337_78 * a + 0.215_803_76 * b;
    let k_m = -0.105_561_346 * a - 0.063_854_17 * b;
    let k_s = -0.089_484_18 * a - 1.291_485_5 * b;
    let l_ = 1.0 + s * k_l;
    let m_ = 1.0 + s * k_m;
    let s_ = 1.0 + s * k_s;
    let f = wl * l_ * l_ * l_ + wm * m_ * m_ * m_ + ws * s_ * s_ * s_;
    let f1 = 3.0 * (wl * k_l * l_ * l_ + wm * k_m * m_ * m_ + ws * k_s * s_ * s_);
    let f2 = 6.0 * (wl * k_l * k_l * l_ + wm * k_m * k_m * m_ + ws * k_s * k_s * s_);
    s -= f * f1 / (f1 * f1 - 0.5 * f * f2);
    s
}

/// Roll a boosted saturation `target` off towards the gamut limit `s_max`
/// instead of crossing it: exact below `SAT_KNEE · s_max`, then an
/// exponential approach (slope-continuous at the knee) that never reaches
/// the limit. Never returns less than `s`, the saturation before the boost,
/// so a colour already near the edge holds rather than losing chroma.
pub fn soft_limit(target: f32, s: f32, s_max: f32) -> f32 {
    let knee = SAT_KNEE * s_max;
    if target <= knee {
        return target;
    }
    let room = s_max - knee;
    let limited = knee + room * (1.0 - (-(target - knee) / room).exp());
    limited.max(s)
}

/// Fairchild–Pirrotta hue weight `0.116·|sin((h − 90°)/2)| + 0.085` for the
/// unit hue direction `(a, b)`: largest for blues, smallest for yellows.
/// `|sin(θ/2)| = sqrt((1 − cos θ)/2)` and `cos(h − 90°) = sin h = b`, so no
/// trigonometry is needed.
pub fn hk_hue_weight(b: f32) -> f32 {
    0.116 * ((1.0 - b) * 0.5).max(0.0).sqrt() + 0.085
}

/// How much of the skin band hue direction `(a, b)` sits in, 0..1.
pub fn skin_weight(a: f32, b: f32) -> f32 {
    let c = a * SKIN_DIR[0] + b * SKIN_DIR[1];
    smoothstep(SKIN_COS_LO, SKIN_COS_HI, c)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Saturation and Vibrance on linear sRGB, in Oklab.
///
/// `sat` is a chroma factor (1 = unchanged, 0 = monochrome); `vib` the
/// vibrance amount (0 = unchanged, ±1 the slider ends). Hue is held exactly;
/// a boost rolls off into the gamut cone (`soft_limit`); lightness is
/// compensated for the Helmholtz–Kohlrausch effect. Vibrance weights by
/// distance from the gamut limit (`1 − s/s_max`) — muted colours move most,
/// whatever their hue — and spares skin a share of any boost.
pub fn saturation(rgb: [f32; 3], sat: f32, vib: f32) -> [f32; 3] {
    if sat == 1.0 && vib == 0.0 {
        return rgb;
    }
    let lab = linear_srgb_to_oklab(rgb);
    let l = lab[0];
    let c = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    if l <= 1e-6 || c <= 1e-7 {
        // Grey (or black) has no chroma to scale.
        return rgb;
    }
    let (da, db) = (lab[1] / c, lab[2] / c);
    let s = c / l;
    let s_max = max_saturation(da, db).max(1e-4);

    let mut s1 = s * sat;
    if sat > 1.0 {
        s1 = soft_limit(s1, s, s_max);
    }
    let mut s2 = s1;
    if vib != 0.0 {
        let mut w = 1.0 - (s1 / s_max).clamp(0.0, 1.0);
        if vib > 0.0 {
            w *= 1.0 - SKIN_PROTECT * skin_weight(da, db);
        }
        let f = 1.0 + vib * w;
        s2 = s1 * f;
        if f > 1.0 {
            s2 = soft_limit(s2, s1, s_max);
        }
    }

    let c2 = s2 * l;
    let hk = HK_STRENGTH * hk_hue_weight(db) * (1.0 - l).clamp(0.0, 1.0);
    // Scaling the whole (L, a, b) by `r` keeps s and hue: still in the cone.
    let r = ((l - hk * (c2 - c)) / l).clamp(0.5, 1.5);
    oklab_to_linear_srgb([l * r, da * c2 * r, db * c2 * r])
}

/// Rotate the Oklab hue by `angle` radians at constant L and chroma; a
/// colour the new hue cannot hold is brought in to its gamut limit along
/// that hue (constant L) rather than clipped per channel.
pub fn hue_shift(rgb: [f32; 3], angle: f32) -> [f32; 3] {
    if angle == 0.0 {
        return rgb;
    }
    let lab = linear_srgb_to_oklab(rgb);
    let c = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    if lab[0] <= 1e-6 || c <= 1e-7 {
        return rgb;
    }
    let (sn, cs) = angle.sin_cos();
    let da = (lab[1] * cs - lab[2] * sn) / c;
    let db = (lab[1] * sn + lab[2] * cs) / c;
    let c2 = c.min(max_saturation(da, db) * lab[0]);
    oklab_to_linear_srgb([lab[0], da * c2, db * c2])
}

/// The contrast curve on Oklab L: slope `k` at `CONTRAST_PIVOT_L`, black
/// and white held. Below the pivot `Lp·(L/Lp)^k`, above it the mirror image
/// towards 1; past 1 (scene-linear overbrights on the profile-less path)
/// the offset at 1 — zero — carries on unchanged.
pub fn contrast_curve(l: f32, k: f32) -> f32 {
    let p = CONTRAST_PIVOT_L;
    if l <= p {
        p * (l.max(0.0) / p).powf(k)
    } else if l < 1.0 {
        1.0 - (1.0 - p) * ((1.0 - l) / (1.0 - p)).powf(k)
    } else {
        l
    }
}

/// Contrast on linear sRGB: the S-curve acts on Oklab L, and the colour is
/// scaled to match, so hue and Oklab saturation are untouched (no more
/// saturation shift with contrast) and nothing is clipped to black.
/// `k = 1` is the identity.
pub fn contrast(rgb: [f32; 3], k: f32) -> [f32; 3] {
    if k == 1.0 {
        return rgb;
    }
    let l = oklab_l(rgb);
    if l <= 1e-6 {
        return rgb;
    }
    let r = contrast_curve(l, k.max(0.05)) / l;
    let g = r * r * r;
    [rgb[0] * g, rgb[1] * g, rgb[2] * g]
}

/// Whites/Blacks: the old linear levels `(Y − b)/(w − b)`, now applied to
/// `Y = L³` — which is exactly the channel value for a grey, so neutrals
/// map as before — and carried to colours by scaling, not per channel.
pub fn levels(rgb: [f32; 3], blacks: f32, whites: f32) -> [f32; 3] {
    if blacks == 0.0 && whites == 1.0 {
        return rgb;
    }
    let l = oklab_l(rgb);
    let y = l * l * l;
    if y <= 1e-9 {
        return [0.0; 3];
    }
    let g = ((y - blacks) / (whites - blacks + 0.0001)).max(0.0) / y;
    [rgb[0] * g, rgb[1] * g, rgb[2] * g]
}

// ── Gamut mapping ───────────────────────────────────────────────────────────
// The colour pass carries extended-range linear sRGB (negative channels for
// colours outside sRGB, channels past 1 for overbrights) from the camera
// matrix to the end, so every edit sees the real colour, and fits it into
// the display gamut ONCE, here, just before encoding.

/// Relative boundary distance (see `gamut_map`) where compression starts;
/// colours nearer the grey axis than this are returned untouched.
///
/// **Mirrored as `GAMUT_THRESHOLD`.**
pub const GAMUT_THRESHOLD: f32 = 0.9;

/// Relative distance that lands exactly on the boundary; anything further
/// out is clipped onto it. **Mirrored as `GAMUT_LIMIT`.**
pub const GAMUT_LIMIT: f32 = 1.5;

/// Shape of the compression curve (the ACES Reference Gamut Compression
/// uses 1.2). **Mirrored as `GAMUT_POWER`.**
pub const GAMUT_POWER: f32 = 1.2;

/// Ottosson's "adaptive L0" α: how far towards L = 0.5 a chromatic
/// colour's projection point moves. **Mirrored as `GAMUT_L0_ALPHA`.**
pub const GAMUT_L0_ALPHA: f32 = 0.05;

/// The scale that makes `gamut_compress(GAMUT_LIMIT) == 1` (the RGC's
/// parameterisation). **Mirrored, precomputed, as `GAMUT_SCALE`.**
pub fn gamut_compress_scale() -> f32 {
    let (t, l, p) = (GAMUT_THRESHOLD, GAMUT_LIMIT, GAMUT_POWER);
    (l - t) / (((1.0 - t) / (l - t)).powf(-p) - 1.0).powf(1.0 / p)
}

/// The ACES RGC power curve: identity below `GAMUT_THRESHOLD`, then a
/// slope-continuous roll-off that reaches 1 at `GAMUT_LIMIT`.
pub fn gamut_compress(d: f32) -> f32 {
    if d <= GAMUT_THRESHOLD {
        return d;
    }
    let s = gamut_compress_scale();
    let x = (d - GAMUT_THRESHOLD) / s;
    GAMUT_THRESHOLD + s * x / (1.0 + x.powf(GAMUT_POWER)).powf(1.0 / GAMUT_POWER)
}

/// Lightness and chroma of the most saturated colour sRGB holds at hue
/// direction `(a, b)` — the cusp of the gamut's L–C cross-section.
/// Ottosson's `find_cusp`.
pub fn find_cusp(a: f32, b: f32) -> (f32, f32) {
    let s_cusp = max_saturation(a, b);
    let rgb = oklab_to_linear_srgb([1.0, s_cusp * a, s_cusp * b]);
    let l_cusp = (1.0 / rgb[0].max(rgb[1]).max(rgb[2])).cbrt();
    (l_cusp, l_cusp * s_cusp)
}

/// Where the line from `(l0, 0)` through `(l1, c1)` meets the sRGB gamut
/// boundary, at hue direction `(a, b)`, as the parameter `t` with the
/// boundary at `(l0 + t·(l1 − l0), t·c1)`: `t < 1` means `(l1, c1)` is
/// outside. Ottosson's `find_gamut_intersection`: below the cusp the
/// boundary is the straight edge from black (the cone again); above it a
/// triangle estimate polished by one Halley step on whichever channel
/// reaches 1 first.
pub fn gamut_intersection(a: f32, b: f32, l1: f32, c1: f32, l0: f32) -> f32 {
    let (cusp_l, cusp_c) = find_cusp(a, b);
    if (l1 - l0) * cusp_c - (cusp_l - l0) * c1 <= 0.0 {
        return cusp_c * l0 / (c1 * cusp_l + cusp_c * (l0 - l1));
    }
    let mut t = cusp_c * (l0 - 1.0) / (c1 * (cusp_l - 1.0) + cusp_c * (l0 - l1));

    let dl = l1 - l0;
    let k_l = 0.396_337_78 * a + 0.215_803_76 * b;
    let k_m = -0.105_561_346 * a - 0.063_854_17 * b;
    let k_s = -0.089_484_18 * a - 1.291_485_5 * b;
    let l_dt = dl + c1 * k_l;
    let m_dt = dl + c1 * k_m;
    let s_dt = dl + c1 * k_s;

    let l = l0 * (1.0 - t) + t * l1;
    let c = t * c1;
    let l_ = l + c * k_l;
    let m_ = l + c * k_m;
    let s_ = l + c * k_s;
    let lms = [l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_];
    let d1 = [3.0 * l_dt * l_ * l_, 3.0 * m_dt * m_ * m_, 3.0 * s_dt * s_ * s_];
    let d2 = [6.0 * l_dt * l_dt * l_, 6.0 * m_dt * m_dt * m_, 6.0 * s_dt * s_dt * s_];
    let mut step = f32::MAX;
    for row in 0..3 {
        let w = &OK_M1_INV[row * 3..row * 3 + 3];
        let f = w[0] * lms[0] + w[1] * lms[1] + w[2] * lms[2] - 1.0;
        let f1 = w[0] * d1[0] + w[1] * d1[1] + w[2] * d1[2];
        let f2 = w[0] * d2[0] + w[1] * d2[1] + w[2] * d2[2];
        let u = f1 / (f1 * f1 - 0.5 * f * f2);
        if u >= 0.0 {
            step = step.min(-f * u);
        }
    }
    if step < f32::MAX {
        t += step;
    }
    t
}

/// Fit extended-range linear sRGB into `[0, 1]³` at constant Oklab hue.
///
/// Each colour is measured by its relative distance `d` to the gamut
/// boundary along the line from a point `L0` on the grey axis (`d = 1` on
/// the boundary). Below `GAMUT_THRESHOLD` it is returned untouched; above,
/// `d` is compressed along that line (`gamut_compress`, the ACES RGC curve)
/// so colours near and past the edge keep their gradations instead of
/// piling up on it, and anything past `GAMUT_LIMIT` is clipped onto it.
///
/// `L0` is Ottosson's adaptive point: it tends to the colour's own L as its
/// chroma vanishes — so near-greys only ever lose chroma, never lightness,
/// and a dark neutral is never lifted — and to white for overbrights, which
/// is the path to white: a light too bright for its hue fades to white
/// rather than flattening into a neon primary.
pub fn gamut_map(rgb: [f32; 3]) -> [f32; 3] {
    let lab = linear_srgb_to_oklab(rgb);
    let l = lab[0];
    if l <= 0.0 {
        return [0.0; 3];
    }
    let c = lab[1].hypot(lab[2]);
    if c <= 1e-6 {
        // A grey: in gamut unless brighter than white.
        return if l <= 1.0 { rgb.map(|v| v.clamp(0.0, 1.0)) } else { [1.0; 3] };
    }
    let (da, db) = (lab[1] / c, lab[2] / c);
    let ld = l - 0.5;
    let e1 = 0.5 + ld.abs() + GAMUT_L0_ALPHA * c;
    let l0 = 0.5 * (1.0 + ld.signum() * (e1 - (e1 * e1 - 2.0 * ld.abs()).max(0.0).sqrt()));
    let t = gamut_intersection(da, db, l, c, l0);
    let d = 1.0 / t.max(1e-6);
    if d <= GAMUT_THRESHOLD {
        return rgb;
    }
    let f = gamut_compress(d).min(1.0) / d;
    let (l2, c2) = (l0 + f * (l - l0), f * c);
    oklab_to_linear_srgb([l2, da * c2, db * c2]).map(|v| v.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
    }

    fn hue(rgb: [f32; 3]) -> f32 {
        let lab = linear_srgb_to_oklab(rgb);
        lab[2].atan2(lab[1]).to_degrees()
    }

    fn hue_diff(a: f32, b: f32) -> f32 {
        ((a - b + 540.0) % 360.0 - 180.0).abs()
    }

    /// Hues around the circle at a few chroma levels, all inside sRGB.
    fn samples() -> Vec<[f32; 3]> {
        let mut v = Vec::new();
        for i in 0..36 {
            let h = (i as f32 * 10.0).to_radians();
            for &(l, frac) in &[(0.45, 0.5), (0.65, 0.3), (0.75, 0.85), (0.55, 0.95)] {
                let s = max_saturation(h.cos(), h.sin()) * frac;
                v.push(oklab_to_linear_srgb([l, h.cos() * s * l, h.sin() * s * l]));
            }
        }
        v
    }

    #[test]
    fn oklab_matches_ottossons_reference_values() {
        // Linear sRGB red, from Ottosson's reference implementation.
        let red = linear_srgb_to_oklab([1.0, 0.0, 0.0]);
        assert!(close(red, [0.627_955_4, 0.224_863_1, 0.125_846_3], 1e-5), "{red:?}");
        let white = linear_srgb_to_oklab([1.0, 1.0, 1.0]);
        assert!(close(white, [1.0, 0.0, 0.0], 1e-5), "{white:?}");
    }

    #[test]
    fn oklab_round_trips() {
        for rgb in [[0.2, 0.5, 0.9], [1.0, 0.0, 0.0], [0.01, 0.002, 0.03], [1.4, 0.9, 0.2]] {
            let back = oklab_to_linear_srgb(linear_srgb_to_oklab(rgb));
            assert!(close(rgb, back, 1e-5), "{rgb:?} → {back:?}");
        }
    }

    #[test]
    fn grey_has_lightness_cube_root_of_its_value() {
        // OK_M1's rows sum to 1, so a grey's L³ is the grey itself — which is
        // what lets `levels` keep the old mapping for neutrals.
        for g in [0.01f32, 0.18, 0.5, 1.0] {
            assert!((oklab_l([g, g, g]).powi(3) - g).abs() < 1e-5);
        }
    }

    /// The colour at saturation `s_max` must sit on the gamut boundary: its
    /// smallest channel ≈ 0 and none negative.
    #[test]
    fn max_saturation_lands_on_the_srgb_boundary() {
        for i in 0..360 {
            let h = (i as f32).to_radians();
            let (a, b) = (h.cos(), h.sin());
            let s = max_saturation(a, b);
            let rgb = oklab_to_linear_srgb([1.0, a * s, b * s]);
            let lo = rgb[0].min(rgb[1]).min(rgb[2]);
            let hi = rgb[0].max(rgb[1]).max(rgb[2]);
            // Relative to the brightest channel: at L = 1 the deep blues
            // reach channel values near 8, where Ottosson's single Halley
            // step leaves an error of a few 1e-4 of that.
            assert!(lo.abs() < 2e-3 * hi, "hue {i}°: min channel {lo} ({rgb:?})");
        }
    }

    #[test]
    fn every_control_is_the_identity_at_its_default() {
        for rgb in samples() {
            assert_eq!(saturation(rgb, 1.0, 0.0), rgb);
            assert_eq!(contrast(rgb, 1.0), rgb);
            assert_eq!(levels(rgb, 0.0, 1.0), rgb);
            assert_eq!(hue_shift(rgb, 0.0), rgb);
        }
    }

    #[test]
    fn saturation_holds_hue_and_stays_in_gamut() {
        for rgb in samples() {
            let h0 = hue(rgb);
            for (sat, vib) in [(2.0, 0.0), (1.5, 1.0), (0.4, 0.0), (1.0, -1.0), (1.0, 1.0)] {
                let out = saturation(rgb, sat, vib);
                assert!(
                    hue_diff(hue(out), h0) < 0.5,
                    "{rgb:?} sat {sat} vib {vib}: hue {h0} → {}",
                    hue(out)
                );
                assert!(out.iter().all(|&c| c >= -1e-4), "{rgb:?} → {out:?} left the gamut");
            }
        }
    }

    /// The old luma mix drove channels negative here and the clamp turned
    /// the result into a primary.
    #[test]
    fn a_saturated_blue_boosted_hard_stays_blue_not_purple() {
        let blue = [0.05, 0.12, 0.8];
        let out = saturation(blue, 2.0, 0.0);
        assert!(hue_diff(hue(out), hue(blue)) < 0.5);
        assert!(out[0] >= 0.0 && out[1] >= 0.0);
        // More chroma than before, but not past the boundary.
        let c = |v: [f32; 3]| {
            let l = linear_srgb_to_oklab(v);
            (l[1] * l[1] + l[2] * l[2]).sqrt()
        };
        assert!(c(out) > c(blue));
    }

    #[test]
    fn zero_saturation_is_neutral() {
        for rgb in samples() {
            let out = saturation(rgb, 0.0, 0.0);
            let lab = linear_srgb_to_oklab(out);
            assert!(lab[1].abs() < 1e-4 && lab[2].abs() < 1e-4, "{rgb:?} → {out:?}");
        }
    }

    #[test]
    fn vibrance_moves_muted_colours_more_than_vivid_ones() {
        let h = 200f32.to_radians();
        let s_max = max_saturation(h.cos(), h.sin());
        let at = |frac: f32| oklab_to_linear_srgb([0.6, h.cos() * s_max * frac * 0.6, h.sin() * s_max * frac * 0.6]);
        let gain = |frac: f32| {
            let rgb = at(frac);
            let s = |v: [f32; 3]| {
                let l = linear_srgb_to_oklab(v);
                (l[1] * l[1] + l[2] * l[2]).sqrt() / l[0]
            };
            s(saturation(rgb, 1.0, 1.0)) / s(rgb)
        };
        assert!(gain(0.2) > gain(0.7));
        assert!(gain(0.7) > 1.0);
    }

    #[test]
    fn vibrance_spares_skin() {
        let skin = linear_srgb_to_oklab([0.6, 0.35, 0.22]);
        let c = (skin[1] * skin[1] + skin[2] * skin[2]).sqrt();
        let (da, db) = (skin[1] / c, skin[2] / c);
        assert!(skin_weight(da, db) > 0.9, "test colour should read as skin");
        let rotated = 200f32.to_radians();
        let other = oklab_to_linear_srgb([skin[0], rotated.cos() * c, rotated.sin() * c]);
        let gain = |rgb: [f32; 3]| {
            let a = linear_srgb_to_oklab(rgb);
            let b = linear_srgb_to_oklab(saturation(rgb, 1.0, 1.0));
            (b[1].hypot(b[2]) / b[0]) / (a[1].hypot(a[2]) / a[0])
        };
        assert!(gain([0.6, 0.35, 0.22]) < gain(other));
    }

    #[test]
    fn saturation_compensates_brightness() {
        // H–K: more chroma reads brighter, so L comes down a little.
        let rgb = [0.5, 0.2, 0.1];
        let l0 = oklab_l(rgb);
        let l1 = oklab_l(saturation(rgb, 1.6, 0.0));
        assert!(l1 < l0 && l1 > l0 * 0.95, "{l0} → {l1}");
    }

    #[test]
    fn contrast_keeps_hue_saturation_and_black() {
        for rgb in samples() {
            for k in [0.5, 0.9, 1.1, 2.0] {
                let out = contrast(rgb, k);
                let (a, b) = (linear_srgb_to_oklab(rgb), linear_srgb_to_oklab(out));
                assert!(hue_diff(hue(out), hue(rgb)) < 0.1);
                let s = |l: [f32; 3]| l[1].hypot(l[2]) / l[0];
                assert!((s(a) - s(b)).abs() < 1e-3 * s(a).max(1.0));
            }
        }
        assert_eq!(contrast([0.0; 3], 1.1), [0.0; 3]);
    }

    #[test]
    fn contrast_curve_is_an_s_curve_through_the_pivot() {
        let p = CONTRAST_PIVOT_L;
        assert!((contrast_curve(p, 1.5) - p).abs() < 1e-6);
        assert_eq!(contrast_curve(0.0, 1.5), 0.0);
        assert!((contrast_curve(1.0, 1.5) - 1.0).abs() < 1e-6);
        assert!(contrast_curve(0.3, 1.5) < 0.3 && contrast_curve(0.8, 1.5) > 0.8);
        assert!(contrast_curve(0.3, 0.7) > 0.3 && contrast_curve(0.8, 0.7) < 0.8);
        // Past white the profile-less path still carries overbrights through.
        assert_eq!(contrast_curve(1.4, 1.5), 1.4);
        // Increasing contrast never crushes a shadow to black.
        assert!(contrast([0.01, 0.01, 0.01], 1.1)[0] > 0.0);
    }

    #[test]
    fn levels_match_the_old_mapping_on_greys() {
        for g in [0.05f32, 0.3, 0.9] {
            let old = (g - 0.05) / (0.95 - 0.05 + 0.0001);
            let new = levels([g, g, g], 0.05, 0.95)[0];
            assert!((old.max(0.0) - new).abs() < 1e-4, "{g}: {old} vs {new}");
        }
    }

    fn in_unit_cube(rgb: [f32; 3]) -> bool {
        rgb.iter().all(|&v| (0.0..=1.0).contains(&v))
    }

    #[test]
    fn gamut_compress_is_continuous_and_lands_on_the_boundary_at_the_limit() {
        assert_eq!(gamut_compress(GAMUT_THRESHOLD), GAMUT_THRESHOLD);
        assert!((gamut_compress(GAMUT_LIMIT) - 1.0).abs() < 1e-5);
        // Slope 1 at the threshold (no kink), monotone, and it never passes 1
        // before the limit.
        let e = 1e-3;
        assert!(((gamut_compress(GAMUT_THRESHOLD + e) - GAMUT_THRESHOLD) / e - 1.0).abs() < 0.05);
        let mut prev = 0.0;
        for i in 0..200 {
            let d = i as f32 * 0.01;
            let v = gamut_compress(d);
            assert!(v >= prev, "not monotone at {d}");
            assert!(d > GAMUT_LIMIT || v <= 1.0 + 1e-6);
            prev = v;
        }
    }

    /// The cusp is the corner of the gamut's L–C slice: on the boundary,
    /// with a channel at 1 and one at 0.
    #[test]
    fn the_cusp_sits_on_the_gamut_corner() {
        for i in 0..72 {
            let h = (i as f32 * 5.0).to_radians();
            let (l, c) = find_cusp(h.cos(), h.sin());
            let rgb = oklab_to_linear_srgb([l, c * h.cos(), c * h.sin()]);
            let hi = rgb[0].max(rgb[1]).max(rgb[2]);
            let lo = rgb[0].min(rgb[1]).min(rgb[2]);
            assert!((hi - 1.0).abs() < 2e-3 && lo.abs() < 2e-3, "{}°: {rgb:?}", i * 5);
        }
    }

    #[test]
    fn gamut_intersection_finds_the_boundary_along_the_line() {
        for i in 0..72 {
            let h = (i as f32 * 5.0).to_radians();
            let (a, b) = (h.cos(), h.sin());
            for &(l1, c1, l0) in &[(0.3, 0.4, 0.3), (0.9, 0.3, 0.7), (0.6, 0.5, 0.5), (1.2, 0.2, 0.95)] {
                let t = gamut_intersection(a, b, l1, c1, l0);
                let (l, c) = (l0 + t * (l1 - l0), t * c1);
                let rgb = oklab_to_linear_srgb([l, c * a, c * b]);
                let lo = rgb[0].min(rgb[1]).min(rgb[2]);
                let hi = rgb[0].max(rgb[1]).max(rgb[2]);
                // On the boundary: one channel at 0 or at 1, none past either.
                let edge = lo.abs().min((hi - 1.0).abs());
                assert!(edge < 5e-3 && lo > -5e-3 && hi < 1.0 + 5e-3, "{}° {l1},{c1}: {rgb:?}", i * 5);
            }
        }
    }

    /// Everything — far outside sRGB, negative, overbright — comes out
    /// displayable, at its own Oklab hue.
    #[test]
    fn gamut_map_fits_anything_into_srgb_at_constant_hue() {
        let mut inputs = samples();
        for i in 0..72 {
            let h = (i as f32 * 5.0).to_radians();
            for &(l, s) in &[(0.4, 0.9), (0.6, 0.6), (0.8, 0.35), (0.95, 0.2), (1.1, 0.15), (0.5, 1.5)] {
                inputs.push(oklab_to_linear_srgb([l, h.cos() * s * l, h.sin() * s * l]));
            }
        }
        inputs.extend([[1.3, 0.2, 0.15], [-0.2, 0.5, 0.9], [2.0, 2.0, 1.8], [0.4, -0.05, 0.1]]);
        for rgb in inputs {
            let out = gamut_map(rgb);
            assert!(in_unit_cube(out), "{rgb:?} → {out:?}");
            let lab = linear_srgb_to_oklab(out);
            // Hue only means something where some chroma survives.
            if lab[1].hypot(lab[2]) > 0.02 {
                assert!(hue_diff(hue(out), hue(rgb)) < 1.0, "{rgb:?}: {} → {}", hue(rgb), hue(out));
            }
        }
    }

    #[test]
    fn gamut_map_leaves_ordinary_colours_exactly_alone() {
        for rgb in [[0.18, 0.18, 0.18], [0.5, 0.4, 0.3], [0.02, 0.03, 0.05], [0.9, 0.85, 0.7], [0.0; 3], [1.0; 3]] {
            assert_eq!(gamut_map(rgb), rgb);
        }
        // Moderate chroma at every hue: well inside the threshold.
        for i in 0..36 {
            let h = (i as f32 * 10.0).to_radians();
            let s = max_saturation(h.cos(), h.sin()) * 0.4;
            let rgb = oklab_to_linear_srgb([0.55, h.cos() * s * 0.55, h.sin() * s * 0.55]);
            assert_eq!(gamut_map(rgb), rgb);
        }
    }

    /// Overbright: a grey goes to white, and a colour too bright for its hue
    /// heads to white rather than to the primary.
    #[test]
    fn gamut_map_takes_overbrights_towards_white() {
        assert_eq!(gamut_map([1.5, 1.5, 1.5]), [1.0; 3]);
        let red = gamut_map([2.5, 0.6, 0.5]);
        let clipped = [1.0, 0.6, 0.5];
        let chroma = |v: [f32; 3]| {
            let l = linear_srgb_to_oklab(v);
            l[1].hypot(l[2])
        };
        assert!(chroma(red) < chroma(clipped), "{red:?}");
        assert!(oklab_l(red) > oklab_l(clipped) - 0.05);
    }

    /// Continuity: walking a colour from inside the gamut to far outside
    /// changes the output smoothly — no jump at the threshold or boundary.
    #[test]
    fn gamut_map_has_no_jumps() {
        for i in 0..12 {
            let h = (i as f32 * 30.0).to_radians();
            let s_max = max_saturation(h.cos(), h.sin());
            let mut prev: Option<[f32; 3]> = None;
            for k in 0..400 {
                let s = s_max * k as f32 / 200.0;
                let out = gamut_map(oklab_to_linear_srgb([0.6, h.cos() * s * 0.6, h.sin() * s * 0.6]));
                if let Some(p) = prev {
                    let step = (0..3).map(|j| (out[j] - p[j]).abs()).fold(0.0, f32::max);
                    assert!(step < 0.02, "{}°: jump of {step} at s = {s}", i * 30);
                }
                prev = Some(out);
            }
        }
    }

    /// Dark colours keep their lightness: compression moves them towards
    /// a point at their own L, not towards mid grey.
    #[test]
    fn gamut_map_never_lifts_shadows() {
        for rgb in [[0.01, -0.002, 0.03], [0.03, 0.0, 0.0], [0.002, 0.004, 0.0]] {
            let out = gamut_map(rgb);
            assert!((oklab_l(out) - oklab_l(rgb)).abs() < 0.01, "{rgb:?} → {out:?}");
        }
    }

    #[test]
    fn hue_shift_rotates_and_stays_in_gamut() {
        for rgb in samples() {
            let out = hue_shift(rgb, 30f32.to_radians());
            assert!(hue_diff(hue(out), hue(rgb) + 30.0) < 0.5, "{rgb:?}");
            assert!(out.iter().all(|&c| c >= -2e-3), "{rgb:?} → {out:?}");
            assert!((oklab_l(out) - oklab_l(rgb)).abs() < 1e-4);
        }
    }
}
