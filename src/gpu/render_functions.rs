/// Phase 95: Standalone rendering functions that work with SharedContext + ImageResources
/// These replace the old RenderPipeline methods
use super::shared::{ImageResources, SharedContext};
use iced_wgpu::wgpu;

/// Patch just the `view_rect` field of the already-uploaded uniform block.
///
/// A targeted 16-byte write rather than re-uploading all 784 bytes, and — more
/// importantly — it lets two passes in the same frame see different windows:
/// `queue.write_buffer` is ordered against submissions on the same queue, so
/// write/submit/write/submit gives each pass its own value without a second
/// uniform buffer or dynamic offsets.
///
/// The offset is asserted in `gpu::params`'s tests; if `view_rect` ever moves,
/// this would silently corrupt whatever now sits there.
fn write_view_rect(context: &SharedContext, resources: &ImageResources, view_rect: [f32; 4]) {
    const VIEW_RECT_OFFSET: u64 =
        std::mem::offset_of!(super::params::GpuEditParams, view_rect) as u64;
    context.queue.write_buffer(
        &resources.uniform_buffer,
        VIEW_RECT_OFFSET,
        bytemuck::cast_slice(&view_rect),
    );
}

/// Render to bytes at specified resolution
/// Render to bytes at specified resolution (Async + Instrumented)
/// Render the color pass and read the result back to CPU memory.
///
/// Returns `Arc<[u8]>` rather than `Vec<u8>` because every caller immediately
/// needs shared ownership: the develop path hands the same buffer to the
/// viewport shader and the histogram task. Doing the conversion here keeps it
/// to one place — the caller used to build the `Arc` *and* retain the original
/// `Vec` inside an unused `image::Handle`, holding two full-size copies of a
/// buffer that can reach tens of megabytes at zoom.
/// A pending GPU→CPU transfer: the staging buffer plus what it takes to
/// un-pad it back into tightly-packed RGBA.
struct Readback {
    buffer: wgpu::Buffer,
    bytes_per_row: u32,
    width: u32,
    height: u32,
}

impl Readback {
    /// Copy out of the mapped range, dropping wgpu's 256-byte row padding.
    /// Only valid after the buffer has actually mapped.
    fn unpad(&self) -> Vec<u8> {
        let slice = self.buffer.slice(..);
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height {
            let start = (y * self.bytes_per_row) as usize;
            let end = start + (self.width * 4) as usize;
            out.extend_from_slice(&data[start..end]);
        }
        drop(data);
        self.buffer.unmap();
        out
    }
}

/// Record one color pass into its own texture and copy it to a staging buffer.
///
/// Returns the finished command buffer rather than submitting, so callers can
/// interleave uniform writes between passes: `write/submit/write/submit` gives
/// each pass a different `view_rect` off one uniform buffer.
fn encode_color_pass(
    context: &SharedContext,
    resources: &ImageResources,
    width: u32,
    height: u32,
) -> (wgpu::CommandBuffer, Readback) {
    let output_texture = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Output Texture"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });
    {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        render_pass.set_pipeline(&context.pipeline);
        render_pass.set_bind_group(0, &resources.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }

    let bytes_per_row = (width * 4 + 255) & !255;
    let readback_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Readback Buffer"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &output_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &readback_buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );

    (
        encoder.finish(),
        Readback { buffer: readback_buffer, bytes_per_row, width, height },
    )
}

pub async fn render_to_bytes(
    context: &SharedContext,
    resources: &ImageResources,
    width: u32,
    height: u32,
    view_rect: [f32; 4],
) -> (std::sync::Arc<[u8]>, f32, f32) {
    let t_upload_start = std::time::Instant::now();
    write_view_rect(context, resources, view_rect);
    let (cb, readback) = encode_color_pass(context, resources, width, height);
    let upload_ms = t_upload_start.elapsed().as_secs_f32() * 1000.0;

    let t_render_start = std::time::Instant::now();
    context.queue.submit(Some(cb));

    let (tx, rx) = tokio::sync::oneshot::channel();
    readback.buffer.slice(..).map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });

    // wgpu's map_async callback only fires when the device is polled.
    // block_in_place runs the blocking poll(Wait) on the current thread without
    // blocking the entire tokio executor — correct pattern for wgpu inside async.
    tokio::task::block_in_place(|| {
        context.device.poll(wgpu::Maintain::Wait);
    });

    if let Ok(Ok(())) = rx.await {
        let result = readback.unpad();
        let render_ms = t_render_start.elapsed().as_secs_f32() * 1000.0;
        // One copy: `Arc<[u8]>` needs a refcount header ahead of the data, so
        // it cannot adopt the Vec's allocation. Building it here rather than at
        // the call site at least keeps the total to a single conversion.
        (std::sync::Arc::from(result), upload_ms, render_ms)
    } else {
        (std::sync::Arc::from(Vec::new()), upload_ms, 0.0)
    }
}

/// Render the display window AND a small whole-image pass for the histogram,
/// under a single `poll(Wait)`.
///
/// The histogram must stay whole-image: the display pass only covers what is
/// on screen, so deriving the histogram from it would make it drift as the
/// user pans and zooms. A dedicated ~128px pass keeps the semantics and makes
/// the CPU scan trivial — it used to walk the entire display buffer, which at
/// 100% zoom meant ~24M pixels single-threaded per render.
///
/// Both passes read the same uniform buffer, so their differing `view_rect`
/// values are sequenced by `queue.write_buffer`'s ordering against submits.
pub async fn render_display_and_histogram(
    context: &SharedContext,
    resources: &ImageResources,
    width: u32,
    height: u32,
    view_rect: [f32; 4],
    hist_width: u32,
    hist_height: u32,
) -> (
    std::sync::Arc<[u8]>,
    crate::core::histogram::HistogramData,
    f32,
    f32,
) {
    // Timing is split so the profiler's buckets mean what they say:
    //   `encode_ms` — CPU-side resource creation and command recording
    //   `gpu_ms`    — every submit plus the readback wait
    // The two passes have to interleave (each uniform write applies to the
    // submit that follows it), so the submits can't simply be hoisted out of
    // the encode window — the elapsed time is accumulated instead.
    let t_encode = std::time::Instant::now();
    write_view_rect(context, resources, super::params::FULL_VIEW_RECT);
    let (hist_cb, hist_rb) = encode_color_pass(context, resources, hist_width, hist_height);
    let mut encode_ms = t_encode.elapsed().as_secs_f32() * 1000.0;

    // Counted as GPU time: `submit` can block on queue backpressure, and
    // attributing that stall to "upload" made the HUD read as though texture
    // allocation had suddenly become expensive after a zoom-in.
    let t_submit_a = std::time::Instant::now();
    context.queue.submit(Some(hist_cb));
    write_view_rect(context, resources, view_rect);
    let mut gpu_ms = t_submit_a.elapsed().as_secs_f32() * 1000.0;

    let t_encode_b = std::time::Instant::now();
    let (main_cb, main_rb) = encode_color_pass(context, resources, width, height);
    encode_ms += t_encode_b.elapsed().as_secs_f32() * 1000.0;
    let upload_ms = encode_ms;

    let t_render_start = std::time::Instant::now();
    context.queue.submit(Some(main_cb));

    let (htx, hrx) = tokio::sync::oneshot::channel();
    hist_rb.buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = htx.send(r);
    });
    let (mtx, mrx) = tokio::sync::oneshot::channel();
    main_rb.buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = mtx.send(r);
    });

    // One wait covers both submissions.
    tokio::task::block_in_place(|| {
        context.device.poll(wgpu::Maintain::Wait);
    });

    let histogram = if let Ok(Ok(())) = hrx.await {
        crate::core::histogram::calculate(&hist_rb.unpad())
    } else {
        [[0u32; 256]; 3]
    };

    if let Ok(Ok(())) = mrx.await {
        let result = main_rb.unpad();
        gpu_ms += t_render_start.elapsed().as_secs_f32() * 1000.0;
        (std::sync::Arc::from(result), histogram, upload_ms, gpu_ms)
    } else {
        (std::sync::Arc::from(Vec::new()), histogram, upload_ms, gpu_ms)
    }
}

/// Render to opaque RGB `u16` samples, for 16-bit TIFF export. Mirrors
/// `render_to_bytes` but targets `Rgba16Float` via `context.pipeline_16` —
/// a wgpu RenderPipeline's output format is baked in at creation time, so
/// this can't reuse the 8-bit pipeline/texture with a different format.
///
/// `Rgba16Float` (unlike `Rgba8Unorm`) doesn't clamp writes at the GPU's
/// fixed-function blend stage, so the conversion below clamps to [0,1]
/// before scaling to u16 — functionally the same clipping the 8-bit path
/// already applies in hardware, just performed here in software. 16-bit
/// buys smoother gradation in mid/shadow tones, not extra highlight
/// headroom. Alpha is dropped (always opaque for a flattened photo render),
/// matching the existing JPEG export path's RGB-only precedent.
pub async fn render_to_bytes_16bit(
    context: &SharedContext,
    resources: &ImageResources,
    width: u32,
    height: u32,
) -> (Vec<u16>, f32, f32) {
    let t_upload_start = std::time::Instant::now();

    // Export renders the whole cropped frame. This must be reset explicitly:
    // the uniform buffer is shared, so a display render at zoom leaves a
    // windowed `view_rect` behind that would otherwise silently export only
    // the region that happened to be on screen.
    write_view_rect(context, resources, super::params::FULL_VIEW_RECT);

    let output_texture = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Output Texture 16-bit"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let upload_ms = t_upload_start.elapsed().as_secs_f32() * 1000.0;
    let t_render_start = std::time::Instant::now();

    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder 16-bit"),
        });
    {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Render Pass 16-bit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        render_pass.set_pipeline(&context.pipeline_16);
        render_pass.set_bind_group(0, &resources.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }

    // 8 bytes/pixel (4 channels x f16) instead of 4 for the 8-bit path.
    let bytes_per_row = (width * 8 + 255) & !255;
    let buffer_size = (bytes_per_row * height) as u64;
    let readback_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Readback Buffer 16-bit"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &output_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &readback_buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    context.queue.submit(Some(encoder.finish()));

    let buffer_slice = readback_buffer.slice(..);
    let (tx, rx) = tokio::sync::oneshot::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });

    tokio::task::block_in_place(|| {
        context.device.poll(wgpu::Maintain::Wait);
    });

    if let Ok(Ok(())) = rx.await {
        let data = buffer_slice.get_mapped_range();
        // RGB only (alpha dropped) — 3 u16 samples per pixel.
        let mut result = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            let start = (y * bytes_per_row) as usize;
            let end = start + (width * 8) as usize;
            let row_f16: &[half::f16] = bytemuck::cast_slice(&data[start..end]);
            for pixel in row_f16.chunks_exact(4) {
                for &channel in &pixel[0..3] {
                    let v = (channel.to_f32().clamp(0.0, 1.0) * 65535.0).round() as u16;
                    result.push(v);
                }
            }
        }
        drop(data);
        readback_buffer.unmap();

        let render_ms = t_render_start.elapsed().as_secs_f32() * 1000.0;
        (result, upload_ms, render_ms)
    } else {
        (Vec::new(), upload_ms, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::EditParams;
    use crate::gpu::params::FULL_VIEW_RECT;
    use crate::gpu::shared::ImageResources;

    /// A frame with real 2-channel highlight clipping (measured: ~2900 blown
    /// Bayer blocks, 97.5% of them still carrying usable data). Override with
    /// `RAWEDITOR_TEST_RAW` to point at your own file.
    fn test_raw_path() -> String {
        std::env::var("RAWEDITOR_TEST_RAW").unwrap_or_else(|_| {
            "/home/hsf001/Pictures/Photography/TESTFiles/RAW/_DSC0177.NEF".to_string()
        })
    }

    /// Coefficient of variation (stddev / mean) of luma over `mask`.
    ///
    /// The metric that distinguishes real highlight recovery from merely
    /// darkening: a uniform multiply leaves CV unchanged, because both stddev
    /// and mean scale together. CV only rises if gradation that was compressed
    /// into a flat band gets redistributed — i.e. if detail actually returns.
    fn luma_cv(rgba: &[u8], mask: &[bool]) -> f64 {
        let vals: Vec<f64> = rgba
            .chunks_exact(4)
            .zip(mask)
            .filter(|(_, m)| **m)
            .map(|(p, _)| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64)
            .collect();
        if vals.is_empty() {
            return 0.0;
        }
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        if mean <= 0.0 {
            return 0.0;
        }
        let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / vals.len() as f64;
        (var.sqrt()) / mean
    }

    fn mean_luma(rgba: &[u8], mask: &[bool]) -> f64 {
        let vals: Vec<f64> = rgba
            .chunks_exact(4)
            .zip(mask)
            .filter(|(_, m)| **m)
            .map(|(p, _)| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64)
            .collect();
        if vals.is_empty() { return 0.0; }
        vals.iter().sum::<f64>() / vals.len() as f64
    }

    /// Render one frame at the given highlights setting.
    async fn render_at(
        ctx: &SharedContext,
        res: &ImageResources,
        raw: &crate::raw::loader::RawDataResult,
        highlights: f32,
        w: u32,
        h: u32,
    ) -> std::sync::Arc<[u8]> {
        let mut params = EditParams::default();
        params.highlights = highlights;
        // Mirror the develop path's WB/DCP resolution so the render matches
        // what the app actually produces.
        let as_shot = crate::color::as_shot_kelvin_tint(raw);
        let (kelvin, _tint, wb_override) =
            crate::color::solve_wb(&params, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
        let interpolated = raw.dcp_profile.as_ref().map(|d| {
            crate::raw::dcp::interpolate_at_temperature(d, kelvin, params.profile_curve)
        });
        res.update_uniforms(ctx, &params, interpolated.as_ref(), wb_override);
        let (bytes, _, _) = render_to_bytes(ctx, res, w, h, FULL_VIEW_RECT).await;
        bytes
    }

    /// Highlight recovery must return DETAIL, not just darken.
    ///
    /// Ignored by default: needs a GPU and the test RAW on disk.
    /// Run with `cargo test --release highlight_recovery -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn highlight_recovery_restores_detail_not_just_brightness() {
        let test_raw = test_raw_path();
        if !std::path::Path::new(&test_raw).exists() {
            eprintln!("test RAW missing, skipping");
            return;
        }
        // Multi-thread flavour is required: render_to_bytes uses
        // tokio::task::block_in_place for the wgpu poll.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .expect("runtime");

        rt.block_on(async {
            let ctx = SharedContext::new().await.expect("gpu context");
            let raw = crate::raw::loader::load_raw_data(test_raw.clone())
                .await
                .expect("decode raw");

            let base = EditParams::default();
            let as_shot = crate::color::as_shot_kelvin_tint(&raw);
            let (kelvin, _t, _wb) =
                crate::color::solve_wb(&base, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
            let (forward_matrix, dcp) = match &raw.dcp_profile {
                Some(d) => {
                    let i = crate::raw::dcp::interpolate_at_temperature(d, kelvin, base.profile_curve);
                    (i.forward_matrix, Some(i))
                }
                None => (
                    crate::color::calculate_cam_to_srgb(
                        raw.color_matrix,
                        raw.wb_multipliers,
                        raw.color_matrix_is_d65,
                    ),
                    None,
                ),
            };
            assert!(dcp.is_some(), "this assertion targets the DCP path; no profile found");

            let res = ImageResources::new(
                &ctx, 1, &raw.data, raw.width, raw.height, &base,
                raw.wb_multipliers, forward_matrix, raw.cfa_pattern,
                raw.black_levels, raw.white_level, dcp.as_ref(),
                // Full resolution, not the 1280px preview: the develop
                // preview's box-filter downsample averages isolated clipped
                // pixels away, leaving too few to measure.
                None, raw.orientation,
            )
            .expect("image resources");

            let (w, h) = res.oriented_dims();

            let flat = render_at(&ctx, &res, &raw, 0.0, w, h).await;
            let recovered = render_at(&ctx, &res, &raw, -1.0, w, h).await;
            assert_eq!(flat.len(), recovered.len());

            // The blown region: near-white at highlights 0. This is where the
            // complaint lives — it renders as a flat grey when pulled down.
            let mask: Vec<bool> = flat
                .chunks_exact(4)
                .map(|p| {
                    let l = 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
                    l > 245.0
                })
                .collect();
            let blown = mask.iter().filter(|m| **m).count();
            eprintln!("blown pixels sampled: {blown}");
            assert!(blown > 500, "not enough blown area to judge ({blown} px)");

            let cv_flat = luma_cv(&flat, &mask);
            let cv_rec = luma_cv(&recovered, &mask);
            let mean_flat = mean_luma(&flat, &mask);
            let mean_rec = mean_luma(&recovered, &mask);

            eprintln!("highlights  0: mean {:.1}  CV {:.5}", mean_flat, cv_flat);
            eprintln!("highlights -1: mean {:.1}  CV {:.5}", mean_rec, cv_rec);
            eprintln!("CV ratio: {:.2}x", cv_rec / cv_flat.max(1e-9));

            // Recovery must darken the region...
            assert!(mean_rec < mean_flat, "recovery did not darken the highlights");
            // ...and, crucially, must add gradation rather than translate a
            // flat band. A pure uniform scale leaves CV unchanged.
            assert!(
                cv_rec > cv_flat * 1.5,
                "highlights only darkened without restoring detail: CV {:.5} -> {:.5}",
                cv_flat,
                cv_rec
            );
        });
    }

    /// The calibration guarantee: with Highlights and Shadows at zero, moving
    /// them to scene-linear space must change NOTHING. Both blocks are guarded
    /// by `!= 0.0`, so this holds by construction — but it is the property that
    /// protects the hand-tuned `profile_curve = 0.33` match against camera
    /// JPEGs, so it is worth asserting rather than assuming.
    ///
    /// Renders twice at defaults and compares byte-for-byte; also serves as a
    /// determinism check on the render path.
    #[test]
    #[ignore]
    fn neutral_settings_render_is_stable() {
        let test_raw = test_raw_path();
        if !std::path::Path::new(&test_raw).exists() {
            eprintln!("test RAW missing, skipping");
            return;
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .expect("runtime");

        rt.block_on(async {
            let ctx = SharedContext::new().await.expect("gpu context");
            let raw = crate::raw::loader::load_raw_data(test_raw.clone())
                .await
                .expect("decode raw");
            let base = EditParams::default();
            assert_eq!(base.highlights, 0.0, "test assumes neutral defaults");
            assert_eq!(base.shadows, 0.0, "test assumes neutral defaults");

            let as_shot = crate::color::as_shot_kelvin_tint(&raw);
            let (kelvin, _t, _wb) =
                crate::color::solve_wb(&base, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
            let dcp = raw.dcp_profile.as_ref().map(|d| {
                crate::raw::dcp::interpolate_at_temperature(d, kelvin, base.profile_curve)
            });
            let forward_matrix = dcp.as_ref().map(|i| i.forward_matrix).unwrap_or_else(|| {
                crate::color::calculate_cam_to_srgb(
                    raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65,
                )
            });

            let res = ImageResources::new(
                &ctx, 1, &raw.data, raw.width, raw.height, &base,
                raw.wb_multipliers, forward_matrix, raw.cfa_pattern,
                raw.black_levels, raw.white_level, dcp.as_ref(),
                Some(1280), raw.orientation,
            )
            .expect("image resources");
            let (w, h) = res.oriented_dims();

            let a = render_at(&ctx, &res, &raw, 0.0, w, h).await;
            let b = render_at(&ctx, &res, &raw, 0.0, w, h).await;
            assert_eq!(a.as_ref(), b.as_ref(), "neutral render is not deterministic");
            eprintln!("neutral render stable over {} bytes", a.len());
        });
    }

}
