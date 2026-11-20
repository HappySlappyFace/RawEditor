//! Dehaze, CPU half: everything about an image's haze that does not depend on
//! the slider, estimated once at load from the mosaic (like `core::noise`).
//!
//! ## Model
//! Haze is light scattered into the line of sight (Koschmieder): a pixel
//! whose haze-free radiance is `J` arrives as `I = J·t + A·(1 − t)`, where
//! `A` is the airlight (the colour of the haze itself) and `t` the fraction
//! of `J` that survives the distance. It is linear in radiance, so it is
//! inverted in the colour pass on scene-linear camera values, where it is
//! physically exact — and unaffected by exposure or white balance, because
//! everything below is measured on channels normalised by `A`, which both
//! scale identically.
//!
//! ## Estimation
//! **Dark channel prior** (He, Sun & Tang, 2009): in haze-free scenes almost
//! every patch holds something dark in at least one channel — a shadow, a
//! saturated colour. So the darkest normalised channel over a patch,
//! `dc = min_patch min_c I_c/A_c`, reads `1 − t` directly: haze is what lifts
//! it. The airlight is the colour of the patches where that lift is largest.
//!
//! The patch minimum is blocky and bleeds past object outlines, so it is
//! refined by a **fast guided filter** (He & Sun, 2015): on the coarse grid,
//! fit `dc ≈ a·G + b` locally, where the guide `G` is the mean normalised
//! channel. Only `(a, b)` are kept. The colour pass evaluates `a·G + b` with
//! each pixel's OWN `G`, so the map snaps to full-resolution edges — the
//! skyline stays sharp — at the cost of one texture fetch per pixel.
//!
//! Everything works on a grid of at most `GRID` cells along the long edge,
//! laid over UV rather than pixels, so the preview and the full-resolution
//! export analyse the same areas and dehaze the same way.

/// Cells along the image's long edge.
pub const GRID: usize = 160;
/// Each cell is split `SUB`×`SUB`; the dark channel takes the minimum over
/// these sub-blocks, averaged rather than single pixels so that noise
/// cannot fake a dark pixel.
const SUB: usize = 4;
/// Quads (2×2 Bayer sites) sampled per sub-block axis, at most.
const MAX_QUADS: usize = 4;
/// Dark-channel patch radius in cells: 5×5 cells ≈ 3% of the long edge,
/// the proportion of He et al.'s 15 px patch on 600 px images.
const DC_RADIUS: usize = 2;
/// Guided-filter window radius in cells, and its regulariser (in units of
/// the normalised guide squared): edges with less contrast than ~√ε in `G`
/// are smoothed through rather than followed.
const GF_RADIUS: usize = 6;
const GF_EPS: f32 = 2e-3;
/// Fraction of cells (the haziest) whose mean colour is the airlight.
const AIRLIGHT_FRACTION: f32 = 0.002;

/// What the colour pass needs to dehaze: the airlight and, per grid cell,
/// the guided-filter coefficients of the dark channel.
#[derive(Debug, Clone, PartialEq)]
pub struct HazeModel {
    /// Airlight in the analysis space (white-balanced unit range, the same
    /// units `core::noise` uses).
    pub airlight: [f32; 3],
    pub width: usize,
    pub height: usize,
    /// Row-major `(a, b)`: dark channel ≈ `a·G + b`, with `G` the mean of
    /// the airlight-normalised channels.
    pub coeffs: Vec<[f32; 2]>,
}

impl Default for HazeModel {
    /// No haze anywhere (dark channel 0): dehaze is then a no-op.
    fn default() -> Self {
        Self { airlight: [1.0; 3], width: 1, height: 1, coeffs: vec![[0.0, 0.0]] }
    }
}

impl HazeModel {
    /// The dark channel the colour pass will compute for guide value `g` in
    /// cell `(x, y)` — the CPU mirror of the shader, for tests.
    #[cfg(test)]
    pub fn dark_channel(&self, x: usize, y: usize, g: f32) -> f32 {
        let [a, b] = self.coeffs[y * self.width + x];
        (a * g + b).clamp(0.0, 1.0)
    }
}

/// Mean over a `(2r+1)²` window, shrunk at the borders (each output divides
/// by the number of cells it actually covered).
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        let row = &src[y * w..][..w];
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = row[x0..=x1].iter().sum::<f32>() / (x1 - x0 + 1) as f32;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            let s: f32 = (y0..=y1).map(|yy| tmp[yy * w + x]).sum();
            out[y * w + x] = s / (y1 - y0 + 1) as f32;
        }
    }
    out
}

/// Minimum over a `(2r+1)²` window (separable — exact for a square).
fn min_filter(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = src[y * w + x0..=y * w + x1].iter().copied().fold(f32::MAX, f32::min);
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (y0..=y1).map(|yy| tmp[yy * w + x]).fold(f32::MAX, f32::min);
        }
    }
    out
}

/// Analyse a Bayer mosaic. `colors`, `black` and `scale` are per 2×2 tile
/// position exactly as `gpu::demosaic` uses them (see `core::noise::estimate`).
pub fn analyse(
    mosaic: &[u16],
    width: usize,
    height: usize,
    colors: [u32; 4],
    black: [f32; 4],
    scale: [f32; 4],
) -> HazeModel {
    let (qw, qh) = (width / 2, height / 2);
    if qw < 16 || qh < 16 || mosaic.len() < width * height {
        return HazeModel::default();
    }
    let (gw, gh) = if qw >= qh {
        (GRID.min(qw / SUB), ((GRID * qh) as f32 / qw as f32).round().max(1.0) as usize)
    } else {
        (((GRID * qw) as f32 / qh as f32).round().max(1.0) as usize, GRID.min(qh / SUB))
    };
    let (gw, gh) = (gw.max(1).min(qw / SUB), gh.max(1).min(qh / SUB));
    let n = gw * gh;

    // Per cell: mean colour, and per-channel minimum over its sub-blocks.
    let mut mean = vec![[0.0f32; 3]; n];
    let mut low = vec![[0.0f32; 3]; n];
    let (sw, sh) = (gw * SUB, gh * SUB);
    for cy in 0..gh {
        for cx in 0..gw {
            let mut cell_sum = [0.0f32; 3];
            let mut cell_low = [f32::MAX; 3];
            for sy in 0..SUB {
                for sx in 0..SUB {
                    // Sub-block's quad range, over UV so every resolution
                    // covers the same part of the image.
                    let (bx, by) = (cx * SUB + sx, cy * SUB + sy);
                    let (qx0, qx1) = (bx * qw / sw, ((bx + 1) * qw / sw).max(bx * qw / sw + 1));
                    let (qy0, qy1) = (by * qh / sh, ((by + 1) * qh / sh).max(by * qh / sh + 1));
                    let (nx, ny) = ((qx1 - qx0).min(MAX_QUADS), (qy1 - qy0).min(MAX_QUADS));
                    let mut sum = [0.0f32; 3];
                    let mut cnt = [0.0f32; 3];
                    for j in 0..ny {
                        let qy = qy0 + j * (qy1 - qy0) / ny;
                        for i in 0..nx {
                            let qx = qx0 + i * (qx1 - qx0) / nx;
                            for k in 0..4 {
                                let (x, y) = (2 * qx + (k & 1), 2 * qy + (k >> 1));
                                let c = colors[k] as usize;
                                sum[c] += (mosaic[y * width + x] as f32 - black[k]) * scale[k];
                                cnt[c] += 1.0;
                            }
                        }
                    }
                    for c in 0..3 {
                        let v = (sum[c] / cnt[c].max(1.0)).max(0.0);
                        cell_sum[c] += v;
                        cell_low[c] = cell_low[c].min(v);
                    }
                }
            }
            let i = cy * gw + cx;
            mean[i] = cell_sum.map(|s| s / (SUB * SUB) as f32);
            low[i] = cell_low;
        }
    }

    // Airlight: among the cells whose (unnormalised) dark channel is highest
    // — the haziest, most distant part of the frame — the brightest. He et
    // al. take the single brightest pixel; averaging the brightest quarter
    // of the candidates keeps one specular cell from setting it. Brightness
    // matters most when there is little haze: the dark channel is then
    // nearly level, and an average of arbitrary candidates would be a dim
    // colour that makes everything else look hazy by comparison.
    let dark0 = min_filter(&low.iter().map(|l| l[0].min(l[1]).min(l[2])).collect::<Vec<_>>(), gw, gh, DC_RADIUS);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_unstable_by(|&a, &b| dark0[b].total_cmp(&dark0[a]));
    let top = ((n as f32 * AIRLIGHT_FRACTION).ceil() as usize).clamp(16, n);
    let candidates = &mut order[..top];
    let brightness = |i: usize| mean[i][0] + mean[i][1] + mean[i][2];
    candidates.sort_unstable_by(|&a, &b| brightness(b).total_cmp(&brightness(a)));
    let pick = &candidates[..(top / 4).max(1)];
    let mut airlight = [0.0f32; 3];
    for &i in pick {
        for c in 0..3 {
            airlight[c] += mean[i][c] / pick.len() as f32;
        }
    }
    let peak = airlight[0].max(airlight[1]).max(airlight[2]);
    if peak < 1e-4 {
        return HazeModel::default();
    }
    // A channel the haze barely has (a deep-blue dusk, say) would make
    // every normalised value in it explode; floor it relative to the rest.
    let airlight = airlight.map(|a| a.max(peak * 0.05));

    // Dark channel and guide, both on the airlight-normalised channels.
    let dc: Vec<f32> = min_filter(
        &low.iter().map(|l| (0..3).map(|c| l[c] / airlight[c]).fold(f32::MAX, f32::min)).collect::<Vec<_>>(),
        gw,
        gh,
        DC_RADIUS,
    )
    .into_iter()
    .map(|d| d.clamp(0.0, 1.0))
    .collect();
    let guide: Vec<f32> = mean.iter().map(|m| (0..3).map(|c| m[c] / airlight[c]).sum::<f32>() / 3.0).collect();

    // Guided filter (He & Sun): local linear fit of dc against the guide.
    let r = GF_RADIUS;
    let mean_g = box_mean(&guide, gw, gh, r);
    let mean_p = box_mean(&dc, gw, gh, r);
    let corr_gg = box_mean(&guide.iter().map(|g| g * g).collect::<Vec<_>>(), gw, gh, r);
    let corr_gp = box_mean(&guide.iter().zip(&dc).map(|(g, p)| g * p).collect::<Vec<_>>(), gw, gh, r);
    let mut a = vec![0.0f32; n];
    let mut b = vec![0.0f32; n];
    for i in 0..n {
        let var = (corr_gg[i] - mean_g[i] * mean_g[i]).max(0.0);
        let cov = corr_gp[i] - mean_g[i] * mean_p[i];
        a[i] = cov / (var + GF_EPS);
        b[i] = mean_p[i] - a[i] * mean_g[i];
    }
    let (ma, mb) = (box_mean(&a, gw, gh, r), box_mean(&b, gw, gh, r));
    HazeModel {
        airlight,
        width: gw,
        height: gh,
        coeffs: ma.into_iter().zip(mb).map(|(a, b)| [a, b]).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WB: [f32; 3] = [2.0, 1.0, 1.5];
    const COLORS: [u32; 4] = [0, 1, 1, 2];

    /// RGGB mosaic of a white-balanced scene `f(x, y) → rgb`, noise-free,
    /// and the analysis of it.
    fn analyse_scene(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> HazeModel {
        let (white, blk) = (16383.0f32, 512.0f32);
        let mut m = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                let k = (y & 1) * 2 + (x & 1);
                let c = COLORS[k] as usize;
                let v = (f(x, y)[c] / WB[c]).clamp(0.0, 1.0);
                m[y * w + x] = (blk + v * (white - blk)).round() as u16;
            }
        }
        let scale = std::array::from_fn(|k| WB[COLORS[k] as usize] / (white - blk));
        analyse(&m, w, h, COLORS, [blk; 4], scale)
    }

    /// Haze-free content in the spirit of the prior: patches of saturated
    /// colour with dark gaps between them.
    fn clear_scene(x: usize, y: usize) -> [f32; 3] {
        let k = ((x / 12) * 7 + (y / 12) * 13) % 4;
        if (x % 12) < 3 || (y % 12) < 3 {
            return [0.02, 0.02, 0.02];
        }
        [[0.6, 0.08, 0.05], [0.07, 0.45, 0.1], [0.08, 0.1, 0.5], [0.5, 0.45, 0.06]][k]
    }

    const A: [f32; 3] = [0.75, 0.8, 0.85];

    /// Transmission falling towards the top of the frame, as distance does,
    /// down to a band of pure haze (sky) at the very top.
    fn transmission(y: usize, h: usize) -> f32 {
        let v = y as f32 / h as f32;
        if v < 0.1 {
            0.0
        } else {
            0.25 + 0.7 * (v - 0.1) / 0.9
        }
    }

    fn hazy(x: usize, y: usize, h: usize) -> [f32; 3] {
        let t = transmission(y, h);
        let j = clear_scene(x, y);
        std::array::from_fn(|c| j[c] * t + A[c] * (1.0 - t))
    }

    #[test]
    fn finds_the_airlight() {
        let (w, h) = (1200, 800);
        let m = analyse_scene(w, h, |x, y| hazy(x, y, h));
        for (c, (&est, &truth)) in m.airlight.iter().zip(&A).enumerate() {
            let err = (est - truth).abs() / truth;
            assert!(err < 0.05, "channel {c}: airlight {est:.3} vs {truth:.3}");
        }
    }

    /// The refined dark channel must read `1 − t` across the frame. Only
    /// where the patch minimum reaches across the sky line into featureless
    /// haze — which gives the guide nothing to follow — may it lag: the sky
    /// just above a horizon is dehazed a little less than the rest.
    #[test]
    fn dark_channel_tracks_the_transmission() {
        let (w, h) = (1200, 800);
        let m = analyse_scene(w, h, |x, y| hazy(x, y, h));
        let sky_line = m.height / 10;
        let (mut worst, mut worst_at_edge) = (0.0f32, 0.0f32);
        for cy in 2..m.height - 2 {
            for cx in 2..m.width - 2 {
                // Guide averaged over a few of the cell's pixels, as the
                // colour pass sees it on average.
                let (x, y) = ((cx * 2 + 1) * w / (2 * m.width), (cy * 2 + 1) * h / (2 * m.height));
                let mut g = 0.0;
                for (dy, dx) in [(0, 0), (4, 4), (8, 8), (2, 10)] {
                    let p = hazy(x + dx, y + dy, h);
                    g += (0..3).map(|c| p[c] / m.airlight[c]).sum::<f32>() / 12.0;
                }
                let e = (m.dark_channel(cx, cy, g) - (1.0 - transmission(y, h))).abs();
                if cy.abs_diff(sky_line) <= DC_RADIUS + 2 {
                    worst_at_edge = worst_at_edge.max(e);
                } else {
                    worst = worst.max(e);
                }
            }
        }
        assert!(worst < 0.06, "dark channel off by up to {worst:.3}");
        assert!(worst_at_edge < 0.25, "dark channel off by {worst_at_edge:.3} at the sky line");
    }

    #[test]
    fn a_clear_scene_has_no_haze() {
        let m = analyse_scene(1200, 800, clear_scene);
        let mean: f32 = (0..m.height)
            .flat_map(|y| (0..m.width).map(move |x| (x, y)))
            .map(|(x, y)| {
                let [a, b] = m.coeffs[y * m.width + x];
                (a * 0.3 + b).clamp(0.0, 1.0)
            })
            .sum::<f32>()
            / (m.width * m.height) as f32;
        assert!(mean < 0.1, "clear scene reads as hazy: mean dark channel {mean:.3}");
    }

    /// The develop preview (≥1280 px wide) and the full-resolution export
    /// must analyse the same grid, or they would dehaze differently.
    #[test]
    fn grid_follows_the_aspect_and_not_the_resolution() {
        let full = analyse_scene(3840, 2400, |x, y| hazy(x / 3, y / 3, 800));
        let preview = analyse_scene(1280, 800, |x, y| hazy(x, y, 800));
        assert_eq!((full.width, full.height), (GRID, GRID * 5 / 8));
        assert_eq!((preview.width, preview.height), (full.width, full.height));
        for c in 0..3 {
            assert!((full.airlight[c] - preview.airlight[c]).abs() < 0.02);
        }
    }

    #[test]
    fn tiny_or_black_inputs_fall_back_to_no_haze() {
        assert_eq!(analyse(&[0u16; 64], 8, 8, COLORS, [0.0; 4], [1.0; 4]), HazeModel::default());
        let m = analyse(&vec![512u16; 256 * 256], 256, 256, COLORS, [512.0; 4], [1e-4; 4]);
        assert_eq!(m, HazeModel::default());
    }
}
