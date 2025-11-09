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
use crate::state::edit::EditParams;

/// Represents the edit parameters in a GPU-friendly format
/// Must match the WGSL struct layout with proper alignment
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuEditParams {
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
    padding1: f32,  // For 16-byte alignment
    padding2: f32,
    // Phase 14: Color science (must match WGSL layout!)
    wb_multipliers: [f32; 4],   // White balance [R, G, B, G2] - vec4 in WGSL
    // Color matrix split into 3 rows with padding (WGSL vec3 = 12 bytes + 4 padding)
    color_matrix_0: [f32; 3],   // Row 0
    _padding3: f32,
    color_matrix_1: [f32; 3],   // Row 1
    _padding4: f32,
    color_matrix_2: [f32; 3],   // Row 2
    _padding5: f32,
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
            temperature: params.temperature as f32,
            tint: params.tint as f32,
            padding1: 0.0,
            padding2: 0.0,
            // Default values (will be overwritten by set_color_metadata)
            wb_multipliers: [1.0, 1.0, 1.0, 1.0],
            color_matrix_0: [1.0, 0.0, 0.0],
            _padding3: 0.0,
            color_matrix_1: [0.0, 1.0, 0.0],
            _padding4: 0.0,
            color_matrix_2: [0.0, 0.0, 1.0],
            _padding5: 0.0,
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
    pub width: u32,           // Full resolution width
    pub height: u32,          // Full resolution height
    pub preview_width: u32,   // Preview resolution width (for fast rendering)
    pub preview_height: u32,  // Preview resolution height (for fast rendering)
    pub image_id: i64,        // Phase 20: Track which image this pipeline is for
    // Phase 14: Color science metadata
    wb_multipliers: [f32; 4],  // White balance from camera
    color_matrix: [f32; 9],    // Color correction matrix
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
    pub async fn new(
        image_id: i64,        // Phase 20: Track which image this pipeline is for
        raw_data: Vec<u16>,
        width: u32,
        height: u32,
        params: &EditParams,
        wb_multipliers: [f32; 4],
        color_matrix: [f32; 9],
    ) -> Result<Self, String> {
        // Calculate preview dimensions for fast rendering
        // Phase 13: Render to smaller texture to eliminate 1-2s lag
        const MAX_PREVIEW_WIDTH: u32 = 2560;
        let aspect_ratio = width as f32 / height as f32;
        let preview_width = width.min(MAX_PREVIEW_WIDTH);
        let preview_height = (preview_width as f32 / aspect_ratio) as u32;
        
        println!("📐 Full resolution: {}x{}", width, height);
        println!("📐 Preview resolution: {}x{} ({:.1}% of full)", 
            preview_width, preview_height,
            (preview_width * preview_height) as f32 / (width * height) as f32 * 100.0);
        
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
            format: wgpu::TextureFormat::R16Uint,  // 16-bit unsigned integer for RAW data
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        
        // Upload RAW u16 data directly (no conversion!)
        let raw_bytes = bytemuck::cast_slice(&raw_data);
        println!("💾 Uploading {} bytes of RAW u16 data to GPU", raw_bytes.len());
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
                bytes_per_row: Some(2 * width),  // 2 bytes per pixel (u16)
                rows_per_image: Some(height),
            },
            texture_size,
        );
        println!("✅ RAW texture uploaded to GPU!");
        
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
        // Split flat color_matrix [9] into 3 rows with padding
        gpu_params.color_matrix_0 = [color_matrix[0], color_matrix[1], color_matrix[2]];
        gpu_params.color_matrix_1 = [color_matrix[3], color_matrix[4], color_matrix[5]];
        gpu_params.color_matrix_2 = [color_matrix[6], color_matrix[7], color_matrix[8]];
        
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
                        sample_type: wgpu::TextureSampleType::Uint,  // Integer texture for RAW u16
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
            image_id,          // Phase 20: Track which image this pipeline is for
            wb_multipliers,
            color_matrix,
        })
    }
    
    /// Update uniform buffer with new edit parameters
    pub fn update_uniforms(&self, params: &EditParams) {
        let mut gpu_params = GpuEditParams::from(params);
        // Preserve color metadata (doesn't change with slider updates)
        gpu_params.wb_multipliers = self.wb_multipliers;
        // Convert flat matrix to split rows
        let cm = &self.color_matrix;
        gpu_params.color_matrix_0 = [cm[0], cm[1], cm[2]];
        gpu_params.color_matrix_1 = [cm[3], cm[4], cm[5]];
        gpu_params.color_matrix_2 = [cm[6], cm[7], cm[8]];
        
        println!("🎨 GPU Uniforms Updated:");
        println!("   Exposure: {:.2}, Contrast: {:.0}", gpu_params.exposure, gpu_params.contrast);
        println!("   Highlights: {:.0}, Shadows: {:.0}", gpu_params.highlights, gpu_params.shadows);
        println!("   Temp: {}, Tint: {}", gpu_params.temperature, gpu_params.tint);
        
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[gpu_params]),
        );
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
        render_pass.set_viewport(
            0.0,
            0.0,
            viewport.0 as f32,
            viewport.1 as f32,
            0.0,
            1.0,
        );
        
        // Execute our shader
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        render_pass.draw(0..3, 0..1); // Full-screen triangle
    }
    
    /// Phase 13: Render to preview resolution for fast updates
    /// Renders full RAW texture to smaller output (GPU downsamples automatically)
    pub fn render_to_bytes(&self) -> Vec<u8> {
        // Create PREVIEW-SIZED output texture (Phase 13 optimization!)
        let output_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Output Texture (Preview)"),
            size: wgpu::Extent3d {
                width: self.preview_width,   // Preview size, not full!
                height: self.preview_height,  // Preview size, not full!
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
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });
        
        // Render to PREVIEW texture (GPU rasterizer auto-downsamples from full res input)
        self.render_to_target(&mut encoder, &output_view, (self.preview_width, self.preview_height));
        
        // Readback from PREVIEW buffer (much smaller!)
        let bytes_per_row = self.preview_width * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        let buffer_size = (padded_bytes_per_row * self.preview_height) as u64;
        
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
                    rows_per_image: Some(self.preview_height),  // Preview, not full!
                },
            },
            wgpu::Extent3d {
                width: self.preview_width,   // Preview, not full!
                height: self.preview_height,  // Preview, not full!
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
        rx.recv().unwrap().unwrap();
        
        let data = buffer_slice.get_mapped_range();
        let mut output = Vec::with_capacity((self.preview_width * self.preview_height * 4) as usize);
        for y in 0..self.preview_height {  // Preview, not full!
            let start = (y * padded_bytes_per_row) as usize;
            let end = start + (self.preview_width * 4) as usize;  // Preview, not full!
            output.extend_from_slice(&data[start..end]);
        }
        
        drop(data);
        output_buffer.unmap();
        output
    }
    
    /// Phase 19: Render to FULL resolution for export
    /// This is SLOW (1-2 seconds for 24MP) - only use for final export!
    pub fn render_full_res_to_bytes(&self) -> Vec<u8> {
        // Create FULL-SIZED output texture (all 24 megapixels!)
        let output_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Output Texture (Full Resolution)"),
            size: wgpu::Extent3d {
                width: self.width,   // FULL resolution!
                height: self.height,  // FULL resolution!
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
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder (Full Res)"),
        });
        
        // Render to FULL resolution texture
        self.render_to_target(&mut encoder, &output_view, (self.width, self.height));
        
        // Readback from FULL buffer (LARGE! ~96MB for 24MP)
        let bytes_per_row = self.width * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        let buffer_size = (padded_bytes_per_row * self.height) as u64;
        
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
                    rows_per_image: Some(self.height),  // FULL resolution!
                },
            },
            wgpu::Extent3d {
                width: self.width,   // FULL resolution!
                height: self.height,  // FULL resolution!
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
        rx.recv().unwrap().unwrap();
        
        let data = buffer_slice.get_mapped_range();
        let mut output = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height {  // FULL resolution!
            let start = (y * padded_bytes_per_row) as usize;
            let end = start + (self.width * 4) as usize;  // FULL resolution!
            output.extend_from_slice(&data[start..end]);
        }
        
        drop(data);
        output_buffer.unmap();
        output
    }
    
    /// Get the texture dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
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
