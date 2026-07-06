/// Phase 95: Shared GPU Context and Per-Image Resources
///
/// This module contains the refactored pipeline architecture:
/// - SharedContext: Persistent GPU resources (created once)
/// - ImageResources: Per-image data (created for each image)
use iced_wgpu::wgpu;
use std::sync::Arc;
use wgpu::util::DeviceExt;

use super::pipeline::GpuEditParams;
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
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Phase 128: Debayer pipeline (Pass 1 — reads R16Uint, writes Rgba16Float)
    pub debayer_pipeline: wgpu::RenderPipeline,
    pub debayer_bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
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

        // Request device and queue
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("RAW Editor Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .map_err(|e| format!("Failed to create device: {:?}", e))?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        // Phase 128: Load both shaders
        let color_shader_source = shaders::get_color_shader();
        let color_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Color Shader (Pass 2)"),
            source: wgpu::ShaderSource::Wgsl(color_shader_source.into()),
        });

        let debayer_shader_source = shaders::get_debayer_shader();
        let debayer_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Debayer Shader (Pass 1)"),
            source: wgpu::ShaderSource::Wgsl(debayer_shader_source.into()),
        });

        // Phase 128: Debayer bind group layout (reads R16Uint)
        let debayer_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Debayer Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
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
            ],
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
            ],
        });

        // Phase 128: Debayer pipeline (Pass 1 → Rgba16Float)
        let debayer_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Debayer Pipeline Layout"),
            bind_group_layouts: &[&debayer_bind_group_layout],
            push_constant_ranges: &[],
        });

        let debayer_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Debayer Render Pipeline (Pass 1)"),
            layout: Some(&debayer_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &debayer_shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &debayer_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: None,
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

        // Color pipeline (Pass 2 → Rgba8Unorm canvas)
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Color Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
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

        // Create samplers
        // 1. Nearest sampler (NonFiltering — for Pass 1 RAW u16 integer texture)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("RAW Nearest Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // 2. Linear sampler (Filtering — for Pass 2 smooth preview downscaling)
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
            bind_group_layout,
            debayer_pipeline,
            debayer_bind_group_layout,
            sampler,
            sampler_linear,
        })
    }
}

/// Per-image GPU resources (created for each image)
/// Contains all the image-specific data that changes when switching images
#[derive(Debug)]
pub struct ImageResources {
    pub texture: wgpu::Texture,
    pub texture_view: wgpu::TextureView,
    /// Phase 128: Debayer bind group (Pass 1 input: R16Uint raw texture)
    pub debayer_bind_group: wgpu::BindGroup,
    /// Phase 128: Intermediate debayered texture (Rgba16Float, full resolution)
    pub debayer_texture: wgpu::Texture,
    pub debayer_texture_view: wgpu::TextureView,
    /// Phase 128: Color bind group (Pass 2 input: Rgba16Float intermediate)
    pub bind_group: wgpu::BindGroup,
    pub uniform_buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    pub original_width: u32,
    pub original_height: u32,
    pub preview_width: u32,
    pub preview_height: u32,
    pub histogram_width: u32,
    pub histogram_height: u32,
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
    pub white_level: u32,
    pub current_params: std::sync::Mutex<GpuEditParams>,
    // Phase 140: DCP Textures
    pub hsv_lut_texture: wgpu::Texture,
    pub hsv_lut_view: wgpu::TextureView,
    pub tone_curve_texture: wgpu::Texture,
    pub tone_curve_view: wgpu::TextureView,
    pub has_dcp: bool,
    pub dcp_has_curve: bool,
    /// Normalised EXIF orientation (1/3/6/8). Sensor textures stay landscape;
    /// the color pass maps display UVs to sensor space, and render targets are
    /// sized to `oriented_*` dims.
    pub orientation: u32,
    /// Cache key of the last DCP LUT upload: (kelvin, profile_curve strength).
    /// Skips redundant HSM-3D-LUT / tone-curve texture uploads per render.
    pub last_uploaded_dcp_kelvin: std::sync::RwLock<Option<(f32, f32)>>,
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
    ) -> Result<Self, String> {
        const DEFAULT_PREVIEW_WIDTH: u32 = 1280;
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

        let aspect_ratio = width as f32 / height as f32;
        let preview_width = width.min(DEFAULT_PREVIEW_WIDTH);
        let preview_height = (preview_width as f32 / aspect_ratio) as u32;

        const HISTOGRAM_WIDTH: u32 = 128;
        let histogram_width = HISTOGRAM_WIDTH;
        let histogram_height = (histogram_width as f32 / aspect_ratio) as u32;

        tracing::debug!(
            "Texture {}x{}, Preview {}x{}, Histogram {}x{}",
            width,
            height,
            preview_width,
            preview_height,
            histogram_width,
            histogram_height
        );

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

        // Upload RAW data with padding
        let bytes_per_pixel = 2;
        let unpadded_bytes_per_row = width * bytes_per_pixel;
        let padded_bytes_per_row = (unpadded_bytes_per_row + 255) & !255;

        let raw_bytes = bytemuck::cast_slice(raw_data);

        if unpadded_bytes_per_row == padded_bytes_per_row {
            context.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                raw_bytes,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(unpadded_bytes_per_row),
                    rows_per_image: Some(height),
                },
                texture_size,
            );
        } else {
            let mut padded_data = Vec::with_capacity((padded_bytes_per_row * height) as usize);
            for y in 0..height {
                let start = (y * unpadded_bytes_per_row) as usize;
                let end = start + unpadded_bytes_per_row as usize;
                padded_data.extend_from_slice(&raw_bytes[start..end]);
                padded_data.extend(
                    std::iter::repeat_n(0, (padded_bytes_per_row - unpadded_bytes_per_row) as usize),
                );
            }

            context.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &padded_data,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
                texture_size,
            );
        }

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
        gpu_params.orientation = orientation;

        let uniform_buffer = context
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Uniform Buffer"),
                contents: bytemuck::cast_slice(&[gpu_params]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        // Phase 128: Debayer bind group (Pass 1 reads R16Uint raw texture)
        let debayer_bind_group = context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Debayer Bind Group"),
                layout: &context.debayer_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&context.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                ],
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
            size: wgpu::Extent3d { width: 1024, height: 1, depth_or_array_layers: 1 },
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

            // Write Tone Curve
            let tc_f16: Vec<half::f16> = dcp.tone_curve.iter().map(|&v| half::f16::from_f32(v)).collect();
            context.queue.write_texture(
                wgpu::ImageCopyTexture { texture: &tone_curve_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                bytemuck::cast_slice(&tc_f16),
                wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(1024 * 2), rows_per_image: None },
                wgpu::Extent3d { width: 1024, height: 1, depth_or_array_layers: 1 },
            );
        }

        let hsv_lut_view = hsv_lut_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let tone_curve_view = tone_curve_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Phase 128: Color bind group (Pass 2 reads Rgba16Float intermediate)
        let bind_group = context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Color Bind Group"),
                layout: &context.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&debayer_texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&context.sampler_linear),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&hsv_lut_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&tone_curve_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&context.sampler_linear), // Clamp to edge is default
                    },
                ],
            });

        let resources = Self {
            texture,
            texture_view,
            debayer_bind_group,
            debayer_texture,
            debayer_texture_view,
            bind_group,
            uniform_buffer,
            width,
            height,
            original_width,
            original_height,
            preview_width,
            preview_height,
            histogram_width,
            histogram_height,
            image_id,
            wb_multipliers,
            wb_current: std::sync::RwLock::new(wb_multipliers),
            forward_matrix: std::sync::RwLock::new(forward_matrix),
            cfa_pattern,
            black_levels,
            white_level,
            current_params: std::sync::Mutex::new(gpu_params),
            hsv_lut_texture,
            hsv_lut_view,
            tone_curve_texture,
            tone_curve_view,
            has_dcp,
            dcp_has_curve,
            orientation,
            // None → the first update_uniforms after load re-uploads the LUTs
            // once with the anchored-kelvin interpolation (cheap, ~90 KB).
            last_uploaded_dcp_kelvin: std::sync::RwLock::new(None),
        };

        // Phase 128: Run initial debayer pass
        resources.run_debayer(context);
        tracing::info!("Phase 128: Initial debayer pass complete for image {}", image_id);

        Ok(resources)
    }

    /// Phase 128: Execute the debayer pass (Pass 1).
    /// Renders the R16Uint raw texture into the Rgba16Float intermediate texture.
    /// Call this when the image changes or sensor parameters (black level) change.
    pub fn run_debayer(&self, context: &SharedContext) {
        let mut encoder = context.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Debayer Encoder"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Debayer Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.debayer_texture_view,
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
            rpass.set_viewport(0.0, 0.0, self.width as f32, self.height as f32, 0.0, 1.0);
            rpass.set_pipeline(&context.debayer_pipeline);
            rpass.set_bind_group(0, &self.debayer_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
        context.queue.submit(Some(encoder.finish()));
    }

    /// Display-space dimensions of the working (possibly subsampled) texture:
    /// swapped for 90°/270° EXIF orientations.
    pub fn oriented_dims(&self) -> (u32, u32) {
        match self.orientation {
            6 | 8 => (self.height, self.width),
            _ => (self.width, self.height),
        }
    }

    /// Display-space dimensions of the full-resolution sensor area.
    pub fn oriented_original_dims(&self) -> (u32, u32) {
        match self.orientation {
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
        let current_strength = params.profile_curve;
        let mut needs_lut_upload = true;

        if let Ok(last_key) = self.last_uploaded_dcp_kelvin.read() {
            if let Some((t, s)) = *last_key {
                if (t - current_temp).abs() < 0.0005 && (s - current_strength).abs() < 0.001 {
                    needs_lut_upload = false;
                }
            }
        }

        if let Some(dcp) = dcp_profile {
            if let Ok(mut matrix) = self.forward_matrix.write() {
                *matrix = dcp.forward_matrix;
            }

            if needs_lut_upload {
                // Re-upload the LUT texture (temperature slider re-interpolated it)
                let (lut_data, extent, padded_bytes_per_row) = build_hsm_lut_upload(dcp);
                context.queue.write_texture(
                    wgpu::ImageCopyTexture { texture: &self.hsv_lut_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    &lut_data,
                    wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(padded_bytes_per_row), rows_per_image: Some(extent.height) },
                    extent,
                );

                // Re-upload the tone curve (Profile Curve slider re-baked it)
                let tc_f16: Vec<half::f16> = dcp.tone_curve.iter().map(|&v| half::f16::from_f32(v)).collect();
                context.queue.write_texture(
                    wgpu::ImageCopyTexture { texture: &self.tone_curve_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    bytemuck::cast_slice(&tc_f16),
                    wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(1024 * 2), rows_per_image: None },
                    wgpu::Extent3d { width: 1024, height: 1, depth_or_array_layers: 1 },
                );

                if let Ok(mut last) = self.last_uploaded_dcp_kelvin.write() {
                    *last = Some((current_temp, current_strength));
                }
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
        gpu_params.orientation = self.orientation;

        context
            .queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[gpu_params]));

        if let Ok(mut current) = self.current_params.lock() {
            *current = gpu_params;
        }
    }

    /// Update uniforms with zoom and pan
    pub fn update_uniforms_with_zoom(
        &self,
        context: &SharedContext,
        params: &EditParams,
        zoom: f32,
        pan_x: f32,
        pan_y: f32,
    ) {
        let mut gpu_params = GpuEditParams::from(params);
        // Use the ACTIVE WB (slider-derived), not the as-shot multipliers —
        // otherwise a zoom/resize render silently reverts the WB sliders.
        gpu_params.wb_multipliers = *self.wb_current.read().unwrap_or_else(|e| e.into_inner());
        let fm = *self.forward_matrix.read().unwrap_or_else(|e| e.into_inner());
        gpu_params.forward_matrix_0 = [fm[0], fm[1], fm[2]];
        gpu_params.forward_matrix_1 = [fm[3], fm[4], fm[5]];
        gpu_params.forward_matrix_2 = [fm[6], fm[7], fm[8]];
        gpu_params.cfa_pattern = self.cfa_pattern;
        gpu_params.black_levels = self.black_levels;
        gpu_params.white_level = self.white_level;
        gpu_params.has_dcp = if self.has_dcp { 1 } else { 0 };
        gpu_params.dcp_has_curve = if self.dcp_has_curve { 1 } else { 0 };
        gpu_params.orientation = self.orientation;
        gpu_params.zoom = zoom;
        gpu_params.pan_x = pan_x;
        gpu_params.pan_y = pan_y;

        context
            .queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[gpu_params]));

        if let Ok(mut current) = self.current_params.lock() {
            *current = gpu_params;
        }
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
