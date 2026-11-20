//! Highlight reconstruction, CPU half: the colour correction clipped
//! highlights are rebuilt with, learned once at load from the mosaic (like
//! `core::haze`). The rebuild itself is Step 3 of the colour pass; the
//! tests here check it through `reconstruct`, its CPU mirror.
//!
//! ## The problem
//! A photosite that saturates records its ceiling instead of its true value.
//! Bayer green is the most sensitive channel, so in a bright sky or cloud it
//! clips first, then blue, while red still holds data. White balance
//! multiplies red and blue up past the clipped green — magenta — and pulling
//! exposure down, which should grey that region out like the unclipped sky
//! beside it, shows it as purple instead.
//!
//! ## Rebuilding (darktable's "inpaint opposed", iterated)
//! Each clipped channel is estimated from the other two — the *opposed*
//! channels — by their geometric mean in white-balanced space, times a
//! correction learned here. Two things matter:
//!
//! - **Clipped opposed channels still count.** A clipped value is a lower
//!   bound on the truth, and a close one near the clip. Estimating green
//!   from red alone because blue is clipped too (the previous rule: the mean
//!   of the *unclipped* channels) sets a sky's green equal to its red while
//!   blue stays at its ceiling above both — lavender. Instead every clipped
//!   channel is re-estimated from the others' *current estimates*, a few
//!   rounds, so two clipped channels lift each other to a consistent colour.
//!   With nothing unclipped, that converges on grey at the brightest ceiling.
//! - **The geometric mean is the right neutral.** Daylight, skylight,
//!   cloud and tungsten all sit near the Planckian locus, where green is the
//!   geometric mean of red and blue once white-balanced; a highlight's colour
//!   off that line (a face: red well above the other two) is what the
//!   learned correction is for.
//!
//! An estimate only ever *raises* a channel (`max` with the measured value),
//! so a wrong guess low is harmless; and it is blended in by each channel's
//! clip weight, so it fades in continuously as a channel approaches clipping.
//!
//! ## The learned correction
//! For each channel `c`, `k_c = ln c − ½(ln o₁ + ln o₂)` over quads with
//! every channel unclipped that lie near clipping, weighted by how close to
//! clipping they are (the colour a highlight has just before it clips is the
//! best evidence of the colour it has after — darktable's chrominance
//! correction). All three come from the same samples: a sky clips green
//! before blue, so wherever blue is about to clip green already has, and a
//! per-channel rim (darktable's) would have no clean evidence for blue.
//!
//! One correction per image, like darktable's, not a local map. A local one
//! was tried: a clipped sky running off the top of the frame has no rim of
//! its own, and its nearest "rim" is the defocused skyline below it — sky
//! mixed with whatever it meets — which tinted it. Averaged over the whole
//! image, the sky's real rims outweigh such mixtures.
//!
//! Only the **warm–cool** part of it is kept (`k_r` and `k_b` move by equal
//! and opposite amounts; `k_g` stays on the locus). Natural light varies
//! along that axis — skylight, daylight, sunset, tungsten — and the bright
//! neighbours most likely to contaminate the evidence, foliage against a
//! sky, differ from the highlight precisely on the other one: measured
//! unrestricted, the rims of a grey overcast sky taught a green cast.
//!
//! It is stored **camera-native**, a property of the scene that holds under
//! any white balance; the colour pass converts it. Where evidence is thin it
//! is pulled towards grey under the as-shot white balance.
//!
//! Clipping and nearness are judged on a grid laid over UV rather than
//! pixels, so the preview and the full-resolution export learn the same
//! correction.

/// Cells along the image's long edge.
const GRID: usize = 64;
/// Quads sampled per cell axis, at most (a full-resolution export has ~47).
const MAX_QUADS: usize = 32;
/// A quad with any site at or above this (camera-native, sensor white = 1)
/// counts as clipped.
const CLIPPED: f32 = 0.99;
/// Start of the colour pass's clip band: a channel at `HL_CLIP_LO` of its
/// ceiling begins to be rebuilt. MIRRORED in `gpu::shaders` — checked by
/// `shader_highlight_constants_match_rust`. Samples keep every channel below
/// it, so they are values Step 3 itself trusts.
pub const HL_CLIP_LO: f32 = 0.9;
/// A sample's brightest channel must reach this: the shoulder of a
/// highlight, not whatever lies beside it.
const SAMPLE_LO: f32 = 0.45;
/// Channels darker than this make the log ratios meaningless.
const SAMPLE_FLOOR: f32 = 0.01;
/// A cell clips when this share of its quads do (or two quads, whichever is
/// more) — one hot pixel does not make a highlight.
const CLIPPED_SHARE: f32 = 0.005;
/// Samples count only within this many cells of a clipped cell (~12% of
/// the long edge): a sky's unclipped rim can lie well away from where it
/// clips, but a bright subject across the frame is not evidence.
const DILATE: usize = 8;
/// Total sample weight at which the learned correction is fully trusted;
/// with less it is pulled towards grey (darktable requires 100 samples
/// before using any).
const CONFIDENT_WEIGHT: f32 = 40.0;
/// The learned warm–cool deviation from the as-shot grey may be at most this
/// (natural log, per `k_r`/`k_b`) — a guard against a mis-sampled rim, not a
/// limit a real highlight approaches: a clear blue sky is ~0.2.
const MAX_DEVIATION: f32 = 0.6;
/// Rebuilt channels are desaturated towards the brightest one as the
/// *least* clipped channel climbs from this share of its ceiling to
/// clipping: with nothing left unclipped there is no colour to keep, and a
/// blown area renders white. White at the brightest *estimate*, not the
/// sensor ceiling — a sky rebuilt bluer than grey would otherwise lose
/// blue as it brightened — and over a half stop rather than the clip band
/// itself, which is too narrow to read as anything but a ring. MIRRORED in
/// `gpu::shaders` (only the mirror, `reconstruct`, uses it on the CPU).
#[cfg(test)]
pub const HL_FADE_LO: f32 = 0.7;
/// Rounds of re-estimation. Each one halves the remaining error of two
/// channels clipped together (Gauss–Seidel on the opposed means); MIRRORED
/// in `gpu::shaders`.
#[cfg(test)]
pub const HL_ITERATIONS: usize = 4;

/// What the colour pass needs to rebuild clipped highlights: the correction
/// `[k_r, k_g, k_b]`, camera-native (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighlightModel {
    pub correction: [f32; 3],
}

impl HighlightModel {
    /// No evidence: highlights are rebuilt grey under the as-shot white
    /// balance `wb`.
    pub fn neutral(wb: [f32; 3]) -> Self {
        Self { correction: grey_correction(wb) }
    }
}

/// The camera-native correction of a colour that is grey under `wb`
/// (native `1/wb`): `k_c = −ln w_c + ½(ln w_o₁ + ln w_o₂)`.
pub fn grey_correction(wb: [f32; 3]) -> [f32; 3] {
    let l = wb.map(|w| w.max(1e-6).ln());
    let sum = l[0] + l[1] + l[2];
    l.map(|lc| -1.5 * lc + 0.5 * sum)
}

/// The correction learned in camera-native terms, as seen in the space
/// white-balanced by `wb` — what the colour pass applies (Step 3 does this
/// conversion itself).
#[cfg(test)]
pub fn correction_under(k_native: [f32; 3], wb: [f32; 3]) -> [f32; 3] {
    let g = grey_correction(wb);
    std::array::from_fn(|c| k_native[c] - g[c])
}

/// Analyse a Bayer mosaic. `colors`, `black` and `scale` are per 2×2 tile
/// position exactly as `gpu::demosaic` uses them; `wb` is its per-colour
/// ceiling (the as-shot white balance), which `scale` includes and which is
/// divided back out here to get camera-native values.
pub fn analyse(
    mosaic: &[u16],
    width: usize,
    height: usize,
    colors: [u32; 4],
    black: [f32; 4],
    scale: [f32; 4],
    wb: [f32; 3],
) -> HighlightModel {
    let neutral = HighlightModel::neutral(wb);
    let (qw, qh) = (width / 2, height / 2);
    if qw < 16 || qh < 16 || mosaic.len() < width * height {
        return neutral;
    }
    let (gw, gh) = if qw >= qh {
        (GRID.min(qw), ((GRID * qh) as f32 / qw as f32).round().max(1.0) as usize)
    } else {
        (((GRID * qw) as f32 / qh as f32).round().max(1.0) as usize, GRID.min(qh))
    };
    let (gw, gh) = (gw.clamp(1, qw), gh.clamp(1, qh));
    let n = gw * gh;
    // Tile position → camera-native multiplier.
    let native: [f32; 4] = std::array::from_fn(|k| scale[k] / wb[colors[k] as usize].max(1e-6));

    let mut weight = vec![0.0f32; n];
    let mut sum: [Vec<f32>; 3] = std::array::from_fn(|_| vec![0.0; n]);
    let mut clipped = vec![false; n];
    for cy in 0..gh {
        for cx in 0..gw {
            let (qx0, qx1) = (cx * qw / gw, ((cx + 1) * qw / gw).max(cx * qw / gw + 1));
            let (qy0, qy1) = (cy * qh / gh, ((cy + 1) * qh / gh).max(cy * qh / gh + 1));
            let (nx, ny) = ((qx1 - qx0).min(MAX_QUADS), (qy1 - qy0).min(MAX_QUADS));
            let i = cy * gw + cx;
            let mut clipped_quads = 0usize;
            for j in 0..ny {
                let qy = qy0 + j * (qy1 - qy0) / ny;
                for t in 0..nx {
                    let qx = qx0 + t * (qx1 - qx0) / nx;
                    let mut total = [0.0f32; 3];
                    let mut count = [0.0f32; 3];
                    let mut peak = [0.0f32; 3];
                    for k in 0..4 {
                        let (x, y) = (2 * qx + (k & 1), 2 * qy + (k >> 1));
                        let c = colors[k] as usize;
                        let v = (mosaic[y * width + x] as f32 - black[k]).max(0.0) * native[k];
                        total[c] += v;
                        count[c] += 1.0;
                        peak[c] = peak[c].max(v);
                    }
                    let hi = peak[0].max(peak[1]).max(peak[2]);
                    if hi >= CLIPPED {
                        clipped_quads += 1;
                        continue;
                    }
                    let mean: [f32; 3] = std::array::from_fn(|c| total[c] / count[c].max(1.0));
                    if !(SAMPLE_LO..HL_CLIP_LO).contains(&hi) || mean.iter().any(|&m| m < SAMPLE_FLOOR) {
                        continue;
                    }
                    let w = ((hi - SAMPLE_LO) / (HL_CLIP_LO - SAMPLE_LO)).powi(2);
                    let l = mean.map(f32::ln);
                    weight[i] += w;
                    for c in 0..3 {
                        sum[c][i] += w * (l[c] - 0.5 * (l[(c + 1) % 3] + l[(c + 2) % 3]));
                    }
                }
            }
            clipped[i] = clipped_quads >= 2 && clipped_quads as f32 >= (nx * ny) as f32 * CLIPPED_SHARE;
        }
    }

    // Only the rims of clipped areas are evidence.
    let mut near = vec![false; n];
    for cy in 0..gh {
        for cx in 0..gw {
            if clipped[cy * gw + cx] {
                for y in cy.saturating_sub(DILATE)..=(cy + DILATE).min(gh - 1) {
                    for x in cx.saturating_sub(DILATE)..=(cx + DILATE).min(gw - 1) {
                        near[y * gw + x] = true;
                    }
                }
            }
        }
    }
    for i in (0..n).filter(|&i| !near[i]) {
        weight[i] = 0.0;
        for s in &mut sum {
            s[i] = 0.0;
        }
    }
    let total: f32 = weight.iter().sum();
    if total <= 0.0 {
        return neutral;
    }

    let grey = grey_correction(wb);
    let learned: [f32; 3] = std::array::from_fn(|c| sum[c].iter().sum::<f32>() / total);
    let warm = ((learned[0] - grey[0]) - (learned[2] - grey[2])) * 0.5;
    let d = warm.clamp(-MAX_DEVIATION, MAX_DEVIATION) * (total / CONFIDENT_WEIGHT).min(1.0);
    HighlightModel { correction: [grey[0] + d, grey[1], grey[2] - d] }
}

/// Rebuild one white-balanced pixel exactly as the colour pass's Step 3
/// does — the CPU mirror of `reconstruct_highlight` in `gpu::shaders`.
/// `ceiling` is each channel's clip point (the white balance itself) and
/// `k` the correction in this white balance's terms ([`correction_under`]).
#[cfg(test)]
pub fn reconstruct(measured: [f32; 3], ceiling: [f32; 3], k: [f32; 3]) -> [f32; 3] {
    let smooth = |e0: f32, e1: f32, x: f32| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let clip_w: [f32; 3] = std::array::from_fn(|c| smooth(HL_CLIP_LO * ceiling[c], ceiling[c], measured[c]));
    if clip_w.iter().all(|&w| w <= 0.0) {
        return measured;
    }
    let lm = measured.map(|v| v.max(1e-6).ln());
    let mut l = lm;
    for _ in 0..HL_ITERATIONS {
        for c in [1, 2, 0] {
            let est = 0.5 * (l[(c + 1) % 3] + l[(c + 2) % 3]) + k[c];
            l[c] = lm[c] + (lm[c].max(est) - lm[c]) * clip_w[c];
        }
    }
    let least = (0..3).map(|c| measured[c] / ceiling[c]).fold(f32::MAX, f32::min);
    let white = smooth(HL_FADE_LO, 1.0, least);
    let top = l[0].max(l[1]).max(l[2]);
    std::array::from_fn(|c| (l[c] + (top - l[c]) * white * clip_w[c]).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WB: [f32; 3] = [2.0, 1.0, 1.5];
    const COLORS: [u32; 4] = [0, 1, 1, 2];
    const BLACK: f32 = 100.0;
    const WHITE: f32 = 4000.0;

    /// RGGB mosaic of a camera-native scene `f(u, v) → rgb` (values past 1
    /// clip, like a sensor), and the analysis of it.
    fn analyse_scene(w: usize, h: usize, f: impl Fn(f32, f32) -> [f32; 3]) -> HighlightModel {
        let mut mosaic = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                let c = COLORS[(y & 1) * 2 + (x & 1)] as usize;
                let v = f(x as f32 / w as f32, y as f32 / h as f32)[c].clamp(0.0, 1.0);
                mosaic[y * w + x] = (BLACK + v * (WHITE - BLACK)).round() as u16;
            }
        }
        let scale = std::array::from_fn(|k| WB[COLORS[k] as usize] / (WHITE - BLACK));
        analyse(&mosaic, w, h, COLORS, [BLACK; 4], scale, WB)
    }

    /// The correction a camera-native colour has (what analysis learns).
    fn correction_of(nat: [f32; 3]) -> [f32; 3] {
        let l = nat.map(f32::ln);
        std::array::from_fn(|c| l[c] - 0.5 * (l[(c + 1) % 3] + l[(c + 2) % 3]))
    }

    /// The warm–cool part of a correction (what analysis keeps).
    fn warmth(k: [f32; 3]) -> f32 {
        (k[0] - k[2]) * 0.5
    }

    fn balanced(nat: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|c| nat[c] * WB[c])
    }

    /// A camera-native sky colour: white-balanced it is a light blue
    /// (r < g < b), and green is its brightest native channel, then blue —
    /// so green clips first, blue second, red last: the purple case.
    const SKY: [f32; 3] = [0.42, 1.0, 0.80];

    fn sky(u: f32) -> [f32; 3] {
        // Brightening to the left, past green's and blue's clip points.
        let gain = 0.55 + 1.0 * (1.0 - u);
        SKY.map(|c| c * gain)
    }

    #[test]
    fn grey_needs_no_correction_under_its_own_white_balance() {
        let k = correction_under(grey_correction(WB), WB);
        assert!(k.iter().all(|v| v.abs() < 1e-6), "{k:?}");
        // …and the correction of a native grey is what `grey_correction` says.
        let g = correction_of([1.0 / WB[0], 1.0, 1.0 / WB[2]]);
        let want = grey_correction(WB);
        assert!((0..3).all(|c| (g[c] - want[c]).abs() < 1e-5), "{g:?} vs {want:?}");
    }

    #[test]
    fn a_scene_without_clipping_learns_nothing() {
        let m = analyse_scene(256, 192, |u, v| [0.3 * u, 0.5 * v, 0.2]);
        assert_eq!(m, HighlightModel::neutral(WB));
    }

    #[test]
    fn the_rim_of_a_clipped_sky_teaches_its_colour() {
        let m = analyse_scene(512, 256, |u, _| sky(u));
        let k = m.correction;
        assert!((warmth(k) - warmth(correction_of(SKY))).abs() < 0.02, "{k:?}");
        // Green stays on the locus, whatever the rim says.
        assert!((k[1] - grey_correction(WB)[1]).abs() < 1e-6, "{k:?}");
    }

    #[test]
    fn a_green_and_blue_clipped_sky_rebuilds_in_its_own_colour() {
        // Deep in the clipped part: red holds data, green and blue clip.
        let truth = SKY.map(|c| c * 1.4);
        let measured = balanced(truth.map(|c| c.min(1.0)));
        let m = analyse_scene(512, 256, |u, _| sky(u));
        let k = correction_under(m.correction, WB);
        let out = reconstruct(measured, WB, k);
        let want = balanced(truth);
        for c in [0, 2] {
            let (got, w) = (out[c] / out[1], want[c] / want[1]);
            assert!((got / w - 1.0).abs() < 0.04, "channel {c}/g: {got} vs {w} ({out:?})");
        }
    }

    #[test]
    fn without_evidence_two_clipped_channels_still_come_out_grey_not_magenta() {
        // Grey under WB, bright enough that green and blue clip and red not.
        let truth = [1.3 / WB[0], 1.3, 1.3 / WB[2]];
        let measured = balanced(truth.map(|c| c.min(1.0)));
        let out = reconstruct(measured, WB, [0.0; 3]);
        // The old rule gave green = red < blue (lavender). Now: grey.
        assert!((out[1] / out[0] - 1.0).abs() < 0.02, "{out:?}");
        assert!((out[2] / out[0] - 1.0).abs() < 0.02, "{out:?}");
    }

    #[test]
    fn rebuilding_only_ever_raises_a_channel() {
        let k = correction_under(correction_of(SKY), WB);
        for nat in [[0.95, 0.3, 0.2], [1.0, 1.0, 0.1], [0.92, 0.97, 0.99], [1.0, 1.0, 1.0], [0.2, 1.0, 0.95]] {
            let m = balanced(nat);
            let out = reconstruct(m, WB, k);
            for c in 0..3 {
                assert!(out[c] >= m[c] * (1.0 - 1e-5), "{m:?} → {out:?}");
            }
        }
    }

    #[test]
    fn fully_clipped_renders_white() {
        // Without evidence: white at the brightest ceiling, as before.
        let out = reconstruct(WB, WB, [0.0; 3]);
        for c in 0..3 {
            assert!((out[c] / 2.0 - 1.0).abs() < 0.03, "{out:?}");
        }
        // With a learned sky colour: still white, at least that bright.
        let k = correction_under(correction_of(SKY), WB);
        let out = reconstruct(WB, WB, k);
        for c in 0..3 {
            assert!((out[c] / out[0] - 1.0).abs() < 1e-4 && out[c] >= 2.0, "{out:?}");
        }
    }

    #[test]
    fn rebuilding_is_continuous_as_channels_clip() {
        let k = correction_under(correction_of(SKY), WB);
        // Brighten the sky through all three clip points.
        let mut prev = reconstruct(balanced(SKY.map(|c| (c * 0.8).min(1.0))), WB, k);
        for i in 1..=400 {
            let gain = 0.8 + 2.0 * i as f32 / 400.0;
            let out = reconstruct(balanced(SKY.map(|c| (c * gain).min(1.0))), WB, k);
            for c in 0..3 {
                assert!(out[c] >= prev[c] - 1e-4, "darkened at gain {gain}: {prev:?} → {out:?}");
                assert!((out[c] / prev[c] - 1.0).abs() < 0.03, "jump at gain {gain}: {prev:?} → {out:?}");
            }
            prev = out;
        }
    }

    #[test]
    fn bright_subjects_away_from_clipping_are_not_evidence() {
        // The sky clips on the left; a warm, bright (but unclipped) subject
        // sits far off on the right and must not pull the correction.
        let warm = [0.75f32, 0.85, 0.45];
        let m = analyse_scene(1024, 256, |u, _| if u < 0.5 { sky(u / 0.5) } else if u > 0.85 { warm } else { [0.02, 0.04, 0.03] });
        assert!((warmth(m.correction) - warmth(correction_of(SKY))).abs() < 0.02, "{:?}", m.correction);
    }
}
