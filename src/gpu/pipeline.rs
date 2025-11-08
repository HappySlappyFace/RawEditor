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
    pub width: u32,
    pub height: u32,
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
        raw_data: Vec<u16>,
        width: u32,
        height: u32,
        params: &EditParams,
    ) -> Result<Self, String> {
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
        
        // Convert u16 RAW data to f32 RGBA (simple passthrough for now)
        let rgba_data = Self::convert_raw_to_rgba(&raw_data, width, height);
        
        // Create texture
        let texture_size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("RAW Input Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        
        // Upload texture data
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            texture_size,
        );
        
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
        
        // Create uniform buffer
        let gpu_params: GpuEditParams = params.into();
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Edit Params Uniform Buffer"),
            contents: bytemuck::cast_slice(&[gpu_params]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        
        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Bind Group Layout"),
            entries: &[
                // Texture
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
                cull_mode: Some(wgpu::Face::Back),
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
        })
    }
    
    /// Update uniform buffer with new edit parameters
    pub fn update_uniforms(&self, params: &EditParams) {
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[GpuEditParams::from(params)]),
        );
    }
    
    // TODO: Implement render_to_texture() to output RGBA8 buffer
    // This will be used to create an iced::Image for display
    
    /// Convert u16 RAW sensor data to RGBA8 (simple grayscale for now)
    fn convert_raw_to_rgba(raw_data: &[u16], width: u32, height: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        
        for &pixel in raw_data.iter() {
            // Normalize u16 (0-65535) to u8 (0-255)
            let value = (pixel >> 8) as u8;
            rgba.push(value); // R
            rgba.push(value); // G
            rgba.push(value); // B
            rgba.push(255);   // A
        }
        
        rgba
    }
    
    /// Get the texture dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}
