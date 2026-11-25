//! Per-image sensor noise model, estimated from the mosaic itself.
//!
//! Raw sensor noise is well described as Poisson-Gaussian: a pixel recording
//! signal `x` has variance `a·x + b` — shot noise grows with the signal, read
//! noise is a floor. `a` and `b` change with ISO, sensor and (for previews)
//! with how much the mosaic was box-filtered, so a fixed denoise threshold is
//! too strong on a base-ISO frame and too weak at ISO 3200. Knowing the model
//! lets the denoiser work in a **variance-stabilised** space (generalised
//! Anscombe transform) where every pixel's noise is ≈ 1, so its thresholds
//! are expressed in units of the actual noise and one slider position means
//! the same thing on every image.
//!
//! ## Estimation
//! The hard part is not measuring variance but measuring *only noise*: every
//! high-pass filter also passes scene texture, and a model that counts
//! foliage or brickwork as noise makes every NR slider over-smooth by the
//! same factor. (The previous estimator — a median of second differences per
//! brightness bin — read a noise-free photo of a city as roughly ISO 10000.)
//!
//! So the variance is measured per small block and only the *quietest*
//! blocks are believed:
//! 1. Each CFA plane (one colour, every other pixel) is cut into
//!    `BLOCK`×`BLOCK` blocks. Within a block, Immerkær's 3×3 mask
//!    (`[1 −2 1; −2 4 −2; 1 −2 1]`) cancels any quadratic surface, so a
//!    smooth gradient contributes nothing; what is left is noise plus texture,
//!    with `E[r²] = 36σ²` for pure noise.
//! 2. Blocks are binned by their mean level. Texture can only *add* variance,
//!    so the flat blocks (sky, walls, out-of-focus areas) form the bottom of
//!    each bin's distribution. A low quantile is a first guess, but a biased
//!    one — if only 20% of blocks are flat, the 10% quantile is their
//!    *median*. So it is refined as a truncated mean: average the blocks
//!    below `TRUNC_TAU`·σ², correct by that truncation's known effect on
//!    pure noise (`TRUNC_MEAN`), repeat. σ² is a fixed point of that
//!    iteration for any fraction of flat blocks, and textured blocks sitting
//!    above the cut never enter it.
//! 3. A weighted fit of `var = a·x + b` through the bins, refitted after
//!    dropping bins that sit far *above* the line — a bin with no flat block
//!    at all is still inflated, and only ever upwards.
//!
//! Values stay signed (black-subtracted, not clamped): read noise straddles
//! zero, and clamping it would under-read `b` in exactly the bins that set it.

/// `variance(x) = a·x + b` per colour (R, G, B), in white-balanced unit-range
/// units — the same units the demosaic writes before un-balancing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseModel {
    pub a: [f32; 3],
    pub b: [f32; 3],
}

impl Default for NoiseModel {
    /// A plausible low-ISO model, used when estimation has too little data.
    fn default() -> Self {
        Self { a: [6e-5; 3], b: [2e-7; 3] }
    }
}

impl NoiseModel {
    /// Coefficients of the generalised Anscombe transform for colour `c`:
    /// `s(x) = k · sqrt(max(a·x + c0, 0))`, giving unit noise variance.
    /// Returned as `(a, c0, k)` for the shader.
    pub fn vst(&self, c: usize) -> (f32, f32, f32) {
        let a = self.a[c].max(1e-7);
        let b = self.b[c].max(0.0);
        (a, 0.375 * a * a + b, 2.0 / a)
    }
}

/// Block side in samples of one CFA plane (twice that in mosaic pixels).
const BLOCK: usize = 8;
/// Brightness bins per colour, uniform in `sqrt(level)` so the shadows —
/// where `b` is decided and most of a photo usually sits — get as many bins
/// as the highlights.
const BINS: usize = 24;
/// Blocks a bin needs before its quantile is trusted.
const MIN_BIN_BLOCKS: usize = 24;
/// The quantile of a bin's block variances used as the starting guess…
const QUANTILE: f64 = 0.1;
/// …and that quantile on pure white noise, as a fraction of the true
/// variance (Monte Carlo over 250k blocks).
const QUANTILE_RATIO: f64 = 0.536;
/// Truncated-mean refinement: blocks below `TRUNC_TAU`·σ² are averaged…
const TRUNC_TAU: f64 = 1.5;
/// …and on pure noise that average is `TRUNC_MEAN`·σ² (Monte Carlo; the
/// pure-noise test below fails if either constant drifts).
const TRUNC_MEAN: f64 = 0.890;
/// Upper bound on blocks examined per CFA plane: plenty for the fit, and keeps
/// a 45 MP frame to a few tens of milliseconds.
const MAX_BLOCKS_PER_PLANE: usize = 40_000;

/// Estimate the noise model of a Bayer mosaic.
///
/// `colors`, `black` and `scale` are per 2×2 tile position
/// (`k = (y & 1) * 2 + (x & 1)`), exactly as `gpu::demosaic` uses them, so the
/// model describes the white-balanced values the denoiser will see. `ceil` is
/// each colour's full-scale white-balanced value (its WB multiplier).
pub fn estimate(
    mosaic: &[u16],
    width: usize,
    height: usize,
    colors: [u32; 4],
    black: [f32; 4],
    scale: [f32; 4],
    ceil: [f32; 3],
) -> NoiseModel {
    let (pw, ph) = (width / 2, height / 2);
    let (nbx, nby) = (pw / BLOCK, ph / BLOCK);
    if nbx < 4 || nby < 4 || mosaic.len() < width * height {
        return NoiseModel::default();
    }
    // Visit every `skip`-th block along each axis to cap the work.
    let skip = ((nbx * nby) as f64 / MAX_BLOCKS_PER_PLANE as f64).sqrt().ceil().max(1.0) as usize;

    // Per colour, per bin: block variances and the sum of block means.
    let mut vars: Vec<Vec<Vec<f32>>> = vec![vec![Vec::new(); BINS]; 3];
    let mut means: Vec<Vec<f64>> = vec![vec![0.0; BINS]; 3];
    let inner = (BLOCK - 2) * (BLOCK - 2);
    let mut blk = [[0.0f32; BLOCK]; BLOCK];

    for k in 0..4 {
        let (ox, oy) = (k & 1, k >> 1);
        let c = colors[k] as usize;
        let range = ceil[c].max(1e-6);
        for by in (0..nby).step_by(skip) {
            for bx in (0..nbx).step_by(skip) {
                let (mut sum, mut peak) = (0.0f32, f32::MIN);
                for (j, row) in blk.iter_mut().enumerate() {
                    let y = 2 * (by * BLOCK + j) + oy;
                    for (i, v) in row.iter_mut().enumerate() {
                        let x = 2 * (bx * BLOCK + i) + ox;
                        *v = (mosaic[y * width + x] as f32 - black[k]) * scale[k];
                        sum += *v;
                        peak = peak.max(*v);
                    }
                }
                let mean = sum / (BLOCK * BLOCK) as f32;
                // Anything touching the clip ceiling has had its noise cut
                // off; the top of the range is clipping-compressed anyway.
                if peak >= range * 0.95 || mean >= range * 0.9 {
                    continue;
                }
                let mut r2 = 0.0f32;
                for j in 1..BLOCK - 1 {
                    for i in 1..BLOCK - 1 {
                        let r = blk[j - 1][i - 1] - 2.0 * blk[j - 1][i] + blk[j - 1][i + 1]
                            - 2.0 * blk[j][i - 1] + 4.0 * blk[j][i] - 2.0 * blk[j][i + 1]
                            + blk[j + 1][i - 1] - 2.0 * blk[j + 1][i] + blk[j + 1][i + 1];
                        r2 += r * r;
                    }
                }
                let bin = ((mean.max(0.0) / range).sqrt() * BINS as f32) as usize;
                let bin = bin.min(BINS - 1);
                vars[c][bin].push(r2 / (36 * inner) as f32);
                means[c][bin] += mean as f64;
            }
        }
    }

    let mut model = NoiseModel::default();
    for c in 0..3 {
        // Quantisation noise (1/12 DN²) in this colour's units: a bin cannot
        // honestly be quieter than that, and a dead-flat one (black-clipped
        // border, constant synthetic area) would otherwise read zero.
        let floor = (0..4)
            .filter(|&k| colors[k] as usize == c)
            .map(|k| (scale[k] as f64).powi(2) / 12.0)
            .fold(f64::INFINITY, f64::min);
        let floor = if floor.is_finite() { floor } else { 0.0 };
        // (mean, variance, blocks) per usable bin.
        let mut pts: Vec<(f64, f64, f64)> = Vec::new();
        for bin in 0..BINS {
            let v = &mut vars[c][bin];
            let n = v.len();
            if n < MIN_BIN_BLOCKS {
                continue;
            }
            pts.push((means[c][bin] / n as f64, flat_variance(v).max(floor), n as f64));
        }
        if let Some((a, b)) = fit_lower_envelope(&pts) {
            model.a[c] = a as f32;
            model.b[c] = b as f32;
        }
    }
    model
}

/// Noise variance of one bin's blocks: the low quantile as a first guess,
/// then the truncated mean of the blocks below `TRUNC_TAU` times the current
/// estimate until it settles (see the module docs).
fn flat_variance(v: &mut [f32]) -> f64 {
    let q = ((v.len() as f64 * QUANTILE) as usize).min(v.len() - 1);
    let (_, low, _) = v.select_nth_unstable_by(q, |a, b| a.total_cmp(b));
    let mut s = *low as f64 / QUANTILE_RATIO;
    for _ in 0..12 {
        let cut = TRUNC_TAU * s;
        let (sum, n) = v
            .iter()
            .filter(|&&x| (x as f64) < cut)
            .fold((0.0f64, 0usize), |(sum, n), &x| (sum + x as f64, n + 1));
        if n < MIN_BIN_BLOCKS / 2 {
            break;
        }
        let next = sum / n as f64 / TRUNC_MEAN;
        let settled = (next - s).abs() <= 1e-3 * s;
        s = next;
        if settled {
            break;
        }
    }
    s
}

/// Fit `var = a·mean + b` through the bins, then drop bins well above the
/// line and refit: a bin with no flat block at all is still inflated by
/// texture, and texture only ever pushes upwards.
///
/// Weighted by `blocks / var²` — a quantile's error is proportional to the
/// variance it measures, so this weights each bin by its relative precision
/// instead of letting the brightest bins dominate. Returns `None` with fewer
/// than two usable bins.
const MAX_WEIGHT_RATIO: f64 = 1e6;

fn fit_lower_envelope(pts: &[(f64, f64, f64)]) -> Option<(f64, f64)> {
    let mut keep: Vec<(f64, f64, f64)> =
        pts.iter().map(|&(m, v, n)| (m, v, n / (v * v).max(1e-30))).collect();
    // No bin may outweigh the least certain one by more than `MAX_WEIGHT_RATIO`,
    // so a freak near-zero variance cannot decide the fit on its own.
    let wmin = keep.iter().map(|p| p.2).fold(f64::INFINITY, f64::min);
    for p in &mut keep {
        p.2 = p.2.min(wmin * MAX_WEIGHT_RATIO);
    }
    let mut fit = fit_affine(&keep)?;
    for _ in 0..3 {
        let kept: Vec<_> = keep
            .iter()
            .copied()
            .filter(|&(m, v, _)| v <= (fit.0 * m + fit.1).max(1e-12) * 1.5)
            .collect();
        if kept.len() == keep.len() || kept.len() < 2 {
            break;
        }
        keep = kept;
        fit = fit_affine(&keep)?;
    }
    Some(fit)
}

/// Weighted least squares for `var = a·mean + b`, with `a ≥ 0`, `b ≥ 0`.
/// Returns `None` with fewer than two points.
fn fit_affine(pts: &[(f64, f64, f64)]) -> Option<(f64, f64)> {
    if pts.len() < 2 {
        return None;
    }
    let sw: f64 = pts.iter().map(|p| p.2).sum();
    let mx = pts.iter().map(|p| p.0 * p.2).sum::<f64>() / sw;
    let my = pts.iter().map(|p| p.1 * p.2).sum::<f64>() / sw;
    let sxx: f64 = pts.iter().map(|p| p.2 * (p.0 - mx).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|p| p.2 * (p.0 - mx) * (p.1 - my)).sum();
    if sxx <= 0.0 {
        return None;
    }
    let mut a = sxy / sxx;
    let mut b = my - a * mx;
    if a < 0.0 {
        // Pure read noise (or a broken fit): flat variance.
        a = 1e-7;
        b = my;
    }
    if b < 0.0 {
        // Read noise below what the fit can resolve: refit through origin.
        let sxy0: f64 = pts.iter().map(|p| p.2 * p.0 * p.1).sum();
        let sxx0: f64 = pts.iter().map(|p| p.2 * p.0 * p.0).sum();
        a = (sxy0 / sxx0).max(1e-7);
        b = 0.0;
    }
    Some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64* — deterministic, dependency-free noise for the tests.
    struct Rng(u64);
    impl Rng {
        fn next_f64(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f64 {
            let u1 = self.next_f64().max(1e-12);
            let u2 = self.next_f64();
            (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
        }
    }

    const WB: [f32; 3] = [2.0, 1.0, 1.5];

    /// An RGGB mosaic of `signal(x, y)` (unit range, unbalanced) with
    /// Poisson-Gaussian noise of variance `a·signal + b`, estimated the way
    /// the GPU path calls it.
    fn estimate_synthetic(
        (w, h): (usize, usize),
        (a_true, b_true): (f64, f64),
        signal: impl Fn(usize, usize) -> f64,
    ) -> NoiseModel {
        let (white, blk) = (16383.0f64, 600.0f64);
        let colors = [0u32, 1, 1, 2];
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut mosaic = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                let s = signal(x, y);
                let sd = (a_true * s + b_true).sqrt();
                let v = (s + sd * rng.gauss()).clamp(-0.03, 1.0);
                mosaic[y * w + x] = (blk + v * (white - blk)).round() as u16;
            }
        }
        let black = [blk as f32; 4];
        let mut scale = [0.0f32; 4];
        for k in 0..4 {
            scale[k] = WB[colors[k] as usize] / (white as f32 - blk as f32);
        }
        estimate(&mosaic, w, h, colors, black, scale, WB)
    }

    /// White balance multiplies values by wb, so variance scales by wb²: in
    /// balanced units `a_c = wb·a` and `b_c = wb²·b`. Returns the worst
    /// relative error of `a` and of the variance at `probe` (unit range).
    fn errors(m: &NoiseModel, (a_true, b_true): (f64, f64), probe: f64) -> (f64, f64) {
        let (mut ea, mut ev) = (0.0f64, 0.0f64);
        for (c, &wbc) in WB.iter().enumerate() {
            let wbc = wbc as f64;
            let (a_exp, b_exp) = (a_true * wbc, b_true * wbc * wbc);
            let x = probe * wbc;
            let v_exp = a_exp * x + b_exp;
            let v_est = m.a[c] as f64 * x + m.b[c] as f64;
            eprintln!(
                "c{c}: a {:.3e} (expect {a_exp:.3e}), b {:.3e} (expect {b_exp:.3e})",
                m.a[c], m.b[c]
            );
            if a_exp > 0.0 {
                ea = ea.max((m.a[c] as f64 - a_exp).abs() / a_exp);
            }
            ev = ev.max((v_est - v_exp).abs() / v_exp);
        }
        (ea, ev)
    }

    /// Smooth content with known noise: the model must come back.
    #[test]
    fn recovers_a_known_noise_model() {
        let truth = (4e-4, 4e-6);
        let m = estimate_synthetic((1200, 800), truth, |x, y| {
            // Smooth ramps spanning most of the range, different per axis.
            0.03 + 0.85 * ((x as f64 / 1200.0) * 0.7 + (y as f64 / 800.0) * 0.3)
        });
        let (ea, ev) = errors(&m, truth, 0.05);
        assert!(ea < 0.15, "a off by {:.0}%", ea * 100.0);
        // b is small next to a·x over most of the range; judge it through the
        // total variance at the dark end of the data, where it matters.
        assert!(ev < 0.2, "shadow variance off by {:.0}%", ev * 100.0);
    }

    /// Pure read noise (no shot noise): only `b`. This is also the check on
    /// `QUANTILE_RATIO` — a wrong constant scales every estimate.
    #[test]
    fn quantile_ratio_matches_pure_noise() {
        let truth = (0.0, 2e-5);
        let m = estimate_synthetic((1200, 800), truth, |x, _| 0.02 + 0.8 * x as f64 / 1200.0);
        for (c, &wbc) in WB.iter().enumerate() {
            let b_exp = truth.1 * (wbc as f64).powi(2);
            let v = m.a[c] as f64 * 0.4 + m.b[c] as f64;
            let err = (v - b_exp).abs() / b_exp;
            assert!(err < 0.12, "colour {c}: variance {v:.3e}, expected {b_exp:.3e}");
        }
    }

    /// The failure that motivated the block estimator: fine texture over
    /// most of the frame must not be read as noise. The old median of second
    /// differences put this scene's shadow variance several times too high.
    #[test]
    fn texture_does_not_inflate_the_estimate() {
        let truth = (2.5e-4, 1e-6);
        let (w, h) = (1600usize, 1200usize);
        let m = estimate_synthetic((w, h), truth, |x, y| {
            let base = 0.02 + 0.7 * (y as f64 / h as f64).powi(2);
            // 80% of the frame carries ±40% texture with 5–9 px periods; a
            // strip down the left is left smooth, as skies and walls are.
            if x < w / 5 {
                base
            } else {
                let t = (x as f64 * 1.3).sin() * (y as f64 * 0.7).cos() * 0.25
                    + (x as f64 * 0.71 + y as f64 * 0.9).sin() * 0.15;
                base * (1.0 + t)
            }
        });
        let (ea, ev) = errors(&m, truth, 0.03);
        assert!(ea < 0.3, "a off by {:.0}%", ea * 100.0);
        assert!(ev < 0.35, "shadow variance off by {:.0}%", ev * 100.0);
    }

    /// The transform must actually stabilise: noise of unit variance at both
    /// ends of the range.
    #[test]
    fn vst_gives_unit_noise_across_the_range() {
        let model = NoiseModel { a: [4e-4; 3], b: [4e-6; 3] };
        let (a, c0, k) = model.vst(1);
        let s = |x: f64| k as f64 * (a as f64 * x + c0 as f64).max(0.0).sqrt();
        for x in [0.01f64, 0.1, 0.5, 0.9] {
            let sd = (4e-4 * x + 4e-6).sqrt();
            let slope = (s(x + 1e-6) - s(x - 1e-6)) / 2e-6;
            let sd_s = slope * sd;
            assert!((sd_s - 1.0).abs() < 0.1, "x={x}: stabilised σ {sd_s:.3}");
        }
    }

    /// A bin whose blocks are all dead flat reads variance 0; it must not
    /// dominate the fit (weight n/v² would be ~1e30 times the others).
    #[test]
    fn a_dead_flat_bin_does_not_collapse_the_fit() {
        let (a, b) = (0.01f64, 1e-4f64);
        let mut pts: Vec<(f64, f64, f64)> =
            (1..8).map(|i| { let m = i as f64 * 0.1; (m, a * m + b, 100.0) }).collect();
        pts.push((0.001, 0.0, 100.0));
        let (fa, _fb) = fit_lower_envelope(&pts).unwrap();
        assert!(fa > a * 0.3, "slope collapsed to {fa}");
    }

    #[test]
    fn tiny_inputs_fall_back_to_the_default() {
        let m = estimate(&[0u16; 64], 8, 8, [0, 1, 1, 2], [0.0; 4], [1.0; 4], [1.0; 3]);
        assert_eq!(m, NoiseModel::default());
    }
}
