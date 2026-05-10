use crate::core::types::EditParams;
/// wgpu render pipeline for real-time RAW image processing
///
/// This module manages all the wgpu boilerplate:
/// - Device and queue initialization
/// - Texture creation and uploads
/// - Uniform buffer for edit parameters
/// - Render pipeline state
/// - Draw commands
// Use wgpu from iced to avoid dependency conflicts
use iced_wgpu::wgpu;
use wgpu::util::DeviceExt;

/// Represents the edit parameters in a GPU-friendly format
/// Must match the WGSL struct layout with proper alignment
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuEditParams {
    exposure: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    vibrance: f32,
    saturation: f32,
    temperature: f32,
    tint: f32,
    padding1: f32, // For 16-byte alignment
    padding2: f32,
    // Phase 140: Color science (must match WGSL layout!)
    pub wb_multipliers: [f32; 4], // White balance [R, G, B, G2] - vec4 in WGSL
    // Forward matrix split into 3 rows with padding (WGSL vec3 = 12 bytes + 4 padding)
    pub forward_matrix_0: [f32; 3], // Row 0
    _padding3: f32,
    pub forward_matrix_1: [f32; 3], // Row 1
    _padding4: f32,
    pub forward_matrix_2: [f32; 3], // Row 2
    _padding5: f32,
    // Phase 25: Zoom & Pan
    pub zoom: f32,  // Zoom level (1.0 = 100%)
    pub pan_x: f32, // Pan offset X
    pub pan_y: f32, // Pan offset Y
    // Phase 34: CFA Pattern
    pub cfa_pattern: u32, // Offset 128. Ends at 132.

    // Padding to align black_levels to 16 bytes (Offset 144)
    _pad_cfa_1: f32, // 136 (Wait, 124+4=128. So this is 128)
    // Let's recount offsets:
    // cfa_pattern: 124. Ends 128.
    // pad1: 128. Ends 132.
    // pad2: 132. Ends 136.
    // pad3: 136. Ends 140.
    // pad4: 140. Ends 144.
    _pad_cfa_2: f32,
    _pad_cfa_3: f32,
    _pad_cfa_4: f32, // 144

    // Phase 36: Per-channel Black Levels (vec4 alignment = 16 bytes)
    pub black_levels: [u32; 4], // Offset 144. Ends at 160.

    pub white_level: u32, // Offset 160. Ends at 164.

    // Padding to reach 176 bytes (16-byte alignment for struct)
    _pad_end_1: f32, // 168
    _pad_end_2: f32, // 172
    _pad_end_3: f32, // 176

    // Phase 38: Manual Black Level Offsets (vec4 alignment = 16 bytes)
    black_offsets: [f32; 4], // Offset 176. Ends at 192.

    // Phase 39: Black Level Phase Correction
    black_phase_x: u32, // Offset 192. Ends at 196.
    black_phase_y: u32, // Offset 196. Ends at 200.

    // Phase 133: Split Luma & Chroma Noise Reduction
    luma_noise: f32, // Offset 200. Ends at 204.
    color_noise: f32, // Offset 204. Ends at 208.

    // Phase 50: Sharpening
    sharpening: f32, // Offset 208. Ends at 212.

    // Phase 51: Sharpening Masking
    sharpen_masking: f32, // Offset 212. Ends at 216.

    // Phase 52: Rotation
    rotation: f32, // Offset 216. Ends at 220.

    // Padding to reach 224 bytes (16-byte alignment for struct)
    _pad_phase_1: f32, // 220

    // Phase 66: Crop (vec4 alignment = 16 bytes)
    pub crop: [f32; 4], // Offset 224. Ends at 240.

    // Phase 135: Non-destructive crop visibility
    pub is_cropping: u32, // Offset 240. Ends at 244.

    // Phase 140: DCP pipeline toggle
    pub has_dcp: u32,     // Offset 244. Ends at 248.

    // Padding to reach 256 bytes (16-byte alignment for struct)
    _pad_crop_2: u32, // 252
    _pad_crop_3: u32, // 256
}

impl From<&EditParams> for GpuEditParams {
    fn from(params: &EditParams) -> Self {
        Self {
            exposure: params.exposure,
            contrast: params.contrast,
            highlights: params.highlights,
            shadows: params.shadows,
            whites: params.whites,
            blacks: params.blacks,
            vibrance: params.vibrance,
            saturation: params.saturation,
            temperature: params.temperature,
            tint: params.tint,
            padding1: 0.0,
            padding2: 0.0,
            // Default values (will be overwritten by set_color_metadata)
            wb_multipliers: [1.0, 1.0, 1.0, 1.0],
            forward_matrix_0: [1.0, 0.0, 0.0],
            _padding3: 0.0,
            forward_matrix_1: [0.0, 1.0, 0.0],
            _padding4: 0.0,
            forward_matrix_2: [0.0, 0.0, 1.0],
            _padding5: 0.0,
            // Phase 25: Default zoom and pan (no zoom, no pan)
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            cfa_pattern: 0,
            _pad_cfa_1: 0.0,
            _pad_cfa_2: 0.0,
            _pad_cfa_3: 0.0,
            _pad_cfa_4: 0.0,
            black_levels: [0, 0, 0, 0],
            white_level: 65535,
            _pad_end_1: 0.0,
            _pad_end_2: 0.0,
            _pad_end_3: 0.0,
            black_offsets: params.black_offsets,
            black_phase_x: params.black_phase_x,
            black_phase_y: params.black_phase_y,
            luma_noise: params.luma_noise,
            color_noise: params.color_noise,
            sharpening: params.sharpening,
            sharpen_masking: params.sharpen_masking,
            rotation: params.rotation,
            _pad_phase_1: 0.0,
            crop: params.crop,
            is_cropping: params.is_cropping,
            has_dcp: 0,
            _pad_crop_2: 0,
            _pad_crop_3: 0,
        }
    }
}

/// Main render pipeline for RAW image processing
pub struct RenderPipeline {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    pub width: u32,          // Full resolution width
    pub height: u32,         // Full resolution height
    pub preview_width: u32,  // Preview resolution width (for fast rendering)
    pub preview_height: u32, // Preview resolution height (for fast rendering)
    pub image_id: i64,       // Phase 20: Track which image this pipeline is for
    // Phase 22: Histogram optimization - tiny render for instant histogram
    pub histogram_width: u32,  // Histogram resolution width (256px)
    pub histogram_height: u32, // Histogram resolution height (maintains aspect)
    // Phase 14: Color science metadata
    pub wb_multipliers: [f32; 4], // White balance from camera
    pub forward_matrix: [f32; 9],   // Color correction matrix
    has_dcp: bool,            // DCP active toggle
    cfa_pattern: u32,         // Phase 34: CFA Pattern
    black_levels: [u32; 4],   // Phase 36: Per-channel Black Levels
    white_level: u32,         // Phase 35: White Level

    // Phase 67: Store current params for temporary updates (e.g. export)
    // Wrapped in Mutex for interior mutability since Pipeline is shared via Arc
    current_params: std::sync::Mutex<GpuEditParams>,
}

// Manual Debug implementation (wgpu types don't implement Debug)
impl std::fmt::Debug for RenderPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderPipeline")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl RenderPipeline {
    /// Create a new render pipeline with the given RAW data
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        image_id: i64, // Phase 20: Track which image this pipeline is for
        raw_data: Vec<u16>,
        width: u32,
        height: u32,
        params: &EditParams,
        wb_multipliers: [f32; 4],
        forward_matrix: [f32; 9],
        has_dcp: bool,
        cfa_pattern: u32,
        black_levels: [u32; 4],
        white_level: u32,
    ) -> Result<Self, String> {
        // Calculate preview dimensions for fast rendering
        // Phase 13: Render to smaller texture to eliminate 1-2s lag
        const MAX_PREVIEW_WIDTH: u32 = 1280;
        let aspect_ratio = width as f32 / height as f32;
        let preview_width = width.min(MAX_PREVIEW_WIDTH);
        let preview_height = (preview_width as f32 / aspect_ratio) as u32;

        // Phase 22: Calculate tiny histogram dimensions for instant calculation
        const HISTOGRAM_WIDTH: u32 = 128;
        let histogram_width = HISTOGRAM_WIDTH;
        let histogram_height = (histogram_width as f32 / aspect_ratio) as u32;

        tracing::debug!("Full resolution: {}x{}", width, height);
        tracing::debug!(
            "Preview resolution: {}x{} ({:.1}% of full)",
            preview_width,
            preview_height,
            (preview_width * preview_height) as f32 / (width * height) as f32 * 100.0
        );
        tracing::debug!(
            "Histogram resolution: {}x{} ({:.3}% of full)",
            histogram_width,
            histogram_height,
            (histogram_width * histogram_height) as f32 / (width * height) as f32 * 100.0
        );

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

        // Create texture for RAW u16 data (R16Uint format)
        let texture_size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("RAW Input Texture (R16Uint)"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Uint, // 16-bit unsigned integer for RAW data
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload RAW u16 data
        // CRITICAL: wgpu requires bytes_per_row to be a multiple of 256
        let bytes_per_pixel = 2;
        let unpadded_bytes_per_row = width * bytes_per_pixel;
        let padded_bytes_per_row = (unpadded_bytes_per_row + 255) & !255;

        let raw_bytes = bytemuck::cast_slice(&raw_data);

        if unpadded_bytes_per_row == padded_bytes_per_row {
            // Already aligned, upload directly
            tracing::debug!(
                "Uploading {} bytes of RAW u16 data to GPU (Aligned)",
                raw_bytes.len()
            );
            queue.write_texture(
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
            // Need to pad rows
            tracing::debug!(
                "Uploading RAW u16 data with padding (Row: {} -> {})",
                unpadded_bytes_per_row,
                padded_bytes_per_row
            );
            let mut padded_data = Vec::with_capacity((padded_bytes_per_row * height) as usize);
            for y in 0..height {
                let start = (y * unpadded_bytes_per_row) as usize;
                let end = start + unpadded_bytes_per_row as usize;
                padded_data.extend_from_slice(&raw_bytes[start..end]);
                // Add padding
                padded_data.extend(
                    std::iter::repeat_n(0, (padded_bytes_per_row - unpadded_bytes_per_row) as usize),
                );
            }

            queue.write_texture(
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
        tracing::debug!("RAW texture uploaded to GPU!");

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create sampler
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("RAW Texture Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Create uniform buffer with color metadata
        let mut gpu_params: GpuEditParams = params.into();
        // Phase 14: Set color science metadata from camera
        gpu_params.wb_multipliers = wb_multipliers;
        // Split flat forward_matrix [9] into 3 rows with padding
        gpu_params.forward_matrix_0 = [forward_matrix[0], forward_matrix[1], forward_matrix[2]];
        gpu_params.forward_matrix_1 = [forward_matrix[3], forward_matrix[4], forward_matrix[5]];
        gpu_params.forward_matrix_2 = [forward_matrix[6], forward_matrix[7], forward_matrix[8]];
        gpu_params.has_dcp = if has_dcp { 1 } else { 0 };
        gpu_params.cfa_pattern = cfa_pattern;
        gpu_params.black_levels = black_levels;
        gpu_params.white_level = white_level;

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Edit Params Uniform Buffer"),
            contents: bytemuck::cast_slice(&[gpu_params]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Bind Group Layout"),
            entries: &[
                // Texture (R16Uint = unsigned integer texture)
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint, // Integer texture for RAW u16
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // Sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // Uniform buffer
                // Phase 25: VERTEX visibility added for zoom/pan in vertex shader
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
                // After your existing 3 bind group layout entries, add:
                // binding 3 — HSV LUT (texture_3d)
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
                // binding 4 — Tone curve (texture_1d)
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
                // binding 5 — LUT sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // Create bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });

        // Load shader
        let shader_source = super::shaders::get_shader();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RAW Processing Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create render pipeline
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("RAW Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None, // Disable culling for full-screen triangle
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_group,
            uniform_buffer,
            texture,
            texture_view,
            width,
            height,
            preview_width,
            preview_height,
            image_id,         // Phase 20: Track which image this pipeline is for
            histogram_width,  // Phase 22: Tiny render for histogram
            histogram_height, // Phase 22: Tiny render for histogram
            wb_multipliers,
            forward_matrix,
            has_dcp: false,
            cfa_pattern,
            black_levels,
            white_level,

            // Phase 67: Initialize current params with the fully populated gpu_params (including metadata!)
            current_params: std::sync::Mutex::new(gpu_params),
        })
    }

    /// Update uniform buffer with new edit parameters
    /// Phase 25: Now includes zoom and pan for Canvas rendering
    pub fn update_uniforms(&self, params: &EditParams) {
        self.update_uniforms_with_zoom(params, 1.0, 0.0, 0.0);
    }

    /// Update uniform buffer with zoom and pan
    /// Phase 25: Full control over all uniforms including zoom/pan
    pub fn update_uniforms_with_zoom(
        &self,
        params: &EditParams,
        zoom: f32,
        pan_x: f32,
        pan_y: f32,
    ) {
        let mut gpu_params = GpuEditParams::from(params);
        // Preserve color metadata (doesn't change with slider updates)
        gpu_params.wb_multipliers = self.wb_multipliers;
        // Convert flat matrix to split rows
        let cm = &self.forward_matrix;
        gpu_params.forward_matrix_0 = [cm[0], cm[1], cm[2]];
        gpu_params.forward_matrix_1 = [cm[3], cm[4], cm[5]];
        gpu_params.forward_matrix_2 = [cm[6], cm[7], cm[8]];
        // Phase 25: Set zoom and pan
        gpu_params.zoom = zoom;
        gpu_params.pan_x = pan_x;
        gpu_params.pan_y = pan_y;
        gpu_params.cfa_pattern = self.cfa_pattern;
        gpu_params.black_levels = self.black_levels;
        gpu_params.white_level = self.white_level;
        // Phase 66: Set crop
        gpu_params.crop = params.crop;

        crate::debug_log!(crate::debug::DEBUG_GPU, "🎨 GPU Uniforms Updated:");
        crate::debug_log!(
            crate::debug::DEBUG_GPU,
            "   Exposure: {:.2}, Contrast: {:.0}",
            gpu_params.exposure,
            gpu_params.contrast
        );
        crate::debug_log!(
            crate::debug::DEBUG_GPU,
            "   Highlights: {:.0}, Shadows: {:.0}",
            gpu_params.highlights,
            gpu_params.shadows
        );
        crate::debug_log!(
            crate::debug::DEBUG_GPU,
            "   Temp: {}, Tint: {}",
            gpu_params.temperature,
            gpu_params.tint
        );
        crate::debug_log!(
            crate::debug::DEBUG_GPU,
            "   Zoom: {:.1}%, Pan: ({:.3}, {:.3})",
            zoom * 100.0,
            pan_x,
            pan_y
        );
        crate::debug_log!(
            crate::debug::DEBUG_GPU,
            "   Zoom: {:.1}%, Pan: ({:.3}, {:.3})",
            zoom * 100.0,
            pan_x,
            pan_y
        );
        crate::debug_log!(crate::debug::DEBUG_GPU, "   Crop: {:?}", gpu_params.crop);

        // Phase 67: Store current params (with metadata!)
        if let Ok(mut current) = self.current_params.lock() {
            *current = gpu_params;
        }

        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[gpu_params]));
    }

    /// Render directly to an iced-provided texture view (Canvas integration)
    /// This eliminates the GPU→CPU readback bottleneck!
    pub fn render_to_target(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: (u32, u32),
    ) {
        // Create render pass that draws directly to iced's surface
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("RAW Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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

        // Set viewport to match canvas size
        render_pass.set_viewport(0.0, 0.0, viewport.0 as f32, viewport.1 as f32, 0.0, 1.0);

        // Execute our shader
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        render_pass.draw(0..3, 0..1); // Full-screen triangle
    }

    /// Phase 13: Render to preview resolution for fast updates
    /// Renders full RAW texture to smaller output (GPU downsamples automatically)
    /// Phase 66: Added width/height args to support dynamic aspect ratios for cropping
    pub fn render_to_bytes(&self, width: u32, height: u32) -> Vec<u8> {
        // Create PREVIEW-SIZED output texture (Phase 13 optimization!)
        let output_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Output Texture (Preview)"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        // Render to PREVIEW texture (GPU rasterizer auto-downsamples from full res input)
        self.render_to_target(&mut encoder, &output_view, (width, height));

        // Readback from PREVIEW buffer (much smaller!)
        let bytes_per_row = width * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        let buffer_size = (padded_bytes_per_row * height) as u64;

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Output Buffer"),
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
                buffer: &output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(Some(encoder.finish()));

        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::error!("GPU buffer map failed: {:?}", e);
                return Vec::new();
            }
            Err(e) => {
                tracing::error!("GPU buffer map channel error: {:?}", e);
                return Vec::new();
            }
        }

        let data = buffer_slice.get_mapped_range();
        let mut output = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let start = (y * padded_bytes_per_row) as usize;
            let end = start + (width * 4) as usize;
            output.extend_from_slice(&data[start..end]);
        }

        drop(data);
        output_buffer.unmap();
        output
    }

    /// Phase 19: Render to FULL resolution for export
    /// This is SLOW (1-2 seconds for 24MP) - only use for final export!
    /// Phase 17: Render full resolution to bytes (for export)
    /// Phase 67: Added crop parameter
    /// Phase 17: Render full resolution to bytes (for export)
    /// Phase 67: Added crop parameter
    pub fn render_full_res_to_bytes(
        &self,
        output_format: wgpu::TextureFormat,
        crop: [f32; 4],
    ) -> Result<Vec<u8>, String> {
        // Calculate target dimensions based on crop
        let crop_w = crop[2];
        let crop_h = crop[3];

        let target_width = (self.width as f32 * crop_w) as u32;
        let target_height = (self.height as f32 * crop_h) as u32;

        tracing::info!(
            "Exporting crop: {}x{} (Original: {}x{})",
            target_width,
            target_height,
            self.width,
            self.height
        );

        // Create FULL-SIZED (or cropped) output texture
        let output_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Output Texture (Full Resolution)"),
            size: wgpu::Extent3d {
                width: target_width,
                height: target_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: output_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder (Full Res)"),
            });

        // Phase 67: Update uniforms with the crop parameters for this export
        // We need to temporarily update the uniforms to reflect the crop
        let mut export_params = if let Ok(current) = self.current_params.lock() {
            *current
        } else {
            return Err("Failed to lock params".to_string());
        };

        export_params.crop = crop; // Set the crop

        // Write to buffer
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[export_params]),
        );

        // Render to texture
        self.render_to_target(&mut encoder, &output_view, (target_width, target_height));

        // Readback from buffer
        let bytes_per_row = target_width * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        let buffer_size = (padded_bytes_per_row * target_height) as u64;

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Output Buffer (Full Res)"),
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
                buffer: &output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(target_height),
                },
            },
            wgpu::Extent3d {
                width: target_width,
                height: target_height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(Some(encoder.finish()));

        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
        self.device.poll(wgpu::Maintain::Wait);

        match rx.recv() {
            Ok(Ok(())) => {
                let data = buffer_slice.get_mapped_range();
                let mut output = Vec::with_capacity((target_width * target_height * 4) as usize);
                for y in 0..target_height {
                    let start = (y * padded_bytes_per_row) as usize;
                    let end = start + (target_width * 4) as usize;
                    if end <= data.len() {
                        output.extend_from_slice(&data[start..end]);
                    } else {
                        tracing::warn!("Export buffer underrun at row {}", y);
                    }
                }

                drop(data);
                output_buffer.unmap();
                Ok(output)
            }
            Ok(Err(e)) => {
                tracing::error!("GPU Readback failed: {:?}", e);
                Err(format!("GPU Readback failed: {:?}", e))
            }
            Err(e) => {
                tracing::error!("GPU Readback channel error: {:?}", e);
                Err(format!("GPU Readback channel error: {:?}", e))
            }
        }
    }

    /// Get the texture dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Phase 22: Render to tiny histogram-sized bytes (256px wide)
    /// This is ~100x faster than rendering full preview for histogram calculation
    pub fn render_to_histogram_bytes(&self) -> Vec<u8> {
        // Create tiny output texture for histogram (256px wide)
        let output_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Histogram Output Texture"),
            size: wgpu::Extent3d {
                width: self.histogram_width,
                height: self.histogram_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create command encoder and render pass
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Histogram Render Encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Histogram Render Pass"),
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

            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }

        // Read back the tiny rendered image
        let bytes_per_pixel = 4;
        let unpadded_bytes_per_row = self.histogram_width * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;
        let buffer_size = (padded_bytes_per_row * self.histogram_height) as u64;

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Histogram Output Buffer"),
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
                buffer: &output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(self.histogram_height),
                },
            },
            wgpu::Extent3d {
                width: self.histogram_width,
                height: self.histogram_height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(Some(encoder.finish()));

        // Read the data
        let buffer_slice = output_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::Maintain::Wait);

        let data = buffer_slice.get_mapped_range();

        // Copy to output vector (remove padding)
        let mut output =
            Vec::with_capacity((self.histogram_width * self.histogram_height * 4) as usize);
        for row in 0..self.histogram_height {
            let start = (row * padded_bytes_per_row) as usize;
            let end = start + unpadded_bytes_per_row as usize;
            output.extend_from_slice(&data[start..end]);
        }

        drop(data);
        output_buffer.unmap();
        output
    }

    /// Phase 21: Calculate RGB histogram from rendered RGBA bytes
    /// Returns [R[256], G[256], B[256]] histogram data
    pub fn calculate_histogram(&self, rgba_bytes: &[u8]) -> [[u32; 256]; 3] {
        let mut histograms = [[0u32; 256]; 3];

        // Process pixels in chunks of 4 (RGBA)
        for pixel in rgba_bytes.chunks_exact(4) {
            let r = pixel[0] as usize;
            let g = pixel[1] as usize;
            let b = pixel[2] as usize;
            // pixel[3] is alpha, ignore it

            histograms[0][r] += 1; // Red channel
            histograms[1][g] += 1; // Green channel
            histograms[2][b] += 1; // Blue channel
        }

        histograms
    }
}
