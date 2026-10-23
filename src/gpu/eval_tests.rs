//! Objective quality harness for the stages that cannot be judged by reading
//! code: a synthetic Bayer mosaic is built from a known scene, given
//! Poisson-Gaussian noise of a chosen strength (or fogged with a known haze),
//! and rendered through the real GPU pipeline. The clean mosaic rendered
//! without the stage is the reference, so denoising and dehazing are scored
//! against ground truth; Texture and Clarity are checked for which band of
//! scales they move, and for the preview and the export agreeing.
//!
//! The scene is procedural unless `RAWEDITOR_EVAL_IMAGE` names a photo.
//! Needs a GPU (lavapipe works). Everything here is `#[ignore]`d.

use super::render_functions::render_to_bytes;
use super::shared::{ImageResources, SharedContext};
use crate::core::types::EditParams;
use crate::gpu::params::FULL_VIEW_RECT;

pub struct Scene {
    pub w: usize,
    pub h: usize,
    /// Linear RGB, white-balanced (what the colour pass sees after WB).
    pub lin: Vec<[f32; 3]>,
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

impl Scene {
    /// A photo from disk, resized to `w` wide (even dims).
    pub fn from_file(path: &str, w: usize) -> Option<Self> {
        let img = image::open(path).ok()?.to_rgb8();
        let h = (img.height() as usize * w / img.width() as usize) & !1;
        let img = image::imageops::resize(&img, w as u32, h as u32, image::imageops::FilterType::Lanczos3);
        let lin = img
            .pixels()
            .map(|p| p.0.map(|c| srgb_to_linear(c as f32 / 255.0) * 0.8))
            .collect();
        Some(Self { w, h, lin })
    }

    /// Smooth gradients, hard edges and band-limited texture at several
    /// scales — no file needed.
    pub fn procedural(w: usize, h: usize) -> Self {
        let mut lin = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                // Sky-like gradient on top, textured ground below.
                let mut c = if v < 0.45 {
                    [0.25 + 0.2 * v, 0.32 + 0.2 * v, 0.55 - 0.1 * v]
                } else {
                    let t = (x as f32 * 0.21).sin() * (y as f32 * 0.17).sin() * 0.5
                        + (x as f32 * 0.043 + y as f32 * 0.031).sin() * 0.3
                        + (x as f32 * 0.9).sin() * (y as f32 * 0.7).cos() * 0.15;
                    let g = 0.12 * (1.0 + 0.6 * t);
                    if u > 0.5 {
                        // Foliage-like: random high-contrast detail 2–3 px
                        // across — the content that fooled the old noise
                        // estimator into reading texture as noise.
                        let k = ((x / 2) as u64 * 0x9E37_79B9) ^ ((y / 3) as u64 * 0x85EB_CA6B);
                        let k = (k ^ (k >> 15)).wrapping_mul(0x2C1B_3C6D);
                        let r = ((k >> 20) & 0xFF) as f32 / 255.0;
                        let g = g * (0.4 + 1.2 * r);
                        [g * 0.8, g * 1.1, g * 0.5]
                    } else {
                        [g * 0.9, g * 1.05, g * 0.6]
                    }
                };
                // A few discs: edges at several contrasts.
                for (cx, cy, r, col) in [
                    (0.25f32, 0.7f32, 0.12f32, [0.6f32, 0.1, 0.08]),
                    (0.6, 0.65, 0.08, [0.05, 0.05, 0.05]),
                    (0.8, 0.3, 0.1, [0.7, 0.7, 0.68]),
                ] {
                    let d = ((u - cx) * w as f32 / h as f32).hypot(v - cy);
                    if d < r {
                        c = col;
                    }
                }
                lin.push(c);
            }
        }
        Self { w, h, lin }
    }

    /// The default scene: `RAWEDITOR_EVAL_IMAGE` if set, else procedural.
    pub fn default_scene(w: usize) -> Self {
        std::env::var("RAWEDITOR_EVAL_IMAGE")
            .ok()
            .and_then(|p| Self::from_file(&p, w))
            .unwrap_or_else(|| Self::procedural(w, (w * 3 / 4) & !1))
    }
}

pub const BLACK: f32 = 512.0;
pub const WHITE: f32 = 16383.0;
pub const WB: [f32; 4] = [2.0, 1.0, 1.5, 1.0];

/// xorshift64* — deterministic noise without a dependency.
struct Rng(u64);
impl Rng {
    fn f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f32 {
        let u1 = self.f64().max(1e-12);
        let u2 = self.f64();
        ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
    }
}

/// RGGB mosaic of `scene` in camera-native units (scene ÷ WB), with
/// variance `a·x + b` (x in unit range, before WB) when `a > 0`.
pub fn mosaic(scene: &Scene, a: f32, b: f32, seed: u64) -> Vec<u16> {
    let mut rng = Rng(seed | 1);
    let mut out = vec![0u16; scene.w * scene.h];
    for y in 0..scene.h {
        for x in 0..scene.w {
            let c = match (y & 1, x & 1) {
                (0, 0) => 0,
                (1, 1) => 2,
                _ => 1,
            };
            let v = (scene.lin[y * scene.w + x][c] / WB[c]).clamp(0.0, 1.0);
            let n = if a > 0.0 { (a * v + b).sqrt() * rng.gauss() } else { 0.0 };
            out[y * scene.w + x] = (BLACK + (v + n) * (WHITE - BLACK)).round().clamp(0.0, 65535.0) as u16;
        }
    }
    out
}

/// Render a mosaic the way develop does (no DCP: identity camera→sRGB,
/// which is exact here because the mosaic is the scene ÷ WB).
pub async fn render(
    ctx: &SharedContext,
    data: &[u16],
    w: usize,
    h: usize,
    params: &EditParams,
    preview: Option<u32>,
) -> (Vec<u8>, u32, u32) {
    let res = ImageResources::new(
        ctx,
        1,
        data,
        w as u32,
        h as u32,
        params,
        WB,
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        0,
        [BLACK as u32; 4],
        WHITE as u32,
        None,
        preview,
        1,
        std::sync::Arc::new(crate::core::lens::LensContext::default()),
    )
    .expect("image resources");
    res.update_uniforms(ctx, params, None, None);
    let (rw, rh) = res.oriented_dims(params.quarter_turns);
    let (bytes, _, _) = render_to_bytes(ctx, &res, rw, rh, FULL_VIEW_RECT).await;
    ctx.reclaim();
    (bytes.to_vec(), rw, rh)
}

/// (luma PSNR, chroma PSNR) of `img` against `reference`, in dB, ignoring a
/// border of `margin` pixels (edge handling is not what is being judged).
pub fn psnr_luma_chroma(img: &[u8], reference: &[u8], w: u32, h: u32, margin: u32) -> (f64, f64) {
    let (mut el, mut ec, mut n) = (0.0f64, 0.0f64, 0usize);
    for y in margin..h - margin {
        for x in margin..w - margin {
            let i = ((y * w + x) * 4) as usize;
            let p = |b: &[u8]| [b[i] as f64, b[i + 1] as f64, b[i + 2] as f64];
            let (a, r) = (p(img), p(reference));
            let la = 0.299 * a[0] + 0.587 * a[1] + 0.114 * a[2];
            let lr = 0.299 * r[0] + 0.587 * r[1] + 0.114 * r[2];
            el += (la - lr).powi(2);
            for c in 0..3 {
                ec += ((a[c] - la) - (r[c] - lr)).powi(2) / 3.0;
            }
            n += 1;
        }
    }
    let db = |e: f64| 10.0 * (255.0f64 * 255.0 / (e / n as f64).max(1e-9)).log10();
    (db(el), db(ec))
}

pub fn write_png(path: &str, img: &[u8], w: u32, h: u32) {
    if let Some(i) = image::RgbaImage::from_raw(w, h, img.to_vec()) {
        let _ = i.save(path);
    }
}

pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().expect("runtime")
}

/// Sweep the NR sliders at three noise levels and two working strides and
/// print PSNR against the clean render. A development tool, not an
/// assertion: run with
/// `RAWEDITOR_EVAL_IMAGE=photo.jpg cargo test --release nr_sweep -- --ignored --nocapture`.
#[test]
#[ignore]
fn nr_sweep() {
    let w: usize = std::env::var("EVAL_W").ok().and_then(|v| v.parse().ok()).unwrap_or(2400);
    let scene = Scene::default_scene(w);
    let dump = std::env::var("EVAL_DUMP").ok();
    let levels: Vec<(&str, f32, f32)> = vec![("iso800", 2.5e-4, 1e-6), ("iso3200", 1e-3, 4e-6), ("iso12800", 4e-3, 2e-5)]
        .into_iter()
        .filter(|(n, _, _)| std::env::var("EVAL_ISO").map_or(true, |f| f.split(',').any(|x| x == *n)))
        .collect();
    let strides: Vec<Option<u32>> = match std::env::var("EVAL_STRIDE").as_deref() {
        Ok("1") => vec![None],
        Ok("3") => vec![Some(w as u32 / 3)],
        _ => vec![Some(w as u32 / 3), None],
    };
    let rt = runtime();
    rt.block_on(async {
        let ctx = SharedContext::new().await.expect("gpu");
        let clean = mosaic(&scene, 0.0, 0.0, 1);
        for preview in &strides {
            let off = EditParams { color_noise: 0.0, ..EditParams::default() };
            let (reference, rw, rh) = render(&ctx, &clean, scene.w, scene.h, &off, *preview).await;
            for (name, a, b) in &levels {
                let noisy = mosaic(&scene, *a, *b, 7);
                let mut best = (f64::MIN, 0.0, 0.0);
                let settings: Vec<(f32, f32)> = std::env::var("EVAL_SETTINGS")
                    .ok()
                    .map(|v| {
                        v.split(';')
                            .filter_map(|p| {
                                let (l, c) = p.split_once(',')?;
                                Some((l.trim().parse().ok()?, c.trim().parse().ok()?))
                            })
                            .collect()
                    })
                    .unwrap_or_else(|| vec![(0.0, 0.0), (0.0, 0.5), (0.25, 0.5), (0.5, 0.5), (0.75, 0.5), (1.0, 0.5), (0.5, 1.0)]);
                for &(luma, color) in &settings {
                    let p = EditParams { luma_noise: luma, color_noise: color, ..EditParams::default() };
                    let t = std::time::Instant::now();
                    let (img, _, _) = render(&ctx, &noisy, scene.w, scene.h, &p, *preview).await;
                    let (pl, pc) = psnr_luma_chroma(&img, &reference, rw, rh, 8);
                    eprintln!(
                        "{:>8} {name:>9} luma {luma:.2} color {color:.2}: Y {pl:6.2} dB  C {pc:6.2} dB  ({:.1}s)",
                        if preview.is_some() { "preview" } else { "full" },
                        t.elapsed().as_secs_f32()
                    );
                    if pl > best.0 {
                        best = (pl, luma, color);
                    }
                    if let Some(d) = &dump {
                        write_png(
                            &format!("{d}/{}_{name}_l{luma}_c{color}.png", if preview.is_some() { "p" } else { "f" }),
                            &img, rw, rh,
                        );
                    }
                }
                eprintln!("  best luma PSNR {:.2} at luma {:.2}", best.0, best.1);
            }
            if let Some(d) = &dump {
                write_png(&format!("{d}/{}_reference.png", if preview.is_some() { "p" } else { "f" }), &reference, rw, rh);
            }
        }
    });
}

/// Noise reduction must move a noisy render TOWARDS the clean one: better
/// PSNR at high ISO, and no damage at low ISO. The second half is what the
/// old noise estimator failed — it read scene texture as noise, so on a
/// detailed ISO 800 frame Luminance 0.5 cost ~20 dB of luma PSNR.
///
/// Run with `cargo test --release denoise_moves_towards -- --ignored --nocapture`.
#[test]
#[ignore]
fn denoise_moves_towards_ground_truth() {
    let scene = Scene::procedural(1600, 1200);
    runtime().block_on(async {
        let ctx = SharedContext::new().await.expect("gpu");
        let off = EditParams { color_noise: 0.0, ..EditParams::default() };
        let (reference, w, h) = render(&ctx, &mosaic(&scene, 0.0, 0.0, 1), scene.w, scene.h, &off, None).await;
        for (name, a, b, luma, min_gain) in [("iso800", 2.5e-4f32, 1e-6f32, 0.5f32, -0.2f64), ("iso6400", 2e-3, 1e-5, 0.7, 2.0)] {
            let noisy = mosaic(&scene, a, b, 7);
            let (raw, _, _) = render(&ctx, &noisy, scene.w, scene.h, &off, None).await;
            let on = EditParams { luma_noise: luma, color_noise: 0.5, ..EditParams::default() };
            let (dn, _, _) = render(&ctx, &noisy, scene.w, scene.h, &on, None).await;
            let (y0, c0) = psnr_luma_chroma(&raw, &reference, w, h, 8);
            let (y1, c1) = psnr_luma_chroma(&dn, &reference, w, h, 8);
            eprintln!("{name}: luma {y0:.2} → {y1:.2} dB, chroma {c0:.2} → {c1:.2} dB");
            assert!(y1 - y0 > min_gain, "{name}: luma NR moved away from ground truth");
            assert!(c1 - c0 > min_gain, "{name}: colour NR moved away from ground truth");
        }
    });
}

/// Airlight of the synthetic haze (white-balanced linear RGB).
pub const HAZE_A: [f32; 3] = [0.62, 0.66, 0.72];

/// The scene seen through haze that thickens towards the top of the frame
/// (`I = J·t + A·(1 − t)`), as distance usually does.
pub fn hazy(scene: &Scene, density: f32) -> Scene {
    let mut lin = scene.lin.clone();
    for y in 0..scene.h {
        let t = (-density * (1.0 - y as f32 / scene.h as f32) * 2.0).exp();
        for x in 0..scene.w {
            let p = &mut lin[y * scene.w + x];
            for c in 0..3 {
                p[c] = p[c] * t + HAZE_A[c] * (1.0 - t);
            }
        }
    }
    Scene { w: scene.w, h: scene.h, lin }
}

/// `EditParams::default()` patched with a JSON object, e.g. `{"clarity":1}`.
pub fn patched(json: &str) -> EditParams {
    let mut v = serde_json::to_value(EditParams::default()).expect("params json");
    if let Ok(serde_json::Value::Object(patch)) = serde_json::from_str::<serde_json::Value>(json) {
        for (k, val) in patch {
            v[k] = val;
        }
    }
    serde_json::from_value(v).expect("patched params")
}

/// Development tool: render the eval scene with `EVAL_PARAMS` (JSON patches
/// separated by `;`) and write PNGs to `EVAL_DUMP`. `EVAL_HAZE=<density>`
/// fogs the scene first.
#[test]
#[ignore]
fn presence_dump() {
    let Ok(dir) = std::env::var("EVAL_DUMP") else { return };
    let w: usize = std::env::var("EVAL_W").ok().and_then(|v| v.parse().ok()).unwrap_or(2400);
    let mut scene = Scene::default_scene(w);
    if let Some(d) = std::env::var("EVAL_HAZE").ok().and_then(|v| v.parse().ok()) {
        scene = hazy(&scene, d);
    }
    let preview = std::env::var("EVAL_STRIDE").ok().filter(|v| v == "3").map(|_| w as u32 / 3);
    let patches = std::env::var("EVAL_PARAMS").unwrap_or_else(|_| "{}".into());
    runtime().block_on(async {
        let ctx = SharedContext::new().await.expect("gpu");
        let data = mosaic(&scene, 2.5e-4, 1e-6, 3);
        for (i, json) in patches.split(';').enumerate() {
            let p = patched(json);
            let t = std::time::Instant::now();
            let (img, rw, rh) = render(&ctx, &data, scene.w, scene.h, &p, preview).await;
            eprintln!("{i}: {json} → {rw}x{rh} in {:.1}s", t.elapsed().as_secs_f32());
            write_png(&format!("{dir}/{i}.png"), &img, rw, rh);
        }
    });
}

/// Dehaze must move a fogged render TOWARDS the haze-free one, and adding
/// haze must move a clear one away from it.
///
/// Run with `cargo test --release dehaze_moves_towards -- --ignored --nocapture`.
#[test]
#[ignore]
fn dehaze_moves_towards_the_clear_scene() {
    let w: usize = std::env::var("EVAL_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1600);
    let scene = Scene::default_scene(w);
    let fog = hazy(&scene, 1.0);
    let dump = std::env::var("EVAL_DUMP").ok();
    runtime().block_on(async {
        let ctx = SharedContext::new().await.expect("gpu");
        let base = EditParams::default();
        let (clear, rw, rh) = render(&ctx, &mosaic(&scene, 0.0, 0.0, 1), scene.w, scene.h, &base, None).await;
        let foggy = mosaic(&fog, 0.0, 0.0, 1);
        let psnr = |img: &[u8]| {
            let (y, c) = psnr_luma_chroma(img, &clear, rw, rh, 8);
            (y, c)
        };
        let mut last = f64::MIN;
        for amount in [0.0f32, 0.5, 1.0] {
            let p = EditParams { dehaze: amount, ..base };
            let (img, _, _) = render(&ctx, &foggy, scene.w, scene.h, &p, None).await;
            let (y, c) = psnr(&img);
            eprintln!("fogged, dehaze {amount:+.1}: luma {y:.2} dB, chroma {c:.2} dB vs clear");
            if let Some(d) = &dump {
                write_png(&format!("{d}/dehaze_{amount}.png"), &img, rw, rh);
            }
            assert!(y > last, "more dehaze did not get closer to the clear scene");
            last = y;
        }
        let (added, _, _) = render(&ctx, &mosaic(&scene, 0.0, 0.0, 1), scene.w, scene.h, &EditParams { dehaze: -1.0, ..base }, None).await;
        let (y, _) = psnr(&added);
        eprintln!("clear, dehaze -1: luma {y:.2} dB");
        assert!(y < 30.0, "negative dehaze did not add haze");
        if let Some(d) = &dump {
            write_png(&format!("{d}/clear.png"), &clear, rw, rh);
            write_png(&format!("{d}/added.png"), &added, rw, rh);
        }
    });
}

/// Box-downscale RGBA `src` (sw×sh) to dw×dh.
fn downscale(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_raw(sw, sh, src.to_vec()).expect("rgba");
    image::imageops::resize(&img, dw, dh, image::imageops::FilterType::Triangle).into_raw()
}

/// What the develop preview shows must be what the export delivers: each
/// presence effect, rendered on the subsampled preview and at full
/// resolution (then scaled down to match), must change the image by the
/// same amount. Scales tied to pixels instead of to the image would make
/// Clarity crunchier on screen than in the file, or the reverse.
///
/// Run with `cargo test --release presence_matches_between -- --ignored --nocapture`.
#[test]
#[ignore]
fn presence_matches_between_preview_and_export() {
    // 3840 wide → a 1280 px preview at stride 3, the develop path's size.
    let w = 3840usize;
    let scene = hazy(&Scene::default_scene(w), 0.3);
    runtime().block_on(async {
        let ctx = SharedContext::new().await.expect("gpu");
        let data = mosaic(&scene, 0.0, 0.0, 1);
        let base = EditParams { color_noise: 0.0, ..EditParams::default() };
        let (p0, pw, ph) = render(&ctx, &data, scene.w, scene.h, &base, Some(w as u32 / 3)).await;
        let (f0, fw, fh) = render(&ctx, &data, scene.w, scene.h, &base, None).await;
        let f0 = downscale(&f0, fw, fh, pw, ph);
        // Allowed strength difference. Texture's band reaches below a preview
        // pixel, so the preview can only approximate it; the rest match.
        for (name, p, tolerance) in [
            ("clarity +1", EditParams { clarity: 1.0, ..base }, 0.15),
            ("clarity -1", EditParams { clarity: -1.0, ..base }, 0.15),
            ("texture +1", EditParams { texture: 1.0, ..base }, 0.5),
            ("dehaze +1", EditParams { dehaze: 1.0, ..base }, 0.15),
        ] {
            let (p1, _, _) = render(&ctx, &data, scene.w, scene.h, &p, Some(w as u32 / 3)).await;
            let (f1, _, _) = render(&ctx, &data, scene.w, scene.h, &p, None).await;
            let f1 = downscale(&f1, fw, fh, pw, ph);
            // Mean absolute change each render made, and how far apart the
            // two changes are.
            let (mut dp, mut df, mut mismatch) = (0.0f64, 0.0f64, 0.0f64);
            for i in (0..p0.len()).step_by(4) {
                for c in 0..3 {
                    let a = p1[i + c] as f64 - p0[i + c] as f64;
                    let b = f1[i + c] as f64 - f0[i + c] as f64;
                    dp += a.abs();
                    df += b.abs();
                    mismatch += (a - b).abs();
                }
            }
            let n = (p0.len() / 4 * 3) as f64;
            let (dp, df, mismatch) = (dp / n, df / n, mismatch / n);
            eprintln!("{name}: preview changes {dp:.2}, export {df:.2}, mismatch {mismatch:.2} (/255)");
            assert!((dp / df - 1.0).abs() < tolerance, "{name}: preview and export differ in strength");
            assert!(mismatch < 0.6 * dp.max(df), "{name}: preview and export change different things");
        }
    });
}

/// Luma of an RGBA render, as f32.
fn luma_plane(img: &[u8]) -> Vec<f32> {
    img.as_chunks::<4>().0.iter().map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).collect()
}

/// Box blur of radius `r` (clamped window), via an integral image.
fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut sat = vec![0.0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += src[y * w + x] as f64;
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0] + sat[y0 * (w + 1) + x0];
            out[y * w + x] = (s / ((y1 - y0) * (x1 - x0)) as f64) as f32;
        }
    }
    out
}

/// RMS of the difference of two blurs: the energy in that band of scales.
fn band_energy(l: &[f32], w: usize, h: usize, fine: usize, coarse: usize) -> f64 {
    let (a, b) = (box_blur(l, w, h, fine), box_blur(l, w, h, coarse));
    (a.iter().zip(&b).map(|(x, y)| ((x - y) as f64).powi(2)).sum::<f64>() / a.len() as f64).sqrt()
}

/// Clarity must work on mid-scale structure and Texture on fine detail —
/// each mostly on its own band — and negative values must do the opposite.
///
/// Run with `cargo test --release presence_moves_its_own_band -- --ignored --nocapture`.
#[test]
#[ignore]
fn presence_moves_its_own_band() {
    let scene = Scene::default_scene(2400);
    runtime().block_on(async {
        let ctx = SharedContext::new().await.expect("gpu");
        let data = mosaic(&scene, 0.0, 0.0, 1);
        let base = EditParams { color_noise: 0.0, ..EditParams::default() };
        let bands = |img: &[u8], w: u32, h: u32| {
            let l = luma_plane(img);
            let (w, h) = (w as usize, h as usize);
            // Fine: 1–3 px; mid: 6–40 px (the long edge is 2400 px, so
            // Texture's scale is ~4 px and Clarity's ~40 px).
            (band_energy(&l, w, h, 0, 3), band_energy(&l, w, h, 6, 40))
        };
        let (img, w, h) = render(&ctx, &data, scene.w, scene.h, &base, None).await;
        let (fine0, mid0) = bands(&img, w, h);
        let gain = |p: EditParams| {
            let ctx = &ctx;
            let data = &data;
            let scene = &scene;
            async move {
                let (img, w, h) = render(ctx, data, scene.w, scene.h, &p, None).await;
                let (f, m) = bands(&img, w, h);
                (f / fine0, m / mid0)
            }
        };
        let (cf, cm) = gain(EditParams { clarity: 1.0, ..base }).await;
        let (tf, tm) = gain(EditParams { texture: 1.0, ..base }).await;
        let (_, cm_neg) = gain(EditParams { clarity: -1.0, ..base }).await;
        let (tf_neg, _) = gain(EditParams { texture: -1.0, ..base }).await;
        eprintln!("clarity +1: fine ×{cf:.2}, mid ×{cm:.2};  clarity -1: mid ×{cm_neg:.2}");
        eprintln!("texture +1: fine ×{tf:.2}, mid ×{tm:.2};  texture -1: fine ×{tf_neg:.2}");
        assert!(cm > 1.1 && cm > cf, "clarity did not mainly raise mid-scale contrast");
        assert!(tf > 1.1 && tf > tm, "texture did not mainly raise fine detail");
        assert!(cm_neg < 0.95, "negative clarity did not soften mid-scale structure");
        assert!(tf_neg < 0.95, "negative texture did not smooth fine detail");
    });
}

/// Lens Constrain through the real shader: a correction that pulls the
/// frame's edges inwards leaves black borders without it, and none with it.
/// (The CPU-side tests in `core::lens` mirror the shader's mapping; this is
/// the check that the mirror and the shader agree.)
#[test]
#[ignore]
fn lens_constrain_leaves_no_black_border() {
    use crate::core::lens::LensParams;
    let scene = Scene::default_scene(800);
    let data = mosaic(&scene, 0.0, 0.0, 1);
    // Pixels on the outermost ring that the shader blanked (exact black: the
    // scene itself has no pure-black pixels).
    let black_border = |img: &[u8], w: u32, h: u32| {
        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            img[i] as u32 + img[i + 1] as u32 + img[i + 2] as u32 == 0
        };
        let horizontal = (0..w).filter(|&x| px(x, 0) || px(x, h - 1)).count();
        let vertical = (0..h).filter(|&y| px(0, y) || px(w - 1, y)).count();
        horizontal + vertical
    };
    runtime().block_on(async {
        let ctx = SharedContext::new().await.expect("gpu");
        for distortion in [-1.0f32, -0.4] {
            let params = |constrain| EditParams {
                lens: LensParams { distortion, constrain, ..LensParams::default() },
                ..EditParams::default()
            };
            let (off, w, h) = render(&ctx, &data, scene.w, scene.h, &params(false), None).await;
            let (on, _, _) = render(&ctx, &data, scene.w, scene.h, &params(true), None).await;
            let (b_off, b_on) = (black_border(&off, w, h), black_border(&on, w, h));
            eprintln!("distortion {distortion}: black border px without constrain {b_off}, with {b_on}");
            assert!(b_off > 0, "the correction should leave borders to remove");
            assert_eq!(b_on, 0, "Constrain left {b_on} black border pixels");
        }
    });
}

/// Highlight reconstruction against ground truth: a light-blue sky that
/// brightens past green's and then blue's clip point — red never clips, the
/// case that rendered purple — with the clipped photosites reading ABOVE
/// white, as a Nikon D3300's do. Pulled down 1.5 EV, it must match the same
/// sky captured with headroom to spare. The old rule (rebuild from the mean
/// of the unclipped channels) gave green = red under a higher blue there:
/// lavender.
#[test]
#[ignore]
fn clipped_sky_matches_the_unclipped_truth() {
    // Camera-native sky; white-balanced by WB it is (0.84, 1, 1.2).
    const SKY: [f32; 3] = [0.42, 1.0, 0.8];
    const HEADROOM: f32 = 4.0;
    let (w, h) = (512usize, 384usize);
    let gain = |y: usize| 2.0 - 1.5 * y as f32 / h as f32;
    let capture = |scale: f32| -> Vec<u16> {
        let range = WHITE - BLACK;
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let c = match (y & 1, x & 1) {
                    (0, 0) => 0,
                    (1, 1) => 2,
                    _ => 1,
                };
                let v = SKY[c] * gain(y) * scale;
                if v >= 1.0 { (WHITE + 0.05 * range) as u16 } else { (BLACK + v * range).round() as u16 }
            })
            .collect()
    };
    // Rows where green and blue clip and red is still well short of it
    // (gain 1.3–1.6), away from where rebuilt colour starts going white.
    let rows = (h * 28 / 100, h * 45 / 100);
    let mean = |img: &[u8], iw: u32| {
        let mut m = [0.0f64; 3];
        let mut n = 0.0;
        for y in rows.0..rows.1 {
            for x in 16..iw as usize - 16 {
                let i = (y * iw as usize + x) * 4;
                for c in 0..3 {
                    m[c] += img[i + c] as f64;
                }
                n += 1.0;
            }
        }
        m.map(|v| v / n)
    };
    runtime().block_on(async {
        let ctx = SharedContext::new().await.expect("gpu");
        let pulled = |exposure| EditParams { exposure, ..EditParams::default() };
        let (clipped, cw, _) = render(&ctx, &capture(1.0), w, h, &pulled(-1.5), None).await;
        let (truth, _, _) = render(&ctx, &capture(1.0 / HEADROOM), w, h, &pulled(-1.5 + HEADROOM.log2()), None).await;
        let (got, want) = (mean(&clipped, cw), mean(&truth, cw));
        eprintln!("clipped sky {got:.1?}, truth {want:.1?}");
        for c in 0..3 {
            assert!((got[c] - want[c]).abs() < 4.0, "channel {c}: {got:.1?} vs truth {want:.1?}");
        }
        assert!(got[1] > got[0] + 2.0, "green not above red — the purple cast: {got:.1?}");
    });
}
