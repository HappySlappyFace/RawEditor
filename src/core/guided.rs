//! Guided filter (He, Sun & Tang, 2010) on the CPU.
//!
//! Filters `p` so that its edges follow those of the guide `I`: locally
//! `q = a·I + b`, with `a, b` the least-squares fit of `p` against `I` over a
//! `(2r+1)²` window, then averaged. AI masks use it to snap the segmentation
//! model's coarse (256² logit) boundary onto the photo's real edges — hair and
//! branches come out soft where the image is, not where the logit grid was.
//! O(pixels) regardless of `r` (summed-area tables).

/// Mean of `src` over the `(2r+1)²` window clamped to the image, per pixel.
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    // Summed-area table in f64: 4 MP of f32 sums lose the low bits.
    let mut sat = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += src[y * w + x] as f64;
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0]
                + sat[y0 * (w + 1) + x0];
            out[y * w + x] = (s / ((x1 - x0) * (y1 - y0)) as f64) as f32;
        }
    }
    out
}

/// Guided filter of `p` by guide `i` (both `w×h`, row-major). `eps` is the
/// regulariser in the guide's units squared: edges with a guide contrast well
/// above `sqrt(eps)` are followed, flatter regions are smoothed.
pub fn filter(i: &[f32], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    assert_eq!(i.len(), w * h);
    assert_eq!(p.len(), w * h);
    let mean_i = box_mean(i, w, h, r);
    let mean_p = box_mean(p, w, h, r);
    let ip: Vec<f32> = i.iter().zip(p).map(|(a, b)| a * b).collect();
    let ii: Vec<f32> = i.iter().map(|a| a * a).collect();
    let corr_ip = box_mean(&ip, w, h, r);
    let corr_ii = box_mean(&ii, w, h, r);
    let mut a = vec![0f32; w * h];
    let mut b = vec![0f32; w * h];
    for k in 0..w * h {
        let var_i = (corr_ii[k] - mean_i[k] * mean_i[k]).max(0.0);
        let cov_ip = corr_ip[k] - mean_i[k] * mean_p[k];
        a[k] = cov_ip / (var_i + eps);
        b[k] = mean_p[k] - a[k] * mean_i[k];
    }
    let mean_a = box_mean(&a, w, h, r);
    let mean_b = box_mean(&b, w, h, r);
    (0..w * h).map(|k| mean_a[k] * i[k] + mean_b[k]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_mean_matches_brute_force() {
        let (w, h, r) = (13, 9, 2);
        let src: Vec<f32> = (0..w * h).map(|k| ((k * 37) % 11) as f32).collect();
        let fast = box_mean(&src, w, h, r);
        for y in 0..h {
            for x in 0..w {
                let (mut s, mut n) = (0.0, 0.0);
                for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                        s += src[yy * w + xx];
                        n += 1.0;
                    }
                }
                assert!((fast[y * w + x] - s / n).abs() < 1e-4);
            }
        }
    }

    /// A blocky mask whose step is 3 px off the guide's edge comes back
    /// following the guide: the essence of the AI-mask refinement.
    #[test]
    fn a_misplaced_mask_edge_snaps_to_the_guide() {
        let (w, h) = (64, 16);
        let edge = 32;
        let guide: Vec<f32> = (0..w * h).map(|k| if k % w < edge { 0.1 } else { 0.9 }).collect();
        let mask: Vec<f32> = (0..w * h).map(|k| if k % w < edge + 3 { 0.0 } else { 1.0 }).collect();
        let q = filter(&guide, &mask, w, h, 6, 1e-3);
        let row = 8 * w;
        // The 50% crossing moves from the mask's edge to the guide's, and
        // the guide's dark side stays clean.
        let crossing = (0..w).find(|&x| q[row + x] >= 0.5).unwrap();
        assert!(crossing <= edge + 1, "crossing at {crossing}, guide edge {edge}");
        assert!(q[row + edge - 1] < 0.1, "{}", q[row + edge - 1]);
        assert!(q[row + 10] < 0.15 && q[row + 54] > 0.85);
    }

    #[test]
    fn a_constant_mask_is_left_alone() {
        let (w, h) = (20, 20);
        let guide: Vec<f32> = (0..w * h).map(|k| (k % 7) as f32 / 7.0).collect();
        let q = filter(&guide, &vec![0.4; w * h], w, h, 3, 1e-2);
        assert!(q.iter().all(|v| (v - 0.4).abs() < 1e-4));
    }
}
