/// Phase 95: Shared GPU Context and Per-Image Resources
/// 
/// This module contains the refactored pipeline architecture:
/// - SharedContext: Persistent GPU resources (created once)
/// - ImageResources: Per-image data (created for each image)

use iced_wgpu::wgpu;
use wgpu::util::DeviceExt;
use std::sync::Arc;

use crate::gpu::shaders;
use super::pipeline::GpuEditParams;
use crate::core::types::EditParams;

/// Shared GPU context (created once, reused for all images)
/// Contains all the persistent GPU resources that don't change between images
#[derive(Debug)]
pub struct SharedContext {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
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
        
        // Load shaders
        let shader_source = shaders::get_shader();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RAW Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
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
                        sample_type: wgpu::TextureSampleType::Uint,
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
        
        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RAW Pipeline Layout"),
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
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        
        // Create sampler
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("RAW Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        
        println!("✅ SharedContext initialized successfully");
        
        Ok(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
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
        
        println!("📐 Image {}x{}, Preview {}x{}, Histogram {}x{}", 
            width, height, preview_width, preview_height, histogram_width, histogram_height);
        
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
                padded_data.extend(std::iter::repeat(0).take((padded_bytes_per_row - unpadded_bytes_per_row) as usize));
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
        
        // Create uniform buffer with initial params
        let mut gpu_params = GpuEditParams::from(params);
        gpu_params.wb_multipliers = wb_multipliers;
        gpu_params.color_matrix_0 = [color_matrix[0], color_matrix[1], color_matrix[2]];
        gpu_params.color_matrix_1 = [color_matrix[3], color_matrix[4], color_matrix[5]];
        gpu_params.color_matrix_2 = [color_matrix[6], color_matrix[7], color_matrix[8]];
        gpu_params.cfa_pattern = cfa_pattern;
        gpu_params.black_levels = black_levels;
        gpu_params.white_level = white_level;
        
        let uniform_buffer = context.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Uniform Buffer"),
            contents: bytemuck::cast_slice(&[gpu_params]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        
        // Create bind group
        let bind_group = context.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bind Group"),
            layout: &context.bind_group_layout,
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
        
        Ok(Self {
            texture,
            texture_view,
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
        })
    }
    
    /// Update uniforms with new edit parameters
    pub fn update_uniforms(&self, context: &SharedContext, params: &EditParams) {
        let mut gpu_params = GpuEditParams::from(params);
        gpu_params.wb_multipliers = self.wb_multipliers;
        gpu_params.color_matrix_0 = [self.color_matrix[0], self.color_matrix[1], self.color_matrix[2]];
        gpu_params.color_matrix_1 = [self.color_matrix[3], self.color_matrix[4], self.color_matrix[5]];
        gpu_params.color_matrix_2 = [self.color_matrix[6], self.color_matrix[7], self.color_matrix[8]];
        gpu_params.cfa_pattern = self.cfa_pattern;
        gpu_params.black_levels = self.black_levels;
        gpu_params.white_level = self.white_level;
        
        context.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[gpu_params]));
        
        if let Ok(mut current) = self.current_params.lock() {
            *current = gpu_params;
        }
    }
    
    /// Update uniforms with zoom and pan
    pub fn update_uniforms_with_zoom(&self, context: &SharedContext, params: &EditParams, zoom: f32, pan_x: f32, pan_y: f32) {
        let mut gpu_params = GpuEditParams::from(params);
        gpu_params.wb_multipliers = self.wb_multipliers;
        gpu_params.color_matrix_0 = [self.color_matrix[0], self.color_matrix[1], self.color_matrix[2]];
        gpu_params.color_matrix_1 = [self.color_matrix[3], self.color_matrix[4], self.color_matrix[5]];
        gpu_params.color_matrix_2 = [self.color_matrix[6], self.color_matrix[7], self.color_matrix[8]];
        gpu_params.cfa_pattern = self.cfa_pattern;
        gpu_params.black_levels = self.black_levels;
        gpu_params.white_level = self.white_level;
        gpu_params.zoom = zoom;
        gpu_params.pan_x = pan_x;
        gpu_params.pan_y = pan_y;
        
        context.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[gpu_params]));
        
        if let Ok(mut current) = self.current_params.lock() {
            *current = gpu_params;
        }
    }
}
