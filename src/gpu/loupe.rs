//! The detail loupe: a true 1:1 render of a small window of the image.
//!
//! Noise reduction is the one stage too expensive to re-run per slider tick
//! over the whole image (~30 ms on the develop preview, ~270 ms at full
//! resolution on a 24 MP sensor). So, as in Lightroom's Detail panel, the main
//! view applies it when a drag pauses or ends, and this window shows it live
//! — at sensor resolution, which is the only scale grain can be judged at.
//!
//! ## How
//! A CFA-aligned crop of the full-resolution mosaic, large enough to cover
//! the window plus every stage's reach, goes through the real passes:
//! demosaic → denoise → the colour pass. Each of those is a neighbourhood
//! operation of bounded reach, so pixels further than that reach from the
//! crop's edge come out exactly as a whole-image render would produce them.
//!
//! The colour pass is the unmodified one. Its input is the crop rather than
//! the whole image, and `GpuEditParams::src_scale`/`src_offset` map
//! full-image UV into it; every whole-image texture it also reads (Texture /
//! Clarity bands, haze map, brush masks, LUTs) is the main image's own, so
//! the loupe cannot drift from the look of the main view or the export.
//!
//! Cost per update: one colour pass over the window (~65 k pixels), plus —
//! only when the noise sliders or the crop change — denoising ~0.3 MP.

use std::sync::{Arc, Mutex};

use iced_wgpu::wgpu;

use super::denoise::{DenoiseSettings, DenoiseTargets};
use super::params::GpuEditParams;
use super::shared::{ColorBindings, ImageResources, SharedContext};
use crate::core::noise::NoiseModel;
use crate::raw::loader::RawDataResult;

/// How far the demosaic's four RCD passes reach, in sensor pixels.
const DEMOSAIC_REACH: u32 = 16;
/// The colour pass's own neighbourhood: 5×5 sharpening, defringe's 3-texel
/// edge search, bilinear taps and lateral-CA shifts.
const COLOR_REACH: u32 = 8;
/// Extra crop around what a window needs, so panning a little re-uses the
/// crop already on the GPU instead of re-cutting it.
const SLACK: u32 = 48;

/// A rectangle of sensor pixels. `x` and `y` are even, so the crop keeps the
/// mosaic's CFA phase (the demosaic's tile lookup is `(y & 1) * 2 + (x & 1)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Region {
    fn contains(&self, o: &Region) -> bool {
        o.x >= self.x && o.y >= self.y && o.x + o.w <= self.x + self.w && o.y + o.h <= self.y + self.h
    }

    /// `[x0, y0, x1, y1]` (sensor pixels, fractional) grown by `pad` and
    /// clamped to the sensor, with an even origin. Never empty and never off
    /// the sensor, even for a box that lies wholly outside it (lens
    /// correction and straightening can map a window's edge past the frame).
    fn around(bbox: [f32; 4], pad: u32, sensor: (u32, u32)) -> Self {
        let pad = pad as f32;
        let axis = |lo: f32, hi: f32, len: u32| {
            let start = ((lo - pad).floor().max(0.0) as u32).min(len.saturating_sub(2)) & !1;
            let end = ((hi + pad).ceil().max(0.0) as u32).clamp(start + 2, len.max(start + 2));
            (start, end - start)
        };
        let (x, w) = axis(bbox[0], bbox[2], sensor.0);
        let (y, h) = axis(bbox[1], bbox[3], sensor.1);
        Region { x, y, w, h }
    }
}

/// The view-UV window (`view_rect` form) a `size`-pixel square shows at 1:1,
/// centred on `center` (view UV — the post-crop display image) and kept
/// inside the image.
pub fn view_window(gp: &GpuEditParams, center: [f32; 2], size: u32, sensor: (u32, u32)) -> [f32; 4] {
    let (dw, dh) = match gp.orientation {
        6 | 8 => (sensor.1, sensor.0),
        _ => sensor,
    };
    let crop = if gp.is_cropping == 1 { [0.0, 0.0, 1.0, 1.0] } else { gp.crop };
    let du = (size as f32 / (crop[2] * dw as f32).max(1.0)).min(1.0);
    let dv = (size as f32 / (crop[3] * dh as f32).max(1.0)).min(1.0);
    let cx = center[0].clamp(du / 2.0, 1.0 - du / 2.0);
    let cy = center[1].clamp(dv / 2.0, 1.0 - dv / 2.0);
    [cx - du / 2.0, cy - dv / 2.0, du, dv]
}

/// Where in the sensor (UV, 0..1 across the unrotated mosaic) the colour
/// pass samples for the view-UV point `view` — a CPU mirror of its vertex
/// mapping (crop), Phase-52 straightening, EXIF orientation and Step 0's
/// geometric lens correction. Lateral CA is left out: it moves red and blue
/// by a few pixels, which `COLOR_REACH` covers.
pub fn view_to_sensor_uv(gp: &GpuEditParams, view: [f32; 2], sensor: (u32, u32)) -> [f32; 2] {
    let mut t = if gp.is_cropping == 0 {
        [gp.crop[0] + view[0] * gp.crop[2], gp.crop[1] + view[1] * gp.crop[3]]
    } else {
        view
    };

    if gp.rotation.abs() > 0.01 {
        let mut aspect = sensor.0 as f32 / sensor.1.max(1) as f32;
        if gp.orientation == 6 || gp.orientation == 8 {
            aspect = 1.0 / aspect;
        }
        let (sin_a, cos_a) = gp.rotation.to_radians().sin_cos();
        let (cx, cy) = ((t[0] - 0.5) * aspect, t[1] - 0.5);
        t = [(cx * cos_a - cy * sin_a) / aspect + 0.5, cx * sin_a + cy * cos_a + 0.5];
    }

    t = match gp.orientation {
        6 => [t[1], 1.0 - t[0]],
        8 => [1.0 - t[1], t[0]],
        3 => [1.0 - t[0], 1.0 - t[1]],
        _ => t,
    };

    if gp.lens_misc[3] > 0.5 {
        let scale = [gp.lens_misc[1], gp.lens_misc[2]];
        let p = [(t[0] - 0.5) * scale[0], (t[1] - 0.5) * scale[1]];
        let ru = (p[0] * p[0] + p[1] * p[1]).sqrt();
        let d = gp.lens_dist;
        let f = d[0] + ru * (d[1] + ru * (d[2] + ru * (d[3] + ru * gp.lens_misc[0])));
        t = [p[0] * f / scale[0] + 0.5, p[1] * f / scale[1] + 0.5];
    }
    t
}

/// The sensor pixels a window's render reads: its outline mapped through
/// [`view_to_sensor_uv`] (sampled along the edges — distortion bends them),
/// as `[x0, y0, x1, y1]`.
fn footprint(gp: &GpuEditParams, window: [f32; 4], sensor: (u32, u32)) -> [f32; 4] {
    const STEPS: usize = 6;
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for i in 0..=STEPS {
        let s = i as f32 / STEPS as f32;
        for [u, v] in [[s, 0.0], [s, 1.0], [0.0, s], [1.0, s], [0.5, 0.5]] {
            let uv = view_to_sensor_uv(gp, [window[0] + u * window[2], window[1] + v * window[3]], sensor);
            let (x, y) = (uv[0] * sensor.0 as f32, uv[1] * sensor.1 as f32);
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
    }
    b
}

/// Copy `region` out of a row-major mosaic `width` pixels wide.
fn extract(mosaic: &[u16], width: u32, region: Region) -> Vec<u16> {
    let mut out = Vec::with_capacity((region.w * region.h) as usize);
    for row in region.y..region.y + region.h {
        let start = (row * width + region.x) as usize;
        out.extend_from_slice(&mosaic[start..start + region.w as usize]);
    }
    out
}

/// One rendered loupe frame.
#[derive(Clone)]
pub struct LoupeFrame {
    /// `size`×`size` tightly packed RGBA.
    pub rgba: Vec<u8>,
    pub size: u32,
    /// The view-UV window it shows (`view_rect` form): converts a drag in
    /// loupe pixels into a move of its centre.
    pub window: [f32; 4],
}

impl std::fmt::Debug for LoupeFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoupeFrame").field("size", &self.size).field("window", &self.window).finish()
    }
}

/// A mosaic to show in place of the RAW's own, over `region` — AI denoise's
/// preview (`handlers::ai_denoise`). `key` identifies its content, so the GPU
/// crop re-uploads exactly when it changes.
pub struct MosaicOverride {
    pub region: Region,
    pub data: Vec<u16>,
    pub key: u64,
}

/// The crop currently on the GPU.
struct Crop {
    region: Region,
    /// The override it was cut from (`None` = the RAW itself).
    key: Option<u64>,
    #[allow(dead_code)] // owner: the views below read it
    mosaic: wgpu::Texture,
    #[allow(dead_code)] // owner
    debayer: wgpu::Texture,
    debayer_view: wgpu::TextureView,
    targets: DenoiseTargets,
    /// The denoise settings `targets` holds a result for, and which target.
    denoised: Option<(DenoiseSettings, usize)>,
}

/// Per-image loupe state: the full-resolution mosaic it cuts from, the
/// full-resolution noise model, and the crop on the GPU.
pub struct Loupe {
    pub image_id: i64,
    raw: Arc<RawDataResult>,
    /// The develop preview's own model describes its box-filtered (quieter)
    /// mosaic; the loupe denoises sensor pixels and needs theirs.
    noise_model: NoiseModel,
    black_offsets: [f32; 4],
    /// The AI denoise amount baked into `raw` (`ImageResources::ai_denoise`).
    ai_denoise: f32,
    uniform: wgpu::Buffer,
    crop: Mutex<Option<Crop>>,
}

impl std::fmt::Debug for Loupe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loupe").field("image_id", &self.image_id).finish_non_exhaustive()
    }
}

impl Loupe {
    /// Blocking: estimates the full-resolution noise model when `main` is a
    /// subsampled preview (a pass over the whole mosaic). Call it inside
    /// `spawn_blocking`.
    pub fn new(context: &SharedContext, main: &ImageResources, raw: Arc<RawDataResult>) -> Result<Self, String> {
        if (raw.width, raw.height) != (main.original_width, main.original_height) {
            return Err("RAW data does not match the loaded image".to_string());
        }
        if raw.data.len() < (raw.width as usize) * (raw.height as usize) {
            return Err("RAW buffer is shorter than the image".to_string());
        }
        let noise_model = if main.width == main.original_width {
            main.noise_model
        } else {
            let dp = demosaic_params(main);
            crate::core::noise::estimate(
                &raw.data,
                raw.width as usize,
                raw.height as usize,
                super::demosaic::tile_colors(main.cfa_pattern),
                dp.black,
                dp.scale,
                [dp.ceil[0], dp.ceil[1], dp.ceil[2]],
            )
        };
        let uniform = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Loupe uniforms"),
            size: std::mem::size_of::<GpuEditParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            image_id: main.image_id,
            raw,
            noise_model,
            black_offsets: main.black_offsets,
            ai_denoise: main.ai_denoise.1,
            uniform,
            crop: Mutex::new(None),
        })
    }

    /// Still valid for `main` (same image, same sensor calibration)?
    pub fn matches(&self, main: &ImageResources) -> bool {
        self.image_id == main.image_id && self.black_offsets == main.black_offsets && self.ai_denoise == main.ai_denoise.1
    }

    fn sensor(&self) -> (u32, u32) {
        (self.raw.width, self.raw.height)
    }

    /// The RAW it cuts from (unmodified — never AI-denoised).
    pub fn raw(&self) -> &Arc<RawDataResult> {
        &self.raw
    }

    /// The sensor region a window needs (every stage's reach) and that region
    /// grown by `slack`.
    fn regions(&self, gp: &GpuEditParams, window: [f32; 4], slack: u32) -> (Region, Region) {
        let sensor = self.sensor();
        let reach = super::denoise::reach(1.0) + DEMOSAIC_REACH + COLOR_REACH;
        let bbox = footprint(gp, window, sensor);
        (Region::around(bbox, reach, sensor), Region::around(bbox, reach + slack, sensor))
    }

    /// The sensor region a `size`-pixel window at `center` needs, grown by
    /// `slack` — what a mosaic override must cover.
    pub fn region_for(&self, main: &ImageResources, center: [f32; 2], size: u32, slack: u32) -> Region {
        let gp = *main.current_params.lock().unwrap_or_else(|e| e.into_inner());
        let window = view_window(&gp, center, size.clamp(16, 2048), self.sensor());
        self.regions(&gp, window, slack).1
    }

    /// Whether `region` covers what a window at `center` needs.
    pub fn covers(&self, main: &ImageResources, center: [f32; 2], size: u32, region: &Region) -> bool {
        let gp = *main.current_params.lock().unwrap_or_else(|e| e.into_inner());
        let window = view_window(&gp, center, size.clamp(16, 2048), self.sensor());
        region.contains(&self.regions(&gp, window, 0).0)
    }

    /// The RAW's own samples over `region`.
    pub fn cut(&self, region: Region) -> Vec<u16> {
        extract(&self.raw.data, self.raw.width, region)
    }

    /// Make sure the GPU crop covers `needed`, cutting a new one if not.
    #[allow(clippy::too_many_arguments)]
    fn ensure_crop<'a>(
        &self,
        context: &SharedContext,
        main: &ImageResources,
        slot: &'a mut Option<Crop>,
        needed: Region,
        grown: Region,
        over: Option<&MosaicOverride>,
    ) -> Result<&'a mut Crop, String> {
        let key = over.map(|o| o.key);
        if slot.as_ref().is_some_and(|c| c.region.contains(&needed) && c.key == key) {
            return slot.as_mut().ok_or_else(|| "loupe crop vanished".to_string());
        }
        let (region, data) = match over {
            Some(o) if o.region.contains(&needed) => (o.region, std::borrow::Cow::Borrowed(&o.data[..])),
            Some(_) => return Err("the preview area moved past the prepared crop".to_string()),
            None => (grown, std::borrow::Cow::Owned(extract(&self.raw.data, self.raw.width, grown))),
        };
        context.check_texture_size(region.w, region.h)?;
        let size = wgpu::Extent3d { width: region.w, height: region.h, depth_or_array_layers: 1 };
        let mosaic = context.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Loupe mosaic"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        context.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &mosaic,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&data[..]),
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(region.w * 2), rows_per_image: Some(region.h) },
            size,
        );
        let debayer = context.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Loupe demosaic"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let debayer_view = debayer.create_view(&wgpu::TextureViewDescriptor::default());
        let mosaic_view = mosaic.create_view(&wgpu::TextureViewDescriptor::default());
        context.demosaic.run(
            &context.device,
            &context.queue,
            &mosaic_view,
            region.w,
            region.h,
            &demosaic_params(main),
            &debayer_view,
        );
        let targets = context.denoise.targets(&context.device, &debayer_view, region.w, region.h);
        *slot = Some(Crop { region, key, mosaic, debayer, debayer_view, targets, denoised: None });
        slot.as_mut().ok_or_else(|| "loupe crop vanished".to_string())
    }

    /// Render a `size`-pixel square at 1:1, centred on `center` (view UV), with
    /// noise reduction at `denoise` — which may differ from what `main` was
    /// last given, since the main view holds NR back during a drag.
    ///
    /// `main` supplies everything else: its pushed uniforms, LUTs, masks and
    /// whole-image analyses.
    pub async fn render(
        &self,
        context: &SharedContext,
        main: &ImageResources,
        center: [f32; 2],
        size: u32,
        denoise: DenoiseSettings,
        over: Option<&MosaicOverride>,
    ) -> Result<LoupeFrame, String> {
        let size = size.clamp(16, 2048);
        let sensor = self.sensor();
        let mut gp = *main.current_params.lock().unwrap_or_else(|e| e.into_inner());
        let window = view_window(&gp, center, size, sensor);
        let (needed, grown) = self.regions(&gp, window, SLACK);

        // The main image's bind groups 1 and 2 (masks, Texture/Clarity, haze),
        // brought up to date exactly as for its own render.
        let main_bindings = main.prepare_stages(context);

        let bindings = {
            let mut slot = self.crop.lock().unwrap_or_else(|e| e.into_inner());
            let crop = self.ensure_crop(context, main, &mut slot, needed, grown, over)?;
            let input = if denoise.is_active() {
                let out = match crop.denoised {
                    Some((key, out)) if key == denoise => out,
                    _ => {
                        let dp = demosaic_params(main);
                        let passes = super::denoise::plan(
                            denoise,
                            &self.noise_model,
                            [dp.ceil[0], dp.ceil[1], dp.ceil[2]],
                            1.0,
                        );
                        let out = context.denoise.run(&context.device, &context.queue, &crop.targets, &passes);
                        crop.denoised = Some((denoise, out));
                        out
                    }
                };
                crop.targets.view(out)
            } else {
                &crop.debayer_view
            };

            let r = crop.region;
            gp.view_rect = window;
            gp.src_scale = [sensor.0 as f32 / r.w as f32, sensor.1 as f32 / r.h as f32];
            gp.src_offset = [-(r.x as f32) / r.w as f32, -(r.y as f32) / r.h as f32];
            gp.set_defringe_radius(1.0);
            context.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&gp));

            ColorBindings {
                main: Arc::new(main.color_bind_group_with(context, input, &self.uniform)),
                masks: main_bindings.masks,
                presence: main_bindings.presence,
            }
        };

        let rgba = super::render_functions::render_with_bindings(context, &bindings, size, size)
            .await
            .ok_or_else(|| "loupe readback failed".to_string())?;
        Ok(LoupeFrame { rgba, size, window })
    }
}

/// The demosaic's calibration for `main`'s image — also the units the noise
/// model and the denoiser's white balance are expressed in.
fn demosaic_params(main: &ImageResources) -> super::demosaic::GpuDemosaicParams {
    super::demosaic::GpuDemosaicParams::new(
        main.cfa_pattern,
        main.black_levels,
        main.black_offsets,
        main.white_level,
        main.wb_multipliers,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::EditParams;

    const SENSOR: (u32, u32) = (6016, 4016);

    fn params() -> GpuEditParams {
        GpuEditParams::from(&EditParams::default())
    }

    /// The loupe's whole point: what it shows is what a full-resolution
    /// render (the export) shows in that window. Checked with every
    /// neighbourhood stage on — NR, sharpening, defringe — and geometry that
    /// moves the window's footprint (straighten, distortion, CA).
    ///
    /// Run: `cargo test --release loupe_matches -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn loupe_matches_a_full_resolution_render() {
        use crate::gpu::render_functions::render_to_bytes;
        let path = std::env::var("RAWEDITOR_TEST_RAW")
            .unwrap_or_else(|_| "/home/hsf001/Pictures/Photography/TESTFiles/RAW/_DSC0177.NEF".into());
        if !std::path::Path::new(&path).exists() {
            eprintln!("no test RAW at {path}, skipping");
            return;
        }
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        rt.block_on(async {
            let ctx = SharedContext::new().await.unwrap();
            let raw = Arc::new(crate::raw::loader::load_raw_data(path).await.unwrap());
            let mut params = EditParams::default();
            params.luma_noise = 0.5;
            params.color_noise = 0.5;
            params.sharpening = 0.8;
            params.rotation = 3.5;
            params.lens.distortion = 0.3;
            params.lens.ca_red = 0.2;
            params.lens.vignetting = 0.3;
            params.defringe.purple_amount = 0.6;
            params.crop = [0.1, 0.05, 0.8, 0.85];

            let as_shot = crate::color::as_shot_kelvin_tint(&raw);
            let (kelvin, _t, wb) =
                crate::color::solve_wb(&params, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
            let dcp = raw.dcp_profile.as_ref().map(|d| {
                crate::raw::dcp::interpolate_at_temperature(d, kelvin, crate::core::tonemap::ToneSettings::from_params(&params))
            });
            let fm = dcp.as_ref().map(|i| i.camera_to_prophoto).unwrap_or_else(|| {
                crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65)
            });
            let build = |preview: Option<u32>| {
                let res = ImageResources::new(&ctx, 1, &raw.data, raw.width, raw.height, &params,
                    raw.wb_multipliers, fm, raw.cfa_pattern, raw.black_levels, raw.white_level,
                    dcp.as_ref(), preview, raw.orientation, raw.lens_context.clone()).unwrap();
                res.update_uniforms(&ctx, &params, dcp.as_ref(), wb);
                res
            };
            let full = build(None);
            let preview = build(Some(1280));
            let nr = DenoiseSettings::from_params(&params);

            for (center, label) in [([0.37, 0.62], "off-centre"), ([0.97, 0.03], "corner")] {
                let size = 256;
                // Reference: the full-resolution pipeline, windowed to the same view.
                let window = view_window(&full.current_params.lock().unwrap(), center, size, (raw.width, raw.height));
                let (reference, _, _) = render_to_bytes(&ctx, &full, size, size, window).await;

                for (main, name, tolerance) in [(&full, "full-res main", 2), (&preview, "preview main", 3)] {
                    let loupe = Loupe::new(&ctx, main, raw.clone()).unwrap();
                    assert_eq!(loupe.noise_model, full.noise_model, "noise model must be the sensor's");
                    let t = std::time::Instant::now();
                    let frame = loupe.render(&ctx, main, center, size, nr, None).await.unwrap();
                    let first_ms = t.elapsed().as_secs_f32() * 1000.0;
                    assert_eq!(frame.window, window);

                    let mut hist = [0usize; 256];
                    for (a, b) in reference.iter().zip(&frame.rgba) {
                        hist[(*a as i32 - *b as i32).unsigned_abs() as usize] += 1;
                    }
                    let max = hist.iter().rposition(|&n| n > 0).unwrap_or(0);
                    let n = reference.len();
                    let p999 = (0..256).find(|&d| hist[..=d].iter().sum::<usize>() * 1000 >= n * 999).unwrap();
                    eprintln!("{label}, {name}: max Δ {max}, 99.9% ≤ {p999}, first render {first_ms:.1} ms");
                    assert!(p999 <= tolerance, "{label}, {name}: loupe disagrees with the export (99.9% ≤ {p999})");

                    // A slider tick: new NR settings on the same crop.
                    let mut tick = nr;
                    tick.luma = 0.6;
                    let t = std::time::Instant::now();
                    loupe.render(&ctx, main, center, size, tick, None).await.unwrap();
                    eprintln!("    NR tick {:.1} ms", t.elapsed().as_secs_f32() * 1000.0);
                    let t = std::time::Instant::now();
                    loupe.render(&ctx, main, center, size, tick, None).await.unwrap();
                    eprintln!("    other-slider tick {:.1} ms", t.elapsed().as_secs_f32() * 1000.0);
                }
            }
        });
    }

    #[test]
    fn regions_keep_the_cfa_phase_and_stay_on_the_sensor() {
        let r = Region::around([101.3, 57.9, 300.2, 260.0], 10, SENSOR);
        assert_eq!((r.x % 2, r.y % 2), (0, 0));
        assert!(r.x <= 91 && r.y <= 47);
        assert!(r.x + r.w >= 311 && r.y + r.h >= 270);
        let edge = Region::around([-40.0, 3990.0, 30.0, 4100.0], 10, SENSOR);
        assert_eq!((edge.x, edge.y), (0, 3980));
        assert!(edge.x + edge.w <= SENSOR.0 && edge.y + edge.h <= SENSOR.1);
        // Wholly past the sensor: pinned to its edge, still non-empty.
        let off = Region::around([7000.0, 5000.0, 7100.0, 5100.0], 10, SENSOR);
        assert!(off.w >= 2 && off.h >= 2);
        assert!(off.x + off.w <= SENSOR.0 && off.y + off.h <= SENSOR.1);
        assert_eq!((off.x % 2, off.y % 2), (0, 0));
    }

    #[test]
    fn a_crop_is_reused_while_it_still_covers_the_window() {
        let grown = Region { x: 100, y: 100, w: 400, h: 400 };
        assert!(grown.contains(&Region { x: 120, y: 140, w: 300, h: 300 }));
        assert!(!grown.contains(&Region { x: 120, y: 140, w: 400, h: 300 }));
    }

    #[test]
    fn extract_copies_the_region_row_by_row() {
        let width = 8u32;
        let mosaic: Vec<u16> = (0..64).collect();
        let out = extract(&mosaic, width, Region { x: 2, y: 4, w: 3, h: 2 });
        assert_eq!(out, vec![34, 35, 36, 42, 43, 44]);
    }

    /// With no geometry edits, view UV is sensor UV, and the window is the
    /// requested number of sensor pixels wide.
    #[test]
    fn plain_geometry_maps_straight_through_at_one_to_one() {
        let gp = params();
        let uv = view_to_sensor_uv(&gp, [0.25, 0.75], SENSOR);
        assert!((uv[0] - 0.25).abs() < 1e-6 && (uv[1] - 0.75).abs() < 1e-6);
        let w = view_window(&gp, [0.5, 0.5], 256, SENSOR);
        assert!((w[2] * SENSOR.0 as f32 - 256.0).abs() < 0.01);
        assert!((w[3] * SENSOR.1 as f32 - 256.0).abs() < 0.01);
    }

    /// The window can't leave the image: pinned at a corner it starts at 0.
    #[test]
    fn the_window_stays_inside_the_image() {
        let w = view_window(&params(), [0.0, 1.0], 256, SENSOR);
        assert_eq!(w[0], 0.0);
        assert!((w[1] + w[3] - 1.0).abs() < 1e-6);
    }

    /// A crop narrows what one view-UV unit spans, so the 1:1 window grows in
    /// view UV, and view (0,0) lands on the crop's corner.
    #[test]
    fn crops_scale_the_window_and_offset_the_mapping() {
        let mut gp = params();
        gp.crop = [0.2, 0.1, 0.5, 0.5];
        let w = view_window(&gp, [0.5, 0.5], 256, SENSOR);
        assert!((w[2] * 0.5 * SENSOR.0 as f32 - 256.0).abs() < 0.01);
        let uv = view_to_sensor_uv(&gp, [0.0, 0.0], SENSOR);
        assert!((uv[0] - 0.2).abs() < 1e-6 && (uv[1] - 0.1).abs() < 1e-6);
    }

    /// Portrait shots: the display is the sensor turned; the mapping must
    /// agree with the eyedropper's (`color::display_to_sensor_uv`).
    #[test]
    fn orientation_matches_the_eyedropper_mapping() {
        for o in [3u32, 6, 8] {
            let mut gp = params();
            gp.orientation = o;
            let got = view_to_sensor_uv(&gp, [0.3, 0.8], SENSOR);
            let want = crate::color::display_to_sensor_uv(0.3, 0.8, o);
            assert!((got[0] - want.0).abs() < 1e-6 && (got[1] - want.1).abs() < 1e-6, "orientation {o}");
        }
    }

    /// Straightening rotates about the centre: the centre stays put, a point
    /// off-centre moves, and the footprint of a window grows to hold it.
    #[test]
    fn rotation_turns_about_the_centre() {
        let mut gp = params();
        gp.rotation = 10.0;
        let c = view_to_sensor_uv(&gp, [0.5, 0.5], SENSOR);
        assert!((c[0] - 0.5).abs() < 1e-6 && (c[1] - 0.5).abs() < 1e-6);
        let p = view_to_sensor_uv(&gp, [0.8, 0.5], SENSOR);
        assert!((p[1] - 0.5).abs() > 0.01);
        let window = view_window(&gp, [0.5, 0.5], 256, SENSOR);
        let b = footprint(&gp, window, SENSOR);
        assert!(b[2] - b[0] > 256.0 && b[3] - b[1] > 256.0);
    }
}
