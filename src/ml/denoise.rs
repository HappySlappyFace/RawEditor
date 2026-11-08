//! AI denoise: PMRID (Wang et al., ECCV 2020, "Practical Deep Raw Image
//! Denoising on Mobile Devices") on the Bayer mosaic, CPU.
//!
//! PMRID is a small U-Net (~1 M parameters) trained on one phone sensor at
//! one *anchor* gain. Its "k-sigma transform" makes it camera-independent: a
//! Poisson-Gaussian mosaic with `var = k·x + σ` (DN units) is mapped by the
//! per-pixel affine
//!
//! `y = x·(k_a/k) + (σ/k² − σ_a/k_a²)·k_a`
//!
//! onto exactly the anchor's statistics (`var(y) = k_a·y + σ_a`), denoised
//! there, and mapped back. `k` and `σ` are not read from ISO tables as in
//! the paper but taken from the image's own measured noise model
//! (`core::noise`, the model every other NR stage already trusts), per colour.
//!
//! The result is a whole new mosaic; the GPU pipeline never knows (see
//! `raw::loader::effective_mosaic`).

use super::models::{file_path, DENOISE};
use super::{runtime, Progress, CANCELLED};
use crate::core::noise::NoiseModel;

/// PMRID's DN scale (`run_benchmark.py`: `V = 959`, the Reno 10x's
/// white − black) and its anchor (ISO 1600 on its `K`/`Sigma` polynomials).
const V: f32 = 959.0;
const K_ANCHOR: f32 = 0.000_599_526_7 * 1600.0 + 0.008_688_61;
const SIGMA_ANCHOR: f32 = 7.117_72e-7 * 1600.0 * 1600.0 + 6.514_934e-4 * 1600.0 + 0.114_927_13;
/// The network's input scale (`inp_scale`).
const INPUT_SCALE: f32 = 256.0;
/// A frame much cleaner than the anchor would be stretched far outside what
/// the network was trained on; treat it as at least this fraction of it.
const MAX_GAIN_UP: f32 = 8.0;

/// Packed tile side (in 2×2 cells) and the overlap blended between tiles —
/// wider than the network's effective reach, so seams vanish.
const TILE: usize = 512;
const OVERLAP: usize = 48;
/// The U-Net downsamples four times by 2 and the reference code pads to 32.
const ALIGN: usize = 32;

/// Where the mosaic sits in sensor units.
#[derive(Debug, Clone, Copy)]
pub struct MosaicLayout {
    pub width: usize,
    pub height: usize,
    pub cfa_pattern: u32,
    /// Per 2×2 tile position `k = (y & 1)·2 + (x & 1)`, as `gpu::demosaic`.
    pub black: [f32; 4],
    pub white: f32,
}

impl MosaicLayout {
    fn colors(&self) -> [u32; 4] {
        crate::gpu::demosaic::tile_colors(self.cfa_pattern)
    }

    /// PMRID was trained on RGGB. Other layouts are mirrored into it, as its
    /// reference code does for BGGR (`bggr2rggb` flips both axes).
    fn flips(&self) -> (bool, bool) {
        let c = self.colors();
        // Flip x if R is on an odd column, y if on an odd row.
        let r = c.iter().position(|&v| v == 0).unwrap_or(0);
        (r & 1 == 1, r >> 1 == 1)
    }
}

/// The image's noise model in unit range (`(raw − black)/(white − black)`,
/// no white balance) — the units the k-sigma transform needs.
pub fn noise_model(mosaic: &[u16], layout: &MosaicLayout) -> NoiseModel {
    let scale = layout.black.map(|b| 1.0 / (layout.white - b).max(1.0));
    crate::core::noise::estimate(
        mosaic,
        layout.width,
        layout.height,
        layout.colors(),
        layout.black,
        scale,
        [1.0; 3],
    )
}

/// Per-colour affine `(gain, offset)` into anchor space, in unit range.
fn ksigma(model: &NoiseModel) -> [(f32, f32); 3] {
    std::array::from_fn(|c| {
        // var_unit = a·u + b  →  in DN (x = u·V): k = a·V, σ = b·V².
        let k = (model.a[c] * V).max(K_ANCHOR / MAX_GAIN_UP);
        let sigma = (model.b[c] * V * V).max(0.0);
        let gain = K_ANCHOR / k;
        let offset_dn = (sigma / (k * k) - SIGMA_ANCHOR / (K_ANCHOR * K_ANCHOR)) * K_ANCHOR;
        (gain, offset_dn / V)
    })
}

/// A packed, RGGB-ordered, normalised `4 × ph × pw` image (planar).
struct Packed {
    pw: usize,
    ph: usize,
    data: Vec<f32>,
}

/// Pack (mirroring into RGGB), normalise to unit range, clip to [0, 1] as
/// the reference does, and apply the k-sigma transform per channel.
fn pack(mosaic: &[u16], layout: &MosaicLayout, ks: &[(f32, f32); 3]) -> Packed {
    let (pw, ph) = (layout.width / 2, layout.height / 2);
    let (fx, fy) = layout.flips();
    let colors = layout.colors();
    let mut data = vec![0f32; 4 * pw * ph];
    for ch in 0..4 {
        let (dx, dy) = (ch & 1, ch >> 1);
        for ty in 0..ph {
            for tx in 0..pw {
                // Position in the mirrored (RGGB) frame → original mosaic.
                let (mx, my) = (2 * tx + dx, 2 * ty + dy);
                let x = if fx { 2 * pw - 1 - mx } else { mx };
                let y = if fy { 2 * ph - 1 - my } else { my };
                let k = (y & 1) * 2 + (x & 1);
                let c = colors[k] as usize;
                let range = (layout.white - layout.black[k]).max(1.0);
                let u = ((mosaic[y * layout.width + x] as f32 - layout.black[k]) / range).clamp(0.0, 1.0);
                data[ch * pw * ph + ty * pw + tx] = (u * ks[c].0 + ks[c].1) * INPUT_SCALE;
            }
        }
    }
    Packed { pw, ph, data }
}

/// The inverse of [`pack`], writing a full mosaic. Pixels that were clipped
/// keep their original value: highlight reconstruction finds clipped sites
/// by their sitting at the ceiling, and a denoised ceiling would hide them.
fn unpack(p: &Packed, mosaic: &[u16], layout: &MosaicLayout, ks: &[(f32, f32); 3]) -> Vec<u16> {
    let (fx, fy) = layout.flips();
    let colors = layout.colors();
    let mut out = mosaic.to_vec();
    for ch in 0..4 {
        let (dx, dy) = (ch & 1, ch >> 1);
        for ty in 0..p.ph {
            for tx in 0..p.pw {
                let (mx, my) = (2 * tx + dx, 2 * ty + dy);
                let x = if fx { 2 * p.pw - 1 - mx } else { mx };
                let y = if fy { 2 * p.ph - 1 - my } else { my };
                let k = (y & 1) * 2 + (x & 1);
                let c = colors[k] as usize;
                let range = (layout.white - layout.black[k]).max(1.0);
                let orig = mosaic[y * layout.width + x];
                if (orig as f32 - layout.black[k]) / range >= 0.98 {
                    continue;
                }
                let v = p.data[ch * p.pw * p.ph + ty * p.pw + tx] / INPUT_SCALE;
                let u = (v - ks[c].1) / ks[c].0;
                out[y * layout.width + x] = (u * range + layout.black[k]).round().clamp(0.0, 65535.0) as u16;
            }
        }
    }
    out
}

/// Tile origins along one axis: `TILE`-sized windows overlapping by
/// `OVERLAP`, the last one flush with the end.
fn tile_starts(len: usize) -> Vec<usize> {
    if len <= TILE {
        return vec![0];
    }
    let step = TILE - OVERLAP;
    let mut starts: Vec<usize> = (0..).map(|i| i * step).take_while(|&s| s + TILE < len).collect();
    starts.push(len - TILE);
    starts
}

/// Weight of position `i` inside a tile `[start, start+len)` of an axis of
/// `total`: a linear ramp across the overlap, 1 at the image's own edges.
fn ramp(i: usize, start: usize, len: usize, total: usize) -> f32 {
    let mut w = 1.0f32;
    if start > 0 && i < OVERLAP {
        w = w.min((i as f32 + 0.5) / OVERLAP as f32);
    }
    if start + len < total && i + OVERLAP >= len {
        w = w.min(((len - i) as f32 - 0.5) / OVERLAP as f32);
    }
    w.max(1e-3)
}

/// Run `net` over `p` tile by tile, blending overlaps. `net` takes a
/// `4 × h × w` planar tile (h, w multiples of `ALIGN`) and returns the same.
fn tiled(
    p: &Packed,
    progress: &Progress,
    mut net: impl FnMut(&[f32], usize, usize) -> Result<Vec<f32>, String>,
) -> Result<Packed, String> {
    let (pw, ph) = (p.pw, p.ph);
    let xs = tile_starts(pw);
    let ys = tile_starts(ph);
    progress.set_total((xs.len() * ys.len()) as u64);
    let mut acc = vec![0f32; 4 * pw * ph];
    let mut wsum = vec![0f32; pw * ph];
    for &y0 in &ys {
        let th = TILE.min(ph);
        for &x0 in &xs {
            if progress.cancelled() {
                return Err(CANCELLED.to_string());
            }
            let tw = TILE.min(pw);
            // Pad to the network's alignment by edge replication.
            let (aw, ah) = (tw.div_ceil(ALIGN) * ALIGN, th.div_ceil(ALIGN) * ALIGN);
            let mut tile = vec![0f32; 4 * aw * ah];
            for ch in 0..4 {
                for y in 0..ah {
                    let sy = y0 + y.min(th - 1);
                    for x in 0..aw {
                        let sx = x0 + x.min(tw - 1);
                        tile[ch * aw * ah + y * aw + x] = p.data[ch * pw * ph + sy * pw + sx];
                    }
                }
            }
            let out = net(&tile, ah, aw)?;
            for y in 0..th {
                let wy = ramp(y, y0, th, ph);
                for x in 0..tw {
                    let w = wy * ramp(x, x0, tw, pw);
                    let o = (y0 + y) * pw + x0 + x;
                    wsum[o] += w;
                    for ch in 0..4 {
                        acc[ch * pw * ph + o] += w * out[ch * aw * ah + y * aw + x];
                    }
                }
            }
            progress.add(1);
        }
    }
    for ch in 0..4 {
        for o in 0..pw * ph {
            acc[ch * pw * ph + o] /= wsum[o];
        }
    }
    Ok(Packed { pw, ph, data: acc })
}

/// Denoise a whole mosaic (or a CFA-aligned crop of one). Blocking: about
/// a second per few megapixels; checks `progress` for cancellation between
/// tiles. `model` is the crop's noise model in unit range — for a crop, pass
/// the whole image's ([`noise_model`]), which a small crop cannot measure.
pub fn denoise(
    mosaic: &[u16],
    layout: &MosaicLayout,
    model: &NoiseModel,
    progress: &Progress,
) -> Result<Vec<u16>, String> {
    if layout.width < 4 || layout.height < 4 || mosaic.len() < layout.width * layout.height {
        return Err("image too small to denoise".into());
    }
    // Even dims: a trailing odd row/column is left as it was.
    let even = MosaicLayout { width: layout.width & !1, height: layout.height & !1, ..*layout };
    let src = if even.width == layout.width {
        std::borrow::Cow::Borrowed(mosaic)
    } else {
        std::borrow::Cow::Owned(
            (0..even.height)
                .flat_map(|y| mosaic[y * layout.width..y * layout.width + even.width].iter().copied())
                .collect(),
        )
    };
    let t = std::time::Instant::now();
    let ks = ksigma(model);
    let packed = pack(&src, &even, &ks);
    let mut session = runtime::session(&file_path(&DENOISE.files[0]))?;
    let result = tiled(&packed, progress, |tile, h, w| {
        let input = runtime::tensor(&[1, 4, h, w], tile.to_vec())?;
        let outputs = session.run(ort::inputs!["input" => input]).map_err(|e| format!("AI denoise: {e}"))?;
        Ok(runtime::extract(&outputs, "output")?.1)
    })?;
    let out = unpack(&result, &src, &even, &ks);
    tracing::info!(
        "AI denoise: {}×{} in {:.0} ms (k-sigma gains {:?})",
        layout.width,
        layout.height,
        t.elapsed().as_secs_f64() * 1e3,
        ks.map(|k| k.0)
    );
    if even.width == layout.width && even.height == layout.height {
        return Ok(out);
    }
    let mut full = mosaic.to_vec();
    for y in 0..even.height {
        full[y * layout.width..y * layout.width + even.width].copy_from_slice(&out[y * even.width..(y + 1) * even.width]);
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(w: usize, h: usize, cfa: u32) -> MosaicLayout {
        MosaicLayout { width: w, height: h, cfa_pattern: cfa, black: [512.0; 4], white: 16383.0 }
    }

    fn ramp_mosaic(w: usize, h: usize) -> Vec<u16> {
        (0..w * h).map(|i| 600 + ((i * 7919) % 12000) as u16).collect()
    }

    /// Pack → unpack is exact for every CFA layout (a mirrored layout lands
    /// every sample back where it came from).
    #[test]
    fn packing_round_trips_for_every_layout() {
        let model = NoiseModel { a: [3e-4, 2e-4, 4e-4], b: [2e-6, 1e-6, 3e-6] };
        let ks = ksigma(&model);
        for cfa in 0..4 {
            let l = layout(24, 16, cfa);
            let m = ramp_mosaic(24, 16);
            let back = unpack(&pack(&m, &l, &ks), &m, &l, &ks);
            for (a, b) in m.iter().zip(&back) {
                assert!((*a as i32 - *b as i32).abs() <= 1, "cfa {cfa}: {a} vs {b}");
            }
        }
    }

    /// The packed channels are R, G, G, B whatever the sensor's layout: the
    /// order the network was trained on.
    #[test]
    fn every_layout_packs_as_rggb() {
        let ks = [(1.0, 0.0); 3];
        for cfa in 0..4 {
            let l = layout(8, 8, cfa);
            let colors = l.colors();
            // Each colour gets its own constant level.
            let level = [2000u16, 6000, 10000];
            let m: Vec<u16> = (0..64).map(|i| level[colors[((i / 8) & 1) * 2 + ((i % 8) & 1)] as usize]).collect();
            let p = pack(&m, &l, &ks);
            let plane = p.pw * p.ph;
            let at = |ch: usize| p.data[ch * plane] / INPUT_SCALE * (l.white - 512.0) + 512.0;
            assert!((at(0) - 2000.0).abs() < 1.0, "cfa {cfa}: R");
            assert!((at(1) - 6000.0).abs() < 1.0 && (at(2) - 6000.0).abs() < 1.0, "cfa {cfa}: G");
            assert!((at(3) - 10000.0).abs() < 1.0, "cfa {cfa}: B");
        }
    }

    /// The transform lands any noise model on the anchor's: the property the
    /// whole method rests on.
    #[test]
    fn ksigma_maps_any_noise_onto_the_anchor() {
        let model = NoiseModel { a: [2e-3, 1e-3, 3e-3], b: [1e-5, 5e-6, 2e-5] };
        let ks = ksigma(&model);
        for (c, &(g, o)) in ks.iter().enumerate() {
            for u in [0.01f32, 0.2, 0.7] {
                // var in unit range → DN after the transform.
                let var_dn = (model.a[c] * u + model.b[c]) * V * V * g * g;
                let y_dn = (u * g + o) * V;
                let want = K_ANCHOR * y_dn + SIGMA_ANCHOR;
                assert!((var_dn - want).abs() / want < 1e-3, "c{c} u{u}: {var_dn} vs {want}");
            }
        }
    }

    /// With an identity "network", tiling and blending reproduce the input
    /// exactly — seams can only come from the model, never the plumbing.
    #[test]
    fn tiling_with_an_identity_network_is_exact() {
        for (pw, ph) in [(1100, 700), (300, 200), (512, 513)] {
            let data: Vec<f32> = (0..4 * pw * ph).map(|i| ((i * 31) % 997) as f32).collect();
            let p = Packed { pw, ph, data: data.clone() };
            let progress = Progress::default();
            let out = tiled(&p, &progress, |t, h, w| {
                assert!(h % ALIGN == 0 && w % ALIGN == 0);
                Ok(t.to_vec())
            })
            .unwrap();
            for (a, b) in data.iter().zip(&out.data) {
                assert!((a - b).abs() < 1e-3);
            }
            assert!((progress.fraction() - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn tiles_cover_the_axis() {
        for len in [100, 512, 513, 1500, 4000] {
            let s = tile_starts(len);
            assert_eq!(s[0], 0);
            assert_eq!(*s.last().unwrap() + TILE.min(len), len);
            for pair in s.windows(2) {
                assert!(pair[1] - pair[0] <= TILE - OVERLAP);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Using a result: the on-disk cache and the blend.
// ---------------------------------------------------------------------------

/// The mosaic layout of a loaded RAW, with its own black levels (never the
/// user's black-offset edits: the cache must not depend on any edit).
pub fn layout_of(raw: &crate::raw::loader::RawDataResult) -> MosaicLayout {
    let dp = crate::gpu::demosaic::GpuDemosaicParams::new(
        raw.cfa_pattern,
        raw.black_levels,
        [0.0; 4],
        raw.white_level,
        raw.wb_multipliers,
    );
    MosaicLayout {
        width: raw.width as usize,
        height: raw.height as usize,
        cfa_pattern: raw.cfa_pattern,
        black: dp.black,
        white: raw.white_level as f32,
    }
}

const CACHE_MAGIC: &[u8; 4] = b"RDN1";

/// `cache/denoised/<image id>-<model hash>.bin`: a new model never reads
/// another model's results.
fn cache_path(image_id: i64) -> Option<std::path::PathBuf> {
    let tag = &DENOISE.files[0].sha256[..12];
    crate::core::paths::cache_dir().map(|d| d.join("denoised").join(format!("{image_id}-{tag}.bin")))
}

/// Store a whole-image result. Blocking (~50 MB at 24 MP).
pub fn save_cache(image_id: i64, width: u32, height: u32, data: &[u16]) -> Result<(), String> {
    let path = cache_path(image_id).ok_or("no cache folder")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let mut bytes = Vec::with_capacity(16 + data.len() * 2);
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    for v in data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("could not save the AI denoise result: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("could not save the AI denoise result: {e}"))
}

/// A stored result for this image at these dimensions. Blocking.
pub fn load_cache(image_id: i64, width: u32, height: u32) -> Option<Vec<u16>> {
    let bytes = std::fs::read(cache_path(image_id)?).ok()?;
    let n = (width as usize) * (height as usize);
    if bytes.len() != 16 + 2 * n
        || &bytes[..4] != CACHE_MAGIC
        || bytes[4..8] != width.to_le_bytes()
        || bytes[8..12] != height.to_le_bytes()
    {
        return None;
    }
    Some(bytes[16..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect())
}

pub fn has_cache(image_id: i64) -> bool {
    cache_path(image_id).is_some_and(|p| p.exists())
}

/// `original` moved `amount` (0..=1) of the way to `denoised`.
pub fn blend(original: &[u16], denoised: &[u16], amount: f32) -> Vec<u16> {
    let a = amount.clamp(0.0, 1.0);
    original
        .iter()
        .zip(denoised)
        .map(|(&o, &d)| (o as f32 + (d as f32 - o as f32) * a).round() as u16)
        .collect()
}

/// The mosaic to upload for `amount` of AI denoise, and the amount actually
/// applied (0 when no result is cached — the caller regenerates it).
/// Blocking.
pub fn effective_mosaic<'a>(
    raw: &'a crate::raw::loader::RawDataResult,
    image_id: i64,
    amount: f32,
) -> (std::borrow::Cow<'a, [u16]>, f32) {
    if amount <= 0.0 {
        return (std::borrow::Cow::Borrowed(&raw.data), 0.0);
    }
    match load_cache(image_id, raw.width, raw.height) {
        Some(denoised) => (std::borrow::Cow::Owned(blend(&raw.data, &denoised, amount)), amount),
        None => {
            tracing::warn!("AI denoise result for image {image_id} is not cached; showing the original");
            (std::borrow::Cow::Borrowed(&raw.data), 0.0)
        }
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn blending_spans_original_to_denoised() {
        let o = [100u16, 200, 300];
        let d = [200u16, 100, 300];
        assert_eq!(blend(&o, &d, 0.0), o);
        assert_eq!(blend(&o, &d, 1.0), d);
        assert_eq!(blend(&o, &d, 0.5), [150, 150, 300]);
    }
}
