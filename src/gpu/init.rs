use iced_wgpu::wgpu;
use wgpu::util::DeviceExt;

use crate::core::types::EditParams;
use super::params::GpuEditParams;
use super::pipeline::RenderPipeline;

impl RenderPipeline {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        image_id: i64,
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
        const MAX_PREVIEW_WIDTH: u32 = 1280;
        let aspect_ratio = width as f32 / height as f32;
        let preview_width = width.min(MAX_PREVIEW_WIDTH);
        let preview_height = (preview_width as f32 / aspect_ratio) as u32;

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
            format: wgpu::TextureFormat::R16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let bytes_per_pixel = 2;
        let unpadded_bytes_per_row = width * bytes_per_pixel;
        let padded_bytes_per_row = (unpadded_bytes_per_row + 255) & !255;
        let raw_bytes = bytemuck::cast_slice(&raw_data);

        if unpadded_bytes_per_row == padded_bytes_per_row {
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
            tracing::debug!(
                "Uploading RAW u16 data with padding (Row: {} -> {})",
                unpadded_bytes_per_row,
                padded_bytes_per_row
            );
            let mut padded_data =
                Vec::with_capacity((padded_bytes_per_row * height) as usize);
            for y in 0..height {
                let start = (y * unpadded_bytes_per_row) as usize;
                let end = start + unpadded_bytes_per_row as usize;
                padded_data.extend_from_slice(&raw_bytes[start..end]);
                padded_data.extend(std::iter::repeat_n(
                    0,
                    (padded_bytes_per_row - unpadded_bytes_per_row) as usize,
                ));
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

        let mut gpu_params: GpuEditParams = params.into();
        gpu_params.wb_multipliers = wb_multipliers;
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

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Bind Group Layout"),
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        // Placeholder 3D HSV LUT (1×1×1, identity — real LUT uploaded when DCP profile loads)
        let lut_placeholder = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("HSV LUT Placeholder"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let lut_view = lut_placeholder.create_view(&wgpu::TextureViewDescriptor::default());

        // Placeholder 1D tone curve (1-pixel linear passthrough)
        let curve_placeholder = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Tone Curve Placeholder"),
            size: wgpu::Extent3d { width: 2, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D1,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let curve_view = curve_placeholder.create_view(&wgpu::TextureViewDescriptor::default());

        let lut_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("DCP LUT Sampler Placeholder"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&lut_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&curve_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&lut_sampler),
                },
            ],
        });

        let shader_source = super::shaders::get_shader();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RAW Processing Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Pipeline Layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

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
                cull_mode: None,
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
            image_id,
            histogram_width,
            histogram_height,
            wb_multipliers,
            forward_matrix,
            has_dcp: false,
            cfa_pattern,
            black_levels,
            white_level,
            current_params: std::sync::Mutex::new(gpu_params),
        })
    }
}
