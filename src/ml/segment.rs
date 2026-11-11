//! Click-to-select segmentation with SAM 2.1 (hiera-tiny), CPU.
//!
//! Two graphs (`tools/models/export.py`):
//! - the **encoder** turns a 1024² image into embeddings — ~1–3 s, run once
//!   per image when the AI-select tool opens. Its session is dropped as soon
//!   as it finishes (it is the 109 MB one).
//! - the **decoder** turns embeddings + clicks into 256² mask logits —
//!   ~20 ms, run per click while the tool is open.
//!
//! The image SAM sees is built from the RAW mosaic itself (2×2 binned,
//! white-balanced, camera matrix, auto-exposed), not from the camera's
//! embedded JPEG: the JPEG's framing can differ from the sensor's active area
//! by a few pixels, and the mask must line up with the pixels being edited.
//! It is rotated upright for the encoder (SAM was trained on upright photos);
//! everything stored is converted back to the sensor frame.

use ort::session::Session;

use super::masks::{to_display, to_sensor, Bitmap, Prompts};
use super::models::{file_path, SEGMENT};
use super::runtime;
use crate::raw::loader::RawDataResult;

const SIZE: usize = 1024;
const LOW_RES: usize = 256;
/// Long side of a stored mask: enough that a mask edge stays sharper than
/// the guided filter's own radius at the develop preview.
const MASK_LONG_SIDE: usize = 2048;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// What the model sees, and the guide its output is refined against.
pub struct SamInput {
    /// `1×3×1024×1024`, ImageNet-normalised, upright (`orientation`).
    image: Vec<f32>,
    /// The display orientation the image was rotated to.
    pub orientation: u32,
    /// Luma in the SENSOR frame, at the stored mask's size.
    guide: Vec<f32>,
    pub mask_w: usize,
    pub mask_h: usize,
}

/// The encoder's output for one image.
pub struct Embedding {
    embed: Vec<f32>,
    high_res_0: Vec<f32>,
    high_res_1: Vec<f32>,
}

/// Build the encoder input from a mosaic. Blocking (~100 ms at 24 MP).
pub fn prepare(raw: &RawDataResult, black_offsets: [f32; 4], orientation: u32) -> SamInput {
    let (w, h) = (raw.width as usize, raw.height as usize);
    let (hw, hh) = (w / 2, h / 2);
    let dp = crate::gpu::demosaic::GpuDemosaicParams::new(
        raw.cfa_pattern,
        raw.black_levels,
        black_offsets,
        raw.white_level,
        raw.wb_multipliers,
    );
    let colors = crate::gpu::demosaic::tile_colors(raw.cfa_pattern);
    let m = crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65);

    // 2×2 binned linear sRGB.
    let mut lin = vec![[0f32; 3]; hw * hh];
    for ty in 0..hh {
        for tx in 0..hw {
            let mut cam = [0f32; 3];
            let mut n = [0f32; 3];
            for (k, &c) in colors.iter().enumerate() {
                let (x, y) = (2 * tx + (k & 1), 2 * ty + (k >> 1));
                let raw_v = raw.data[y * w + x] as f32;
                let c = c as usize;
                cam[c] += ((raw_v - dp.black[k]).max(0.0) * dp.scale[k]).min(dp.ceil[c]);
                n[c] += 1.0;
            }
            let cam = [cam[0] / n[0].max(1.0), cam[1] / n[1].max(1.0), cam[2] / n[2].max(1.0)];
            lin[ty * hw + tx] = [
                (m[0] * cam[0] + m[1] * cam[1] + m[2] * cam[2]).max(0.0),
                (m[3] * cam[0] + m[4] * cam[1] + m[5] * cam[2]).max(0.0),
                (m[6] * cam[0] + m[7] * cam[1] + m[8] * cam[2]).max(0.0),
            ];
        }
    }

    // Auto-exposure: median luma to mid-grey. SAM cares about structure, not
    // the user's look, and an underexposed frame segments worse.
    let luma = |p: &[f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let mut sample: Vec<f32> = lin.iter().step_by(17).map(luma).collect();
    let mid = sample.len() / 2;
    let median = if sample.is_empty() {
        0.18
    } else {
        *sample.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
    };
    let gain = (0.18 / median.max(1e-5)).clamp(0.25, 64.0);
    let encode = |v: f32| {
        let v = (v * gain).clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u8
    };
    let srgb: Vec<[u8; 3]> = lin.iter().map(|p| [encode(p[0]), encode(p[1]), encode(p[2])]).collect();
    drop(lin);

    // The guide: sensor frame, mask size.
    let s = (MASK_LONG_SIDE as f32 / hw.max(hh) as f32).min(1.0);
    let (mask_w, mask_h) = (((hw as f32 * s).round() as usize).max(1), ((hh as f32 * s).round() as usize).max(1));
    let sensor_img = rgb_image(&srgb, hw, hh);
    let small = image::imageops::resize(&sensor_img, mask_w as u32, mask_h as u32, image::imageops::FilterType::Triangle);
    let guide = small
        .pixels()
        .map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0)
        .collect();

    // The encoder input: upright, squashed to 1024² (SAM 2's own transform).
    let (upright, uw, uh) = to_display(&srgb, hw, hh, orientation);
    let upright = rgb_image(&upright, uw, uh);
    let square = image::imageops::resize(&upright, SIZE as u32, SIZE as u32, image::imageops::FilterType::Triangle);
    let mut image = vec![0f32; 3 * SIZE * SIZE];
    for (i, p) in square.pixels().enumerate() {
        for c in 0..3 {
            image[c * SIZE * SIZE + i] = (p[c] as f32 / 255.0 - MEAN[c]) / STD[c];
        }
    }
    SamInput { image, orientation, guide, mask_w, mask_h }
}

fn rgb_image(px: &[[u8; 3]], w: usize, h: usize) -> image::RgbImage {
    image::RgbImage::from_raw(w as u32, h as u32, px.iter().flatten().copied().collect())
        .expect("buffer sized w×h×3")
}

/// Run the encoder. Blocking (seconds); loads and drops its own session.
pub fn embed(input: &SamInput) -> Result<Embedding, String> {
    let t = std::time::Instant::now();
    let mut session = runtime::session(&file_path(&SEGMENT.files[0]))?;
    let image = runtime::tensor(&[1, 3, SIZE, SIZE], input.image.clone())?;
    let outputs = session.run(ort::inputs!["image" => image]).map_err(|e| format!("AI select: {e}"))?;
    let (_, embed) = runtime::extract(&outputs, "image_embed")?;
    let (_, high_res_0) = runtime::extract(&outputs, "high_res_0")?;
    let (_, high_res_1) = runtime::extract(&outputs, "high_res_1")?;
    tracing::info!("AI select: image embedded in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    Ok(Embedding { embed, high_res_0, high_res_1 })
}

/// The per-click half. Holds the small decoder session while the tool is open.
pub struct Decoder {
    session: Session,
}

/// One decoder answer: 256² logits over the upright image, kept to feed the
/// next click (SAM refines from its previous mask).
#[derive(Clone)]
pub struct Logits(Vec<f32>);

impl Decoder {
    /// Blocking.
    pub fn new() -> Result<Self, String> {
        Ok(Self { session: runtime::session(&file_path(&SEGMENT.files[1]))? })
    }

    /// Masks for `prompts` (sensor UV). Blocking (~20 ms).
    pub fn decode(
        &mut self,
        emb: &Embedding,
        input: &SamInput,
        prompts: &Prompts,
        previous: Option<&Logits>,
    ) -> Result<Logits, String> {
        let up = |u: f32, v: f32| {
            let (du, dv) = crate::color::sensor_to_display_uv(u, v, input.orientation);
            [du * SIZE as f32, dv * SIZE as f32]
        };
        let mut coords = Vec::new();
        let mut labels = Vec::new();
        // Box corners first, as SAM's predictor orders them.
        if let Some([u0, v0, u1, v1]) = prompts.rect {
            let (a, b) = (up(u0, v0), up(u1, v1));
            coords.extend([a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]);
            labels.extend([2.0, 3.0]);
        }
        for p in &prompts.points {
            coords.extend(up(p.u, p.v));
            labels.push(if p.positive { 1.0 } else { 0.0 });
        }
        let n = labels.len();
        if n == 0 {
            return Err("nothing selected".into());
        }
        let (mask_input, has_mask) = match previous {
            Some(l) => (l.0.clone(), 1.0),
            None => (vec![0.0; LOW_RES * LOW_RES], 0.0),
        };
        let inputs = ort::inputs![
            "image_embed" => runtime::tensor(&[1, 256, 64, 64], emb.embed.clone())?,
            "high_res_0" => runtime::tensor(&[1, 32, 256, 256], emb.high_res_0.clone())?,
            "high_res_1" => runtime::tensor(&[1, 64, 128, 128], emb.high_res_1.clone())?,
            "point_coords" => runtime::tensor(&[1, n, 2], coords)?,
            "point_labels" => runtime::tensor(&[1, n], labels)?,
            "mask_input" => runtime::tensor(&[1, 1, LOW_RES, LOW_RES], mask_input)?,
            "has_mask_input" => runtime::tensor(&[1], vec![has_mask])?,
        ];
        let outputs = self.session.run(inputs).map_err(|e| format!("AI select: {e}"))?;
        let (_, masks) = runtime::extract(&outputs, "masks")?;
        let (_, iou) = runtime::extract(&outputs, "iou")?;
        let pick = choose_mask(&iou, prompts);
        let plane = LOW_RES * LOW_RES;
        Ok(Logits(masks[pick * plane..(pick + 1) * plane].to_vec()))
    }
}

/// SAM's multimask rule: a lone click is ambiguous (the shirt, the person,
/// the group?), so take the best-scored of the three multimask tokens; any
/// richer prompt is not, so take the single-mask token.
fn choose_mask(iou: &[f32], prompts: &Prompts) -> usize {
    if prompts.points.len() == 1 && prompts.rect.is_none() && iou.len() >= 4 {
        (1..4).max_by(|&a, &b| iou[a].total_cmp(&iou[b])).unwrap_or(0)
    } else {
        0
    }
}

/// Logits → a stored, sensor-frame, edge-refined coverage bitmap. Blocking.
pub fn to_bitmap(logits: &Logits, input: &SamInput) -> Bitmap {
    let (sensor, _, _) = to_sensor(&logits.0, LOW_RES, LOW_RES, input.orientation);
    let (w, h) = (input.mask_w, input.mask_h);
    // Bilinear upsample of the LOGITS (not the probabilities): the boundary
    // stays a smooth zero crossing instead of a staircase.
    let mut p = vec![0f32; w * h];
    let n = LOW_RES as f32;
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * n / h as f32 - 0.5).clamp(0.0, n - 1.0);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(LOW_RES - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * n / w as f32 - 0.5).clamp(0.0, n - 1.0);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(LOW_RES - 1);
            let at = |xx: usize, yy: usize| sensor[yy * LOW_RES + xx];
            let l = (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty)
                + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty;
            p[y * w + x] = 1.0 / (1.0 + (-l).exp());
        }
    }
    // Snap onto the photo's own edges. The radius spans about one logit cell
    // — the scale of the error being corrected.
    let r = (w.max(h) / LOW_RES).max(2);
    let q = crate::core::guided::filter(&input.guide, &p, w, h, r, 1e-3);
    Bitmap {
        width: w as u32,
        height: h as u32,
        data: q.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ml::masks::Point;

    #[test]
    fn a_lone_click_takes_the_best_multimask_token() {
        let one = Prompts { points: vec![Point { u: 0.5, v: 0.5, positive: true }], rect: None };
        assert_eq!(choose_mask(&[0.99, 0.2, 0.9, 0.5], &one), 2);
        let two = Prompts { points: vec![one.points[0]; 2], rect: None };
        assert_eq!(choose_mask(&[0.1, 0.2, 0.9, 0.5], &two), 0);
        let boxed = Prompts { points: one.points.clone(), rect: Some([0.1, 0.1, 0.4, 0.4]) };
        assert_eq!(choose_mask(&[0.1, 0.2, 0.9, 0.5], &boxed), 0);
    }

    pub(crate) fn disc_raw(w: u32, h: u32) -> RawDataResult {
        // A bright disc on a dark field, RGGB, as a minimal "photo".
        let mut data = vec![0u16; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 - w as f32 * 0.4, y as f32 - h as f32 * 0.5);
                let inside = (dx * dx + dy * dy).sqrt() < h as f32 * 0.25;
                data[(y * w + x) as usize] = if inside { 3000 } else { 300 };
            }
        }
        RawDataResult {
            data,
            width: w,
            height: h,
            wb_multipliers: [2.0, 1.0, 1.5, 1.0],
            color_matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            cfa_pattern: 0,
            black_levels: [0; 4],
            white_level: 4095,
            color_matrix_is_d65: true,
            crops: [0; 4],
            cfa_name: "RGGB".into(),
            measured_black_levels: [0.0; 4],
            make: String::new(),
            model: String::new(),
            iso: String::new(),
            shutter_speed: String::new(),
            aperture: String::new(),
            lens: String::new(),
            dcp_profile: None,
            lens_context: Default::default(),
            orientation: 1,
        }
    }

    #[test]
    fn the_guide_is_in_the_sensor_frame_whatever_the_display_orientation() {
        let raw = disc_raw(400, 240);
        for o in [1, 6] {
            let input = prepare(&raw, [0.0; 4], o);
            assert_eq!((input.mask_w, input.mask_h), (200, 120));
            assert_eq!(input.image.len(), 3 * SIZE * SIZE);
            // Disc centre (sensor 0.4, 0.5) is bright in the guide either way.
            let g = |u: f32, v: f32| input.guide[(v * 120.0) as usize * 200 + (u * 200.0) as usize];
            assert!(g(0.4, 0.5) > g(0.95, 0.1) + 0.3, "orientation {o}");
        }
    }

    /// End to end on the real model (needs `RAWEDITOR_MODELS_DIR`): a click
    /// on the disc selects the disc and not the field, in any orientation.
    #[test]
    #[ignore]
    fn clicking_a_disc_selects_it() {
        let raw = disc_raw(1200, 800);
        for o in [1, 6] {
            let input = prepare(&raw, [0.0; 4], o);
            let emb = embed(&input).expect("encoder (set RAWEDITOR_MODELS_DIR)");
            let mut dec = Decoder::new().unwrap();
            let prompts = Prompts { points: vec![Point { u: 0.4, v: 0.5, positive: true }], rect: None };
            let logits = dec.decode(&emb, &input, &prompts, None).unwrap();
            let b = to_bitmap(&logits, &input);
            let at = |u: f32, v: f32| b.data[(v * b.height as f32) as usize * b.width as usize + (u * b.width as f32) as usize];
            assert!(at(0.4, 0.5) > 200, "orientation {o}: centre {}", at(0.4, 0.5));
            assert!(at(0.9, 0.1) < 50, "orientation {o}: field {}", at(0.9, 0.1));
        }
    }
}
