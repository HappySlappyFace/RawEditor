/// Phase 95: Shared GPU Context and Per-Image Resources
///
/// This module contains the refactored pipeline architecture:
/// - SharedContext: Persistent GPU resources (created once)
/// - ImageResources: Per-image data (created for each image)
use iced_wgpu::wgpu;
use std::sync::Arc;
use wgpu::util::DeviceExt;

use super::params::GpuEditParams;
use crate::core::tonemap::{ToneSettings, TONE_LUT_SIZE};
use crate::core::types::EditParams;
use crate::gpu::shaders;

/// Shared GPU context (created once, reused for all images)
/// Contains all the persistent GPU resources that don't change between images
#[derive(Debug)]
pub struct SharedContext {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    /// Phase 128: Color science pipeline (Pass 2 — reads Rgba16Float)
    pub pipeline: wgpu::RenderPipeline,
    /// Same Pass-2 shader/layout as `pipeline`, but with an Rgba16Float
    /// output target instead of Rgba8Unorm — used only for 16-bit TIFF
    /// export. A wgpu RenderPipeline's output format is baked in at
    /// creation time, so a second pipeline is required rather than
    /// swapping the render target's format on the existing one.
    pub pipeline_16: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Colour pass group 1: brush layers + mask uniforms.
    pub mask_bind_group_layout: wgpu::BindGroupLayout,
    /// Colour pass group 2: local-contrast bands + dehaze coefficients.
    pub presence_bind_group_layout: wgpu::BindGroupLayout,
    /// Rasterises brush strokes into mask layers (see `gpu::brush`).
    pub brush: super::brush::BrushRaster,
    /// Pass 1: RCD demosaic (reads R16Uint, writes Rgba16Float).
    pub demosaic: super::demosaic::Demosaic,
    /// Cached noise-reduction pre-pass (see `gpu::denoise`).
    pub denoise: super::denoise::Denoise,
    /// Cached Texture/Clarity band pre-pass (see `gpu::local_contrast`).
    pub local_contrast: super::local_contrast::LocalContrast,
    pub sampler_linear: wgpu::Sampler,
}

impl SharedContext {
    /// Create a new shared GPU context
    /// This should be called once on app startup
    pub async fn new() -> Result<Self, String> {
        // Request wgpu adapter
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or("Failed to find suitable GPU adapter")?;

        // Request device and queue.
        //
        // The size limits come from the adapter, not `Limits::default()`. The
        // defaults are WebGPU's portable floor — 8192px textures, 256 MB
        // buffers — and a full-resolution pass of any sensor wider than 8192px
        // (45 MP and up), or the 16-bit readback of a 45 MP TIFF export (~360
        // MB), failed validation outright. Every desktop GPU supports far more.
        let adapter_limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_texture_dimension_1d: adapter_limits.max_texture_dimension_1d,
            max_texture_dimension_2d: adapter_limits.max_texture_dimension_2d,
            max_buffer_size: adapter_limits.max_buffer_size,
            ..wgpu::Limits::default()
        };
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("RAW Editor Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits,
                },
                None,
            )
            .await
            .map_err(|e| format!("Failed to create device: {:?}", e))?;

        // wgpu's default handler panics on any validation error, which takes
        // the whole app — and any unflushed edits — down with it. Sizes are
        // checked up front (`check_texture_size`), so anything that still gets
        // here is a bug to log, not a reason to crash a photo editor.
        device.on_uncaptured_error(Box::new(|e| {
            tracing::error!("Uncaptured GPU error: {e}");
        }));

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        // Phase 128: Load both shaders
        let color_shader_source = shaders::get_color_shader();
        let color_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Color Shader (Pass 2)"),
            source: wgpu::ShaderSource::Wgsl(color_shader_source.into()),
        });

        // Phase 128: Color bind group layout (reads Rgba16Float = Float texture)
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Color Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Phase 140: 3D LUT (HueSatMap)
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                // Phase 140: 1D Texture (ToneCurve)
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D1,
                        multisampled: false,
                    },
                    count: None,
                },
                // Phase 140: LUT Sampler (Clamp to edge)
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // Point-curve LUT (core::curves)
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D1,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let mask_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mask Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let presence_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Presence Bind Group Layout"),
            entries: &[
                // Local-contrast bands (Rgba16Float, sampled bilinearly).
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // Dehaze coefficients (Rg32Float: f16 cannot hold a and b,
                // which nearly cancel; read with textureLoad).
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // Clarity guided-filter coefficients (coarse, sampled
                // bilinearly — the fast guided filter's upsampling).
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let brush = super::brush::BrushRaster::new(&device);
        let demosaic = super::demosaic::Demosaic::new(&device);
        let denoise = super::denoise::Denoise::new(&device);
        let local_contrast = super::local_contrast::LocalContrast::new(&device);

        // Color pipeline (Pass 2 → Rgba8Unorm canvas)
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Color Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout, &mask_bind_group_layout, &presence_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Color Render Pipeline (Pass 2)"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &color_shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &color_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        // Same pipeline as above, targeting Rgba16Float instead of
        // Rgba8Unorm — used only for 16-bit TIFF export.
        let pipeline_16 = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Color Render Pipeline 16-bit (Export)"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &color_shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &color_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        // Linear sampler (Filtering — for Pass 2 smooth preview downscaling)
        let sampler_linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("RAW Linear Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        tracing::info!("SharedContext initialized successfully");

        Ok(Self {
            device,
            queue,
            pipeline,
            pipeline_16,
            bind_group_layout,
            mask_bind_group_layout,
            presence_bind_group_layout,
            brush,
            demosaic,
            denoise,
            local_contrast,
            sampler_linear,
        })
    }
}

impl SharedContext {
    /// Refuse a texture this device cannot allocate, with a message a user
    /// can act on, instead of letting wgpu fail validation mid-render.
    pub fn check_texture_size(&self, width: u32, height: u32) -> Result<(), String> {
        let max = self.device.limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(format!(
                "{width}×{height} exceeds this GPU's {max}px texture limit"
            ));
        }
        Ok(())
    }

    /// Refuse a readback buffer larger than the device allows — see
    /// `check_texture_size`.
    pub fn check_buffer_size(&self, bytes: u64) -> Result<(), String> {
        let max = self.device.limits().max_buffer_size;
        if bytes > max {
            return Err(format!(
                "a {} MB render exceeds this GPU's {} MB buffer limit",
                bytes >> 20,
                max >> 20
            ));
        }
        Ok(())
    }

    /// Let wgpu actually reclaim textures and buffers whose Rust handles have
    /// already been dropped.
    ///
    /// Dropping a `Texture` or `Buffer` only queues it into the device's life
    /// tracker; the allocation survives until the next `poll`/`maintain`. Every
    /// `poll` in this crate sits *inside* a render function, before that
    /// function's own resources go out of scope — so during a batch export each
    /// image's ~336 MB of textures and readback buffers was carried until the
    /// NEXT image polled, and the final image's was never released at all.
    ///
    /// `Maintain::Poll` is non-blocking, so this is safe to call from the
    /// update thread; it processes what is already finished rather than waiting
    /// on the GPU.
    pub fn reclaim(&self) {
        self.device.poll(wgpu::Maintain::Poll);
    }
}

/// Per-image GPU resources (created for each image)
/// Contains all the image-specific data that changes when switching images
#[derive(Debug)]
pub struct ImageResources {
    // Handles that exist to own their GPU allocations: the bind groups are
    // what the passes actually read. Never read directly, hence the allows.
    #[allow(dead_code)]
    pub texture: wgpu::Texture,
    pub texture_view: wgpu::TextureView,
    /// Demosaiced camera-native RGB (Rgba16Float, working resolution) — the
    /// output of the RCD passes, written once at load.
    #[allow(dead_code)] // owner — see `texture`
    pub debayer_texture: wgpu::Texture,
    pub debayer_texture_view: wgpu::TextureView,
    /// Colour-pass bind group reading the demosaic directly — used whenever
    /// no pre-pass stage is active.
    base_bind_group: Arc<wgpu::BindGroup>,
    /// Cached pre-pass state (denoise); see `prepare_stages`.
    stages: std::sync::Mutex<StageState>,
    /// The params most recently pushed — what `prepare_stages` brings the
    /// cached stages up to date with before a render.
    stage_params: std::sync::Mutex<EditParams>,
    /// `GpuMasks` uniform (group 1, binding 1).
    mask_buffer: wgpu::Buffer,
    /// This shot's optics (matched profile, focal length, crop factor).
    pub lens: Arc<crate::core::lens::LensContext>,
    /// Baked point curves (binding 6); refreshed by `prepare_stages`.
    curve_texture: wgpu::Texture,
    curve_view: wgpu::TextureView,
    /// This image's sensor noise model, estimated at load from the mosaic as
    /// uploaded (so a box-filtered preview gets its own, lower, noise).
    pub noise_model: crate::core::noise::NoiseModel,
    /// Dehaze airlight (analysis units, see `core::haze`), `None` when the
    /// image was too small to analyse. Its coefficient map is `haze_view`.
    haze_airlight: Option<[f32; 3]>,
    /// The as-shot WB the demosaic, noise model and haze analysis work in
    /// (green-normalised, clamped — `GpuDemosaicParams::ceil`).
    analysis_wb: [f32; 3],
    haze_view: wgpu::TextureView,
    /// Highlight-reconstruction correction (`core::highlights`), camera-native.
    highlight_correction: [f32; 3],
    pub uniform_buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    pub original_width: u32,
    pub original_height: u32,
    pub image_id: i64,
    // Metadata
    /// As-shot WB multipliers (from the raw file — never changes).
    pub wb_multipliers: [f32; 4],
    /// Currently active WB multipliers (as-shot, or recomputed from the
    /// user's Temp/Tint sliders). Read by every uniform upload so a zoom or
    /// resize render doesn't silently revert the slider WB.
    pub wb_current: std::sync::RwLock<[f32; 4]>,
    pub forward_matrix: std::sync::RwLock<[f32; 9]>,
    pub cfa_pattern: u32,
    pub black_levels: [u32; 4],
    /// Sensor-correction offsets the mosaic was demosaiced with.
    pub black_offsets: [f32; 4],
    pub white_level: u32,
    pub current_params: std::sync::Mutex<GpuEditParams>,
    // Phase 140: DCP Textures
    pub hsv_lut_texture: wgpu::Texture,
    #[allow(dead_code)] // owner — see `texture`
    pub hsv_lut_view: wgpu::TextureView,
    pub tone_curve_texture: wgpu::Texture,
    #[allow(dead_code)] // owner — see `texture`
    pub tone_curve_view: wgpu::TextureView,
    pub has_dcp: bool,
    /// True when the DCP embeds a ProfileToneCurve. Only consulted while the
    /// `Camera` tone operator is active; every other operator supplies its own
    /// curve regardless of what the profile carries.
    pub dcp_has_curve: bool,
    /// The camera profile these resources were built with, and what chose
    /// it. `None` for paths that don't record it (tests); set by the app's
    /// loaders before the resources are shared.
    pub profile_use: Option<crate::core::camera_profiles::ProfileUse>,
    /// Normalised EXIF orientation (1/3/6/8). Sensor textures stay landscape;
    /// the color pass maps display UVs to sensor space, and render targets are
    /// sized to `oriented_*` dims. The edit's quarter turns compose on top
    /// (`display_orientation`) — what the shader receives is never this
    /// field alone.
    pub orientation: u32,
    /// Temperature slider value at the last HueSatMap 3D LUT upload.
    ///
    /// Kept SEPARATE from the tone key below because the two textures have
    /// genuinely different dependencies: the HSM is re-interpolated only by
    /// temperature, while the ~8 KB tone curve also moves with the tone
    /// operator and its parameters. Sharing one key meant dragging a
    /// tone-shape slider re-uploaded the ~90 KB 3D LUT every frame for no
    /// reason.
    pub last_uploaded_hsm_temp: std::sync::RwLock<Option<f32>>,
    /// Tone settings at the last curve-LUT upload. Skips redundant uploads on
    /// every unrelated slider tick.
    pub last_uploaded_tone: std::sync::RwLock<Option<crate::core::tonemap::ToneSettings>>,
}

/// Upload a baked tone curve into the 1D curve texture.
///
/// Shared by construction and by `update_uniforms` so the two cannot disagree
/// about format or row pitch. Samples are display-linear and indexed by
/// `tonemap::tone_lut_encode` — see `core::tonemap`.
fn write_tone_curve(context: &SharedContext, texture: &wgpu::Texture, curve: &[f32]) {
    // Pad or truncate to the texture's exact width: a short table would leave
    // stale samples at the top of the curve, which reads as a highlight shift.
    let mut samples: Vec<half::f16> = curve
        .iter()
        .take(TONE_LUT_SIZE)
        .map(|&v| half::f16::from_f32(v))
        .collect();
    let last = samples.last().copied().unwrap_or(half::f16::ZERO);
    samples.resize(TONE_LUT_SIZE, last);

    context.queue.write_texture(
        wgpu::ImageCopyTexture {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&samples),
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some((TONE_LUT_SIZE * 2) as u32),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: TONE_LUT_SIZE as u32,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
}

/// Compute stride for Bayer subsampling.
///
/// **Stride MUST be odd** to preserve CFA alignment: with an odd stride N,
/// output column `px` samples source column `px*N`, and since N is odd,
/// `(px*N) % 2 == px % 2` — the even/odd parity alternates correctly.
/// Even strides map every output pixel to an even source column → all same
/// Bayer channel → wrong colors (e.g., all magenta from all-Red samples).
///
/// Returns the largest odd stride such that `full_width / stride >= target_width`,
/// or 1 (no subsampling) if the image is already close to the target size.
fn bayer_stride(full_width: u32, target_width: u32) -> u32 {
    if full_width <= target_width * 2 {
        return 1;
    }
    let raw = full_width / target_width; // floor
    // Round down to nearest odd number (odd strides preserve Bayer parity)
    let odd = if raw % 2 == 1 { raw } else { raw - 1 };
    odd.max(1)
}

/// Downsample a Bayer mosaic by `stride` using a per-CFA-plane box filter.
///
/// Each output site averages ALL same-color Bayer sites inside its stride×stride
/// source block (even offsets keep the color plane; odd stride keeps the output
/// mosaic's CFA parity — see `bayer_stride`). Point-sampling one pixel per block
/// (the old behaviour) aliases fine detail and passes sensor noise through at
/// full strength, which is why previews looked noisier/crunchier than Lightroom.
///
/// Iterates over exactly new_width × new_height output pixels — NOT via step_by
/// on the full range. step_by overshoots when the dimension is not a multiple of
/// stride (e.g. 6016 / 3 = 2005 but step_by(3) on 0..6016 emits 2006 values),
/// causing every subsequent row to be offset by 1 pixel in the upload buffer →
/// diagonal split in the image.
fn downsample_bayer(data: &[u16], full_width: u32, full_height: u32, stride: u32) -> (Vec<u16>, u32, u32) {
    let new_width = full_width / stride;
    let new_height = full_height / stride;
    let mut out = Vec::with_capacity((new_width * new_height) as usize);
    let fw = full_width as usize;
    let s = stride as usize;

    if s == 1 {
        out.extend_from_slice(&data[..fw * new_height as usize]);
        return (out, new_width, new_height);
    }

    // Same-color sites inside the block sit at even offsets: 0, 2, 4, … < s.
    // (Block never leaves the image: anchor ≤ (new_dim-1)·s and new_dim·s ≤ full_dim.)
    let taps_per_axis = s.div_ceil(2);
    let tap_count = (taps_per_axis * taps_per_axis) as u32;

    for row in 0..new_height as usize {
        let sy = row * s;
        for col in 0..new_width as usize {
            let sx = col * s;
            let mut acc: u32 = 0;
            let mut dy = 0;
            while dy < s {
                let line = (sy + dy) * fw + sx;
                let mut dx = 0;
                while dx < s {
                    acc += data[line + dx] as u32;
                    dx += 2;
                }
                dy += 2;
            }
            out.push((acc / tap_count) as u16);
        }
    }
    (out, new_width, new_height)
}

/// Build the padded RGBA16F upload buffer for the DCP HueSatMap 3D LUT.
///
/// DNG spec entry order is value-outermost, then hue, then saturation-innermost:
/// `i = (v * hueDivs + h) * satDivs + s`. (The previous code iterated hue as the
/// fastest axis, transposing hue/sat and scrambling every non-square LUT.)
///
/// The texture's X axis carries hue with ONE EXTRA duplicated row (`hueDivs + 1`
/// texels): hue is cyclic over 360°, and the clamp-addressing sampler would
/// otherwise fail to interpolate across the red seam at h≈0/h≈1. The shader maps
/// hue as `(h * hueDivs + 0.5) / (hueDivs + 1)`.
///
/// Returns `(padded_data, texture_extent, padded_bytes_per_row)`.
fn build_hsm_lut_upload(dcp: &crate::raw::dcp::InterpolatedProfile) -> (Vec<u8>, wgpu::Extent3d, u32) {
    let (hue_divs, sat_divs, val_divs) = dcp.hue_sat_dims;
    let tex_w = hue_divs + 1; // wrap row
    let unpadded_bytes_per_row = tex_w * 8;
    let padded_bytes_per_row = (unpadded_bytes_per_row + 255) & !255;
    let mut data = Vec::with_capacity((padded_bytes_per_row * sat_divs * val_divs) as usize);

    let neutral = [
        half::f16::from_f32(0.0),
        half::f16::from_f32(1.0),
        half::f16::from_f32(1.0),
        half::f16::from_f32(1.0),
    ];

    for v in 0..val_divs {
        for s in 0..sat_divs {
            for x in 0..tex_w {
                let h = x % hue_divs; // last texel duplicates hue 0
                let i = ((v * hue_divs + h) * sat_divs + s) as usize;
                if let Some(e) = dcp.hue_sat_lut.get(i) {
                    let c = [
                        half::f16::from_f32(e[0]),
                        half::f16::from_f32(e[1]),
                        half::f16::from_f32(e[2]),
                        half::f16::from_f32(1.0),
                    ];
                    data.extend_from_slice(bytemuck::cast_slice(&c));
                } else {
                    data.extend_from_slice(bytemuck::cast_slice(&neutral));
                }
            }
            data.extend(std::iter::repeat_n(0u8, (padded_bytes_per_row - unpadded_bytes_per_row) as usize));
        }
    }

    let extent = wgpu::Extent3d {
        width: tex_w,
        height: sat_divs,
        depth_or_array_layers: val_divs,
    };
    (data, extent, padded_bytes_per_row)
}

/// Bind group for the colour pass reading `source` — the demosaic, or a
/// pre-pass stage's output. Everything else in it is per-image and fixed.
fn make_color_bind_group(
    context: &SharedContext,
    source: &wgpu::TextureView,
    uniform_buffer: &wgpu::Buffer,
    hsv_lut_view: &wgpu::TextureView,
    tone_curve_view: &wgpu::TextureView,
    curve_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    context.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Color Bind Group"),
        layout: &context.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&context.sampler_linear),
            },
            wgpu::BindGroupEntry { binding: 2, resource: uniform_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(hsv_lut_view) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(tone_curve_view) },
            wgpu::BindGroupEntry {
                binding: 5,
                // Clamp-to-edge is the default addressing.
                resource: wgpu::BindingResource::Sampler(&context.sampler_linear),
            },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(curve_view) },
        ],
    })
}

/// Cached pre-pass stages and which texture the colour pass should read.
#[derive(Debug)]
struct StageState {
    /// Settings the denoise output was last produced with; `None` when it
    /// has never run. Kept while NR is off (`denoise_on`), so turning it back
    /// on with the same settings — releasing the before/after peek, which
    /// renders with NR off — reuses the output instead of re-running it.
    denoise_key: Option<super::denoise::DenoiseSettings>,
    /// Whether the colour pass reads the denoise output (NR active) or the
    /// demosaic.
    denoise_on: bool,
    /// Allocated on first use and kept: re-allocating ~2 full-size textures
    /// per slider tick would churn the allocator for nothing.
    denoise: Option<(super::denoise::DenoiseTargets, [Arc<wgpu::BindGroup>; 2])>,
    /// The colour bind group renders should use right now.
    active: Arc<wgpu::BindGroup>,
    /// Curves the LUT texture currently holds; `None` = never uploaded.
    curves: Option<crate::core::curves::ToneCurves>,
    /// Strokes the brush layers were last rasterised from.
    brush_key: Option<crate::core::brush::BrushData>,
    /// Allocated on the first stroke.
    brush: Option<super::brush::BrushLayers>,
    /// Group 1 for the colour pass: brush layers (or the placeholder) + masks.
    masks: Arc<wgpu::BindGroup>,
    /// Which denoise target holds the current output (when `denoise_key`).
    denoise_out: usize,
    /// The source the local-contrast bands were last built from, keyed by
    /// the denoise settings that produced it (`None` = the demosaic);
    /// `None` outside = never built.
    local_key: Option<Option<super::denoise::DenoiseSettings>>,
    /// Allocated the first time Texture or Clarity is used.
    local: Option<super::local_contrast::LocalTargets>,
    /// Group 2 for the colour pass: bands and Clarity coefficients (or
    /// placeholders) + the dehaze map.
    presence: Arc<wgpu::BindGroup>,
}

/// The bind groups a colour pass needs.
pub struct ColorBindings {
    pub main: Arc<wgpu::BindGroup>,
    pub masks: Arc<wgpu::BindGroup>,
    pub presence: Arc<wgpu::BindGroup>,
}

fn make_mask_bind_group(
    context: &SharedContext,
    layers: &wgpu::TextureView,
    mask_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    context.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Mask Bind Group"),
        layout: &context.mask_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(layers) },
            wgpu::BindGroupEntry { binding: 1, resource: mask_buffer.as_entire_binding() },
        ],
    })
}

fn make_presence_bind_group(
    context: &SharedContext,
    bands: &wgpu::TextureView,
    haze: &wgpu::TextureView,
    clarity: &wgpu::TextureView,
) -> wgpu::BindGroup {
    context.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Presence Bind Group"),
        layout: &context.presence_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(bands) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(haze) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(clarity) },
        ],
    })
}

/// Upload a haze analysis as the colour pass's coefficient texture.
fn haze_texture(context: &SharedContext, haze: &crate::core::haze::HazeModel) -> wgpu::TextureView {
    let size = wgpu::Extent3d { width: haze.width as u32, height: haze.height as u32, depth_or_array_layers: 1 };
    let texture = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Dehaze coefficients"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    context.queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(&haze.coeffs),
        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(size.width * 8), rows_per_image: None },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

impl StageState {
    fn new(base: Arc<wgpu::BindGroup>, masks: Arc<wgpu::BindGroup>, presence: Arc<wgpu::BindGroup>) -> Self {
        Self {
            denoise_key: None,
            denoise_on: false,
            denoise: None,
            active: base,
            curves: None,
            brush_key: None,
            brush: None,
            masks,
            denoise_out: 0,
            local_key: None,
            local: None,
            presence,
        }
    }
}

impl ImageResources {
    /// Create new image resources for a specific image
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: &SharedContext,
        image_id: i64,
        raw_data: &[u16],
        width: u32,
        height: u32,
        params: &EditParams,
        wb_multipliers: [f32; 4],
        forward_matrix: [f32; 9],
        cfa_pattern: u32,
        black_levels: [u32; 4],
        white_level: u32,
        dcp_profile: Option<&crate::raw::dcp::InterpolatedProfile>,
        max_preview_width: Option<u32>,
        orientation: u32,
        lens: Arc<crate::core::lens::LensContext>,
    ) -> Result<Self, String> {
        let max_w = max_preview_width.unwrap_or(u32::MAX);

        // Stride-sample the Bayer data to a ~max_w working size.
        // Pass max_preview_width=None to skip subsampling (used for export / full-res zoom).
        let original_width = width;
        let original_height = height;
        let stride = if max_w < width { bayer_stride(width, max_w) } else { 1 };
        let (owned_downsampled, width, height) = if stride > 1 {
            let (data, w, h) = downsample_bayer(raw_data, width, height, stride);
            tracing::info!(
                "Bayer subsampled {}×{} → {}×{} (stride {})",
                original_width, original_height, w, h, stride
            );
            (Some(data), w, h)
        } else {
            (None, width, height)
        };
        let raw_data: &[u16] = owned_downsampled.as_deref().unwrap_or(raw_data);

        context.check_texture_size(width, height)?;
        if raw_data.len() < (width as usize) * (height as usize) {
            return Err(format!(
                "RAW buffer holds {} samples, {width}×{height} needs {}",
                raw_data.len(),
                width as usize * height as usize
            ));
        }

        tracing::debug!("Texture {}x{}", width, height);

        // Create texture for RAW data
        let texture_size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let texture = context.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("RAW Input Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload the mosaic as-is. `Queue::write_texture` has no 256-byte
        // row-alignment rule (that applies only to buffer↔texture copies), so
        // the padded staging copy built here before — another ~50 MB per
        // full-resolution load or export at 24 MP — was never needed.
        let unpadded_bytes_per_row = width * 2;
        let pixel_count = (width as usize) * (height as usize);
        context.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&raw_data[..pixel_count]),
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(unpadded_bytes_per_row),
                rows_per_image: Some(height),
            },
            texture_size,
        );

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Phase 128: Create the intermediate debayered texture (Rgba16Float)
        let debayer_texture = context.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Debayered Intermediate Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let debayer_texture_view = debayer_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut gpu_params = GpuEditParams::from(params);
        gpu_params.wb_multipliers = wb_multipliers;
        gpu_params.forward_matrix_0 = [forward_matrix[0], forward_matrix[1], forward_matrix[2]];
        gpu_params.forward_matrix_1 = [forward_matrix[3], forward_matrix[4], forward_matrix[5]];
        gpu_params.forward_matrix_2 = [forward_matrix[6], forward_matrix[7], forward_matrix[8]];
        gpu_params.cfa_pattern = cfa_pattern;
        gpu_params.black_levels = black_levels;
        gpu_params.white_level = white_level;
        gpu_params.has_dcp = if dcp_profile.is_some() { 1 } else { 0 };
        gpu_params.dcp_has_curve = if dcp_profile.is_some_and(|p| p.has_tone_curve) { 1 } else { 0 };
        gpu_params.orientation = crate::color::compose_orientation(orientation, params.quarter_turns);
        gpu_params.set_lens(&crate::core::lens::gpu_params(
            &lens,
            &params.lens,
            original_width,
            original_height,
        ));
        gpu_params.set_defringe_radius(original_width as f32 / width.max(1) as f32);

        let uniform_buffer = context
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Uniform Buffer"),
                contents: bytemuck::cast_slice(&[gpu_params]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        // Phase 140: Create DCP Textures
        let has_dcp = dcp_profile.is_some();
        let dcp_has_curve = dcp_profile.is_some_and(|p| p.has_tone_curve);
        // X axis = hue divisions + 1 duplicated wrap row (see build_hsm_lut_upload)
        let lut_extent = dcp_profile
            .map(|p| wgpu::Extent3d {
                width: p.hue_sat_dims.0 + 1,
                height: p.hue_sat_dims.1,
                depth_or_array_layers: p.hue_sat_dims.2,
            })
            .unwrap_or(wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 });

        let hsv_lut_texture = context.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("HSV LUT 3D Texture"),
            size: lut_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        
        let tone_curve_texture = context.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Tone Curve 1D Texture"),
            size: wgpu::Extent3d { width: TONE_LUT_SIZE as u32, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D1,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        
        if let Some(dcp) = dcp_profile {
            let (lut_data, extent, padded_bytes_per_row) = build_hsm_lut_upload(dcp);
            context.queue.write_texture(
                wgpu::ImageCopyTexture { texture: &hsv_lut_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &lut_data,
                wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(padded_bytes_per_row), rows_per_image: Some(extent.height) },
                extent,
            );

            // Already baked for the selected operator — `interpolate_at_temperature`
            // takes the ToneSettings, so this is not necessarily the profile's
            // own curve.
            write_tone_curve(context, &tone_curve_texture, &dcp.tone_curve);
        } else if params.tone_mapper.is_baked_operator() {
            // No profile, but the user picked an operator. Nothing about these
            // operators is profile-dependent, so bake directly — this is what
            // lets tone mapping work on cameras with no DCP installed.
            write_tone_curve(
                context,
                &tone_curve_texture,
                &crate::core::tonemap::bake(params.tone_mapper, params.gt),
            );
        }

        let hsv_lut_view = hsv_lut_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let tone_curve_view = tone_curve_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let curve_texture = context.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Point Curve LUT"),
            size: wgpu::Extent3d {
                width: crate::core::curves::CURVE_LUT_SIZE as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D1,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let curve_view = curve_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mask_buffer = context.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mask Uniforms"),
            contents: bytemuck::bytes_of(&super::params::GpuMasks::from_params(params)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // A 1×1 stand-in for the brush layers until something is painted
        // (the bind group keeps it alive).
        let mask_bind_group = Arc::new(make_mask_bind_group(
            context,
            &super::brush::placeholder(&context.device),
            &mask_buffer,
        ));

        let base_bind_group = Arc::new(make_color_bind_group(
            context,
            &debayer_texture_view,
            &uniform_buffer,
            &hsv_lut_view,
            &tone_curve_view,
            &curve_view,
        ));

        // Noise model of the mosaic exactly as uploaded: the develop preview
        // is box-filtered, which lowers its noise, and the denoiser must know.
        let demosaic_params = super::demosaic::GpuDemosaicParams::new(
            cfa_pattern,
            black_levels,
            params.black_offsets,
            white_level,
            wb_multipliers,
        );
        let noise_model = crate::core::noise::estimate(
            &raw_data[..pixel_count],
            width as usize,
            height as usize,
            super::demosaic::tile_colors(cfa_pattern),
            demosaic_params.black,
            demosaic_params.scale,
            [demosaic_params.ceil[0], demosaic_params.ceil[1], demosaic_params.ceil[2]],
        );
        tracing::info!("Noise model: a={:?} b={:?}", noise_model.a, noise_model.b);

        // Dehaze's slider-independent half, on the same data and units.
        let haze = crate::core::haze::analyse(
            &raw_data[..pixel_count],
            width as usize,
            height as usize,
            super::demosaic::tile_colors(cfa_pattern),
            demosaic_params.black,
            demosaic_params.scale,
        );
        let analysis_wb = [demosaic_params.ceil[0], demosaic_params.ceil[1], demosaic_params.ceil[2]];
        let haze_airlight = (haze != crate::core::haze::HazeModel::default()).then_some(haze.airlight);
        if let Some(a) = haze_airlight {
            gpu_params.set_haze(a, analysis_wb);
        }
        let haze_view = haze_texture(context, &haze);

        // Highlight reconstruction's per-image half: the colour clipped
        // highlights are rebuilt in, learned from their unclipped rims.
        let highlights = crate::core::highlights::analyse(
            &raw_data[..pixel_count],
            width as usize,
            height as usize,
            super::demosaic::tile_colors(cfa_pattern),
            demosaic_params.black,
            demosaic_params.scale,
            analysis_wb,
        );
        let highlight_correction = highlights.correction;
        gpu_params.set_highlights(highlight_correction);
        // Both analyses ride the uniform; it was uploaded before them.
        context.queue.write_buffer(&uniform_buffer, 0, bytemuck::cast_slice(&[gpu_params]));

        let unused = super::local_contrast::placeholder(&context.device);
        let presence_bind_group = Arc::new(make_presence_bind_group(context, &unused, &haze_view, &unused));

        let resources = Self {
            texture,
            texture_view,
            debayer_texture,
            debayer_texture_view,
            stages: std::sync::Mutex::new(StageState::new(
                base_bind_group.clone(),
                mask_bind_group,
                presence_bind_group,
            )),
            mask_buffer,
            base_bind_group,
            stage_params: std::sync::Mutex::new(*params),
            lens,
            curve_texture,
            curve_view,
            noise_model,
            haze_airlight,
            analysis_wb,
            haze_view,
            highlight_correction,
            uniform_buffer,
            width,
            height,
            original_width,
            original_height,
            image_id,
            wb_multipliers,
            wb_current: std::sync::RwLock::new(wb_multipliers),
            forward_matrix: std::sync::RwLock::new(forward_matrix),
            cfa_pattern,
            black_levels,
            black_offsets: params.black_offsets,
            white_level,
            current_params: std::sync::Mutex::new(gpu_params),
            hsv_lut_texture,
            hsv_lut_view,
            tone_curve_texture,
            tone_curve_view,
            has_dcp,
            dcp_has_curve,
            profile_use: None,
            orientation,
            // None → the first update_uniforms after load re-uploads the LUTs
            // once with the anchored-kelvin interpolation (cheap, ~90 KB).
            last_uploaded_hsm_temp: std::sync::RwLock::new(None),
            last_uploaded_tone: std::sync::RwLock::new(None),
        };

        resources.run_demosaic(context);
        tracing::info!("RCD demosaic submitted for image {}", image_id);

        Ok(resources)
    }

    /// Pass 1: demosaic the R16Uint mosaic into `debayer_texture`. Depends
    /// only on the sensor data and its calibration, so it runs once per load.
    pub fn run_demosaic(&self, context: &SharedContext) {
        let params = super::demosaic::GpuDemosaicParams::new(
            self.cfa_pattern,
            self.black_levels,
            self.black_offsets,
            self.white_level,
            self.wb_multipliers,
        );
        context.demosaic.run(
            &context.device,
            &context.queue,
            &self.texture_view,
            self.width,
            self.height,
            &params,
            &self.debayer_texture_view,
        );
    }

    /// Bring the cached pre-pass stages up to date with the most recently
    /// pushed params, and return the colour bind group to render with.
    ///
    /// Called by every render entry point (develop, overview, export), so a
    /// stage can never be stale for the frame that reads it. Re-runs a stage
    /// only when its own settings changed: a drag on any other slider costs
    /// one mutex lock and a comparison here.
    pub fn prepare_stages(&self, context: &SharedContext) -> ColorBindings {
        let params = *self.stage_params.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = self.stages.lock().unwrap_or_else(|e| e.into_inner());

        // Point curves: re-bake only when they changed (the shader skips the
        // lookup entirely while they are identity — `curve_active`).
        if st.curves != Some(params.curves) {
            let lut: Vec<half::f16> = params
                .curves
                .bake()
                .iter()
                .flat_map(|e| e.map(half::f16::from_f32))
                .collect();
            context.queue.write_texture(
                self.curve_texture.as_image_copy(),
                bytemuck::cast_slice(&lut),
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(crate::core::curves::CURVE_LUT_SIZE as u32 * 8),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: crate::core::curves::CURVE_LUT_SIZE as u32,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            st.curves = Some(params.curves);
        }

        // Brush layers: re-rasterise only when the strokes changed. No
        // strokes → nothing to draw (the shader skips masks without any), and
        // the layers keep what `brush_key` describes: the before/after peek
        // has no strokes, and releasing it must not re-rasterise them.
        if !params.brush.is_empty() && st.brush_key != Some(params.brush) {
            // Layers are display-space and the rasteriser takes its aspect
            // from their size, so a quarter turn (which re-keys the strokes)
            // needs layers of the turned shape, or every dab draws as an
            // ellipse.
            let (dw, dh) = self.oriented_dims(params.quarter_turns);
            let (lw, lh) = super::brush::layer_size(dw, dh);
            if st.brush.as_ref().is_none_or(|l| (l.width, l.height) != (lw, lh)) {
                let layers = context.brush.layers(&context.device, lw, lh);
                st.masks = Arc::new(make_mask_bind_group(context, &layers.array_view, &self.mask_buffer));
                st.brush = Some(layers);
            }
            if let Some(layers) = &st.brush {
                context.brush.rasterize(&context.device, &context.queue, layers, &params.brush);
            }
            st.brush_key = Some(params.brush);
        }
        let st = &mut *st;
        self.prepare_denoise(context, st, &params);
        self.prepare_local_contrast(context, st, &params);
        ColorBindings { main: st.active.clone(), masks: st.masks.clone(), presence: st.presence.clone() }
    }

    /// Noise reduction: re-run when its settings changed, and point the
    /// colour pass at the result (or at the demosaic when it is off).
    fn prepare_denoise(&self, context: &SharedContext, st: &mut StageState, params: &EditParams) {
        let settings = super::denoise::DenoiseSettings::from_params(params);
        st.denoise_on = settings.is_active();
        if !st.denoise_on {
            st.active = self.base_bind_group.clone();
            return;
        }
        if st.denoise_key == Some(settings) {
            if let Some((_, groups)) = &st.denoise {
                st.active = groups[st.denoise_out].clone();
            }
            return;
        }

        if st.denoise.is_none() {
            let targets = context.denoise.targets(
                &context.device,
                &self.debayer_texture_view,
                self.width,
                self.height,
            );
            let groups = [0, 1].map(|i| {
                Arc::new(make_color_bind_group(
                    context,
                    targets.view(i),
                    &self.uniform_buffer,
                    &self.hsv_lut_view,
                    &self.tone_curve_view,
                    &self.curve_view,
                ))
            });
            st.denoise = Some((targets, groups));
        }
        let wb = self.analysis_wb;
        let stride = self.original_width as f32 / self.width.max(1) as f32;
        let passes = super::denoise::plan(settings, &self.noise_model, wb, stride);
        if let Some((targets, groups)) = &st.denoise {
            let out = context.denoise.run(&context.device, &context.queue, targets, &passes);
            st.active = groups[out].clone();
            st.denoise_out = out;
        }
        st.denoise_key = Some(settings);
    }

    /// Texture/Clarity bands: built from whatever the colour pass reads
    /// (so after `prepare_denoise`), and rebuilt only when that changes —
    /// never for the Texture or Clarity sliders themselves. Left stale while
    /// both are zero: the shader does not sample it then.
    fn prepare_local_contrast(&self, context: &SharedContext, st: &mut StageState, params: &EditParams) {
        if params.texture == 0.0 && params.clarity == 0.0 {
            return;
        }
        let source_key = st.denoise_key.filter(|_| st.denoise_on);
        if st.local_key == Some(source_key) {
            return;
        }
        let size = super::local_contrast::analysis_size(self.width, self.height);
        if st.local.is_none() {
            st.local = Some(context.local_contrast.targets(&context.device, &context.sampler_linear, size.0, size.1));
        }
        let source = match (&st.denoise, source_key) {
            (Some((targets, _)), Some(_)) => targets.view(st.denoise_out),
            _ => &self.debayer_texture_view,
        };
        if let Some(targets) = &st.local {
            let out = context.local_contrast.run(
                &context.device,
                &context.queue,
                &context.sampler_linear,
                targets,
                source,
                self.local_luma_weights(),
                size,
            );
            st.presence = Arc::new(make_presence_bind_group(
                context,
                targets.view(out.bands),
                &self.haze_view,
                targets.coarse_view(out.clarity),
            ));
        }
        st.local_key = Some(source_key);
    }

    /// Camera RGB → luma weights for the local-contrast bands, with the
    /// as-shot WB folded in, and the log floor in `w`: a few σ of this
    /// image's read noise, so that in log space the noise floor reads as
    /// flat black rather than as wildly contrasty structure.
    fn local_luma_weights(&self) -> [f32; 4] {
        let k = [0.25f32, 0.5, 0.25];
        let wb = self.analysis_wb;
        let read_var: f32 = (0..3).map(|c| k[c] * k[c] * self.noise_model.b[c]).sum();
        [k[0] * wb[0], k[1] * wb[1], k[2] * wb[2], (3.0 * read_var.sqrt()).max(1.0 / 4096.0)]
    }

    /// The params most recently pushed by `update_uniforms` — what this
    /// image's uniforms and stages currently render. Lets a second resource
    /// set for the same image (the develop preview kept beside a
    /// full-resolution upgrade) be brought to exactly the same state.
    /// A colour-pass bind group reading `source` with `uniform_buffer`, but
    /// this image's LUTs (DCP, tone, point curves). The detail loupe renders
    /// its own crop through it, so its look can't drift from the main view's.
    pub(crate) fn color_bind_group_with(
        &self,
        context: &SharedContext,
        source: &wgpu::TextureView,
        uniform_buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        make_color_bind_group(
            context,
            source,
            uniform_buffer,
            &self.hsv_lut_view,
            &self.tone_curve_view,
            &self.curve_view,
        )
    }

    pub fn pushed_params(&self) -> EditParams {
        *self.stage_params.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The orientation the colour pass renders: the EXIF one these resources
    /// were built with, turned by the edit's `quarter_turns`. Taken from the
    /// params rather than stored, so a turn costs a uniform upload — never a
    /// rebuild.
    pub fn display_orientation(&self, quarter_turns: u32) -> u32 {
        crate::color::compose_orientation(self.orientation, quarter_turns)
    }

    /// Display-space dimensions of the working (possibly subsampled) texture:
    /// swapped for 90°/270° orientations (EXIF and the user's turns together).
    pub fn oriented_dims(&self, quarter_turns: u32) -> (u32, u32) {
        match self.display_orientation(quarter_turns) {
            6 | 8 => (self.height, self.width),
            _ => (self.width, self.height),
        }
    }

    /// Display-space dimensions of the full-resolution sensor area.
    pub fn oriented_original_dims(&self, quarter_turns: u32) -> (u32, u32) {
        match self.display_orientation(quarter_turns) {
            6 | 8 => (self.original_height, self.original_width),
            _ => (self.original_width, self.original_height),
        }
    }

    /// Update uniforms with new edit parameters.
    ///
    /// `wb_override`: WB multipliers recomputed from the user's Temp/Tint
    /// sliders. None = sliders at zero → restore the exact as-shot
    /// multipliers ("keep current" semantics would make Reset a no-op for
    /// WB-only edits: the stale slider WB survives until a slider moves).
    pub fn update_uniforms(
        &self,
        context: &SharedContext,
        params: &EditParams,
        dcp_profile: Option<&crate::raw::dcp::InterpolatedProfile>,
        wb_override: Option<[f32; 4]>,
    ) {
        // Cache key uses the raw slider value: the caller re-interpolates the
        // DCP at its anchored kelvin whenever `temperature` moves, so the
        // slider value is a faithful proxy for "the LUT content changed".
        let current_temp = params.temperature;
        let tone = ToneSettings::from_params(params);

        let hsm_dirty = self
            .last_uploaded_hsm_temp
            .read()
            .ok()
            .and_then(|last| *last)
            .is_none_or(|t| (t - current_temp).abs() >= 0.0005);
        let tone_dirty = self
            .last_uploaded_tone
            .read()
            .ok()
            .and_then(|last| *last)
            .is_none_or(|prev| prev != tone);

        if let Some(dcp) = dcp_profile {
            if let Ok(mut matrix) = self.forward_matrix.write() {
                *matrix = dcp.camera_to_prophoto;
            }

            if hsm_dirty {
                // Re-upload the LUT texture (temperature slider re-interpolated it)
                let (lut_data, extent, padded_bytes_per_row) = build_hsm_lut_upload(dcp);
                context.queue.write_texture(
                    wgpu::ImageCopyTexture { texture: &self.hsv_lut_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    &lut_data,
                    wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(padded_bytes_per_row), rows_per_image: Some(extent.height) },
                    extent,
                );
                if let Ok(mut last) = self.last_uploaded_hsm_temp.write() {
                    *last = Some(current_temp);
                }
            }

            if tone_dirty {
                // Already baked for the selected operator by
                // `interpolate_at_temperature`, which receives the same
                // ToneSettings this key is built from.
                write_tone_curve(context, &self.tone_curve_texture, &dcp.tone_curve);
                if let Ok(mut last) = self.last_uploaded_tone.write() {
                    *last = Some(tone);
                }
            }
        } else if tone_dirty && tone.mapper.is_baked_operator() {
            // No profile: bake the operator directly. `Camera` needs nothing
            // here — with no profile curve to apply, the shader's own filmic
            // block renders it and never samples this texture.
            write_tone_curve(
                context,
                &self.tone_curve_texture,
                &crate::core::tonemap::bake(tone.mapper, tone.gt),
            );
            if let Ok(mut last) = self.last_uploaded_tone.write() {
                *last = Some(tone);
            }
        }

        // Update the active WB: slider-derived override, or as-shot when the
        // sliders are at zero.
        let wb = wb_override.unwrap_or(self.wb_multipliers);
        if let Ok(mut current_wb) = self.wb_current.write() {
            *current_wb = wb;
        }

        let mut gpu_params = GpuEditParams::from(params);
        gpu_params.wb_multipliers = wb;
        let fm = *self.forward_matrix.read().unwrap_or_else(|e| e.into_inner());
        gpu_params.forward_matrix_0 = [fm[0], fm[1], fm[2]];
        gpu_params.forward_matrix_1 = [fm[3], fm[4], fm[5]];
        gpu_params.forward_matrix_2 = [fm[6], fm[7], fm[8]];
        gpu_params.cfa_pattern = self.cfa_pattern;
        gpu_params.black_levels = self.black_levels;
        gpu_params.white_level = self.white_level;
        gpu_params.has_dcp = if self.has_dcp { 1 } else { 0 };
        gpu_params.dcp_has_curve = if self.dcp_has_curve { 1 } else { 0 };
        gpu_params.orientation = self.display_orientation(params.quarter_turns);
        gpu_params.set_lens(&crate::core::lens::gpu_params(
            &self.lens,
            &params.lens,
            self.original_width,
            self.original_height,
        ));
        gpu_params.set_defringe_radius(self.original_width as f32 / self.width.max(1) as f32);
        if let Some(a) = self.haze_airlight {
            gpu_params.set_haze(a, self.analysis_wb);
        }
        gpu_params.set_highlights(self.highlight_correction);

        context
            .queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[gpu_params]));

        if let Ok(mut current) = self.current_params.lock() {
            *current = gpu_params;
        }
        if let Ok(mut stage) = self.stage_params.lock() {
            *stage = *params;
        }
        context.queue.write_buffer(
            &self.mask_buffer,
            0,
            bytemuck::bytes_of(&super::params::GpuMasks::from_params(params)),
        );
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// Box-filtered downsampling must keep each CFA plane pure: build a mosaic
    /// where every plane holds a distinct constant — any cross-plane mixing or
    /// parity slip would corrupt the constants.
    #[test]
    fn test_downsample_bayer_preserves_cfa_planes() {
        let (w, h) = (30u32, 24u32);
        let plane = |x: usize, y: usize| -> u16 {
            match (y % 2, x % 2) {
                (0, 0) => 1000, // R
                (0, 1) => 2000, // G1
                (1, 0) => 3000, // G2
                _ => 4000,      // B
            }
        };
        let data: Vec<u16> = (0..(w * h) as usize)
            .map(|i| plane(i % w as usize, i / w as usize))
            .collect();

        for stride in [1u32, 3, 5] {
            let (out, nw, nh) = downsample_bayer(&data, w, h, stride);
            assert_eq!(out.len(), (nw * nh) as usize);
            for y in 0..nh as usize {
                for x in 0..nw as usize {
                    assert_eq!(
                        out[y * nw as usize + x],
                        plane(x, y),
                        "plane corrupted at ({x},{y}) stride {stride}"
                    );
                }
            }
        }
    }

    /// The filter must actually average within a plane (noise reduction), not
    /// point-sample: two different R values inside one block → their mean.
    #[test]
    fn test_downsample_bayer_averages_within_plane() {
        let (w, h) = (6u32, 6u32);
        let mut data = vec![100u16; (w * h) as usize];
        // R sites in the first 3×3 block: (0,0) and (2,0), (0,2), (2,2)
        data[0] = 500; // (0,0)
        data[2] = 300; // (2,0)
        data[2 * 6] = 100; // (0,2)
        data[2 * 6 + 2] = 100; // (2,2)
        let (out, nw, _) = downsample_bayer(&data, w, h, 3);
        // Output (0,0) = mean(500, 300, 100, 100) = 250
        assert_eq!(out[0], 250);
        assert_eq!(nw, 2);
    }
}
