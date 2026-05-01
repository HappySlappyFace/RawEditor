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
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
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

        // Create sampler (NonFiltering — both passes use textureLoad, not textureSample)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("RAW Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
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
    pub preview_width: u32,
    pub preview_height: u32,
    pub histogram_width: u32,
    pub histogram_height: u32,
    pub image_id: i64,
    // Metadata
    pub wb_multipliers: [f32; 4],
    pub color_matrix: [f32; 9],
    pub cfa_pattern: u32,
    pub black_levels: [u32; 4],
    pub white_level: u32,
    pub current_params: std::sync::Mutex<GpuEditParams>,
}

impl ImageResources {
    /// Create new image resources for a specific image
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: &SharedContext,
        image_id: i64,
        raw_data: Vec<u16>,
        width: u32,
        height: u32,
        params: &EditParams,
        wb_multipliers: [f32; 4],
        color_matrix: [f32; 9],
        cfa_pattern: u32,
        black_levels: [u32; 4],
        white_level: u32,
    ) -> Result<Self, String> {
        // Calculate preview dimensions
        const MAX_PREVIEW_WIDTH: u32 = 1280;
        let aspect_ratio = width as f32 / height as f32;
        let preview_width = width.min(MAX_PREVIEW_WIDTH);
        let preview_height = (preview_width as f32 / aspect_ratio) as u32;

        // Calculate histogram dimensions
        const HISTOGRAM_WIDTH: u32 = 128;
        let histogram_width = HISTOGRAM_WIDTH;
        let histogram_height = (histogram_width as f32 / aspect_ratio) as u32;

        tracing::debug!(
            "Image {}x{}, Preview {}x{}, Histogram {}x{}",
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

        let raw_bytes = bytemuck::cast_slice(&raw_data);

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

        // Create uniform buffer with initial params
        let mut gpu_params = GpuEditParams::from(params);
        gpu_params.wb_multipliers = wb_multipliers;
        gpu_params.color_matrix_0 = [color_matrix[0], color_matrix[1], color_matrix[2]];
        gpu_params.color_matrix_1 = [color_matrix[3], color_matrix[4], color_matrix[5]];
        gpu_params.color_matrix_2 = [color_matrix[6], color_matrix[7], color_matrix[8]];
        gpu_params.cfa_pattern = cfa_pattern;
        gpu_params.black_levels = black_levels;
        gpu_params.white_level = white_level;

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
                        resource: wgpu::BindingResource::Sampler(&context.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform_buffer.as_entire_binding(),
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
            preview_width,
            preview_height,
            histogram_width,
            histogram_height,
            image_id,
            wb_multipliers,
            color_matrix,
            cfa_pattern,
            black_levels,
            white_level,
            current_params: std::sync::Mutex::new(gpu_params),
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

    /// Update uniforms with new edit parameters
    pub fn update_uniforms(&self, context: &SharedContext, params: &EditParams) {
        let mut gpu_params = GpuEditParams::from(params);
        gpu_params.wb_multipliers = self.wb_multipliers;
        gpu_params.color_matrix_0 = [
            self.color_matrix[0],
            self.color_matrix[1],
            self.color_matrix[2],
        ];
        gpu_params.color_matrix_1 = [
            self.color_matrix[3],
            self.color_matrix[4],
            self.color_matrix[5],
        ];
        gpu_params.color_matrix_2 = [
            self.color_matrix[6],
            self.color_matrix[7],
            self.color_matrix[8],
        ];
        gpu_params.cfa_pattern = self.cfa_pattern;
        gpu_params.black_levels = self.black_levels;
        gpu_params.white_level = self.white_level;

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
        gpu_params.wb_multipliers = self.wb_multipliers;
        gpu_params.color_matrix_0 = [
            self.color_matrix[0],
            self.color_matrix[1],
            self.color_matrix[2],
        ];
        gpu_params.color_matrix_1 = [
            self.color_matrix[3],
            self.color_matrix[4],
            self.color_matrix[5],
        ];
        gpu_params.color_matrix_2 = [
            self.color_matrix[6],
            self.color_matrix[7],
            self.color_matrix[8],
        ];
        gpu_params.cfa_pattern = self.cfa_pattern;
        gpu_params.black_levels = self.black_levels;
        gpu_params.white_level = self.white_level;
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
