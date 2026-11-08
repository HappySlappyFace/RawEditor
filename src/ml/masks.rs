//! AI mask bitmaps: the data, its orientation, and its files.
//!
//! An AI mask is a raster, so unlike every other mask it cannot live in
//! `EditParams` (fixed-size, `Copy`). The edit stores only an id
//! (`MaskParams::ai_id`); the bitmap and the prompts that made it are files:
//!
//! `paths::ai_masks_dir()/<image id>/<ai id>.png` — 8-bit grey coverage
//! `paths::ai_masks_dir()/<image id>/<ai id>.json` — the prompts (for Refine)
//!
//! Rules, the same ones edited proxies follow (CLAUDE.md §F):
//! - **Immutable files.** Every change writes a new id, so undo/redo and the
//!   before/after peek simply point at older files, and iced/GPU caches keyed
//!   by id are never stale.
//! - **A saved edit's id always has its file.** The id is set only once the
//!   PNG is on disk.
//! - Files no saved edit refers to are swept at startup.
//!
//! Bitmaps are stored in the **sensor frame** (the mosaic's own orientation,
//! before EXIF rotation and quarter turns), so turning the image never has to
//! touch them; `to_display` rotates one into the frame the shader samples.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// A grey coverage raster, row-major, 0 = outside, 255 = inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// One click: a point in **sensor** UV, adding to or subtracting from the
/// selection.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub u: f32,
    pub v: f32,
    pub positive: bool,
}

/// Everything the user asked for, in sensor UV.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Prompts {
    pub points: Vec<Point>,
    /// Two opposite corners `[u0, v0, u1, v1]`, if a box was dragged.
    #[serde(default)]
    pub rect: Option<[f32; 4]>,
}

impl Prompts {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.rect.is_none()
    }
}

/// Rotate a sensor-frame raster into display orientation `orientation`
/// (1/3/6/8, `color::compose_orientation`). Pixel-centre exact for these
/// four rotations, so `to_sensor(to_display(b)) == b`.
pub fn to_display<T: Copy>(src: &[T], w: usize, h: usize, orientation: u32) -> (Vec<T>, usize, usize) {
    let (dw, dh) = if matches!(orientation, 6 | 8) { (h, w) } else { (w, h) };
    let mut out = Vec::with_capacity(dw * dh);
    for y in 0..dh {
        for x in 0..dw {
            let (u, v) = ((x as f32 + 0.5) / dw as f32, (y as f32 + 0.5) / dh as f32);
            let (su, sv) = crate::color::display_to_sensor_uv(u, v, orientation);
            let sx = ((su * w as f32) as usize).min(w - 1);
            let sy = ((sv * h as f32) as usize).min(h - 1);
            out.push(src[sy * w + sx]);
        }
    }
    (out, dw, dh)
}

/// The inverse of [`to_display`]: `w×h` are the DISPLAY raster's dims.
pub fn to_sensor<T: Copy>(src: &[T], w: usize, h: usize, orientation: u32) -> (Vec<T>, usize, usize) {
    let (sw, sh) = if matches!(orientation, 6 | 8) { (h, w) } else { (w, h) };
    let mut out = Vec::with_capacity(sw * sh);
    for y in 0..sh {
        for x in 0..sw {
            let (u, v) = ((x as f32 + 0.5) / sw as f32, (y as f32 + 0.5) / sh as f32);
            let (du, dv) = crate::color::sensor_to_display_uv(u, v, orientation);
            let dx = ((du * w as f32) as usize).min(w - 1);
            let dy = ((dv * h as f32) as usize).min(h - 1);
            out.push(src[dy * w + dx]);
        }
    }
    (out, sw, sh)
}

impl Bitmap {
    pub fn to_display(&self, orientation: u32) -> Bitmap {
        let (data, w, h) = to_display(&self.data, self.width as usize, self.height as usize, orientation);
        Bitmap { width: w as u32, height: h as u32, data }
    }

    /// Bilinear resample to `w×h` (the AI layer's size).
    pub fn resized(&self, w: u32, h: u32) -> Bitmap {
        if (w, h) == (self.width, self.height) {
            return self.clone();
        }
        let (sw, sh) = (self.width as f32, self.height as f32);
        let mut data = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            let fy = ((y as f32 + 0.5) * sh / h as f32 - 0.5).clamp(0.0, sh - 1.0);
            let (y0, ty) = (fy as usize, fy.fract());
            let y1 = (y0 + 1).min(self.height as usize - 1);
            for x in 0..w {
                let fx = ((x as f32 + 0.5) * sw / w as f32 - 0.5).clamp(0.0, sw - 1.0);
                let (x0, tx) = (fx as usize, fx.fract());
                let x1 = (x0 + 1).min(self.width as usize - 1);
                let at = |xx: usize, yy: usize| self.data[yy * self.width as usize + xx] as f32;
                let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
                let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
                data.push((top * (1.0 - ty) + bottom * ty).round() as u8);
            }
        }
        Bitmap { width: w, height: h, data }
    }
}

/// A fresh, never-reused id: microseconds since the epoch, nudged past the
/// last one handed out (two masks in the same microsecond, or a clock step
/// backwards, still get distinct ids). Never 0 — that means "no bitmap".
pub fn new_id() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(1)
        .max(1);
    let mut last = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(last + 1);
        match LAST.compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(seen) => last = seen,
        }
    }
}

fn image_dir(image_id: i64) -> PathBuf {
    crate::core::paths::ai_masks_dir().join(image_id.to_string())
}

pub fn png_path(image_id: i64, ai_id: u64) -> PathBuf {
    image_dir(image_id).join(format!("{ai_id}.png"))
}

fn prompts_path(image_id: i64, ai_id: u64) -> PathBuf {
    image_dir(image_id).join(format!("{ai_id}.json"))
}

/// Write a bitmap and its prompts. Blocking. The PNG goes in last-and-atomic
/// (temp + rename), so a crash never leaves a truncated mask under a real id.
pub fn save(image_id: i64, ai_id: u64, bitmap: &Bitmap, prompts: &Prompts) -> Result<(), String> {
    let dir = image_dir(image_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let json = serde_json::to_string(prompts).map_err(|e| e.to_string())?;
    std::fs::write(prompts_path(image_id, ai_id), json).map_err(|e| format!("could not save prompts: {e}"))?;

    let final_path = png_path(image_id, ai_id);
    let tmp = final_path.with_extension("png.part");
    let encode = || -> Result<(), String> {
        let file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), bitmap.width, bitmap.height);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&bitmap.data).map_err(|e| e.to_string())?;
        w.finish().map_err(|e| e.to_string())
    };
    encode().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not save the AI mask: {e}")
    })?;
    std::fs::rename(&tmp, &final_path).map_err(|e| format!("could not save the AI mask: {e}"))
}

/// Read a bitmap back. Blocking.
pub fn load(image_id: i64, ai_id: u64) -> Result<Bitmap, String> {
    let path = png_path(image_id, ai_id);
    let file = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Grayscale || info.bit_depth != png::BitDepth::Eight {
        return Err(format!("{}: not an 8-bit grey mask", path.display()));
    }
    buf.truncate(info.buffer_size());
    Ok(Bitmap { width: info.width, height: info.height, data: buf })
}

pub fn load_prompts(image_id: i64, ai_id: u64) -> Prompts {
    std::fs::read_to_string(prompts_path(image_id, ai_id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Delete every mask file no saved edit refers to. `referenced` holds
/// `(image id, ai id)` pairs. Blocking; run once at startup.
pub fn sweep(referenced: &HashSet<(i64, u64)>) -> usize {
    let Ok(images) = std::fs::read_dir(crate::core::paths::ai_masks_dir()) else {
        return 0;
    };
    let mut removed = 0;
    for image in images.flatten() {
        let Some(image_id) = image.file_name().to_str().and_then(|s| s.parse::<i64>().ok()) else {
            continue;
        };
        let Ok(files) = std::fs::read_dir(image.path()) else { continue };
        for file in files.flatten() {
            let name = file.file_name();
            let Some(stem) = name.to_str().and_then(|n| n.split('.').next()) else { continue };
            let keep = stem.parse::<u64>().map(|id| referenced.contains(&(image_id, id))).unwrap_or(false);
            if !keep && std::fs::remove_file(file.path()).is_ok() {
                removed += 1;
            }
        }
        let _ = std::fs::remove_dir(image.path()); // only succeeds when empty
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(w: usize, h: usize) -> Vec<u32> {
        (0..(w * h) as u32).collect()
    }

    #[test]
    fn rotating_to_display_and_back_is_the_identity() {
        let (w, h) = (7, 4);
        let src = raster(w, h);
        for o in [1, 3, 6, 8] {
            let (d, dw, dh) = to_display(&src, w, h, o);
            assert_eq!((dw, dh), if o == 6 || o == 8 { (h, w) } else { (w, h) });
            let (back, bw, bh) = to_sensor(&d, dw, dh, o);
            assert_eq!((bw, bh), (w, h));
            assert_eq!(back, src, "orientation {o}");
        }
    }

    /// The display raster must agree with `display_to_sensor_uv` — the
    /// mapping the shader and every click use — pixel for pixel.
    #[test]
    fn display_rasters_agree_with_the_shader_mapping() {
        let (w, h) = (6, 4);
        let src = raster(w, h);
        for exif in [1, 3, 6, 8] {
            for turns in 0..4 {
                let o = crate::color::compose_orientation(exif, turns);
                let (d, dw, dh) = to_display(&src, w, h, o);
                for y in 0..dh {
                    for x in 0..dw {
                        let (u, v) = ((x as f32 + 0.5) / dw as f32, (y as f32 + 0.5) / dh as f32);
                        let (su, sv) = crate::color::display_to_sensor_uv(u, v, o);
                        let s = (sv * h as f32) as usize * w + (su * w as f32) as usize;
                        assert_eq!(d[y * dw + x], src[s]);
                    }
                }
            }
        }
    }

    #[test]
    fn sensor_and_display_uv_are_inverse() {
        for o in [1, 3, 6, 8] {
            for &(u, v) in &[(0.1f32, 0.7f32), (0.9, 0.2), (0.5, 0.5)] {
                let (su, sv) = crate::color::display_to_sensor_uv(u, v, o);
                let (du, dv) = crate::color::sensor_to_display_uv(su, sv, o);
                assert!((du - u).abs() < 1e-6 && (dv - v).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn ids_are_unique_and_never_zero() {
        let ids: HashSet<u64> = (0..1000).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 1000);
        assert!(!ids.contains(&0));
    }

    #[test]
    fn resizing_keeps_flat_regions_flat() {
        let b = Bitmap { width: 4, height: 2, data: vec![0, 0, 255, 255, 0, 0, 255, 255] };
        let r = b.resized(8, 4);
        assert_eq!(r.data[0], 0);
        assert_eq!(r.data[7], 255);
        assert_eq!(b.resized(4, 2), b);
    }
}
