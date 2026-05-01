// Phase 115: Dual-layer preview architecture.
// - ViewportProgram implements iced::widget::shader::Program for the image (GPU native, respects scissor rects).
// - CropOverlay implements iced::widget::canvas::Program for the crop UI (transparent overlay on top).

use iced::widget::canvas::{self};
use iced::mouse::Cursor;
use iced::{Rectangle, Color};

use iced::widget::shader;
use iced::widget::shader::wgpu;
use wgpu::util::DeviceExt;

use crate::app::message::Message;

// ─────────────────────────────── CropHandle ────────────────────────────────

/// Phase 67: Crop handles
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CropHandle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// Phase 70: Dragging the crop area itself
    Body,
}

// ─────────────────────────────── ViewportPrimitive ────────────────────────

/// Data passed from ViewportProgram::draw() to the GPU prepare/render calls.
#[derive(Debug)]
pub struct ViewportPrimitive {
    /// RGBA pixels, width × height
    pub pixels: std::sync::Arc<[u8]>,
    pub width: u32,
    pub height: u32,
    /// Zoom level (1.0 = fit-to-screen)
    pub zoom: f32,
    /// Pan offset (normalised, same coordinate space as shaders)
    pub pan_x: f32,
    pub pan_y: f32,
    /// Background colour (displayed behind the image)
    pub background: Color,
}

/// Cached GPU state kept in iced's Storage between frames.
struct ViewportPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    // Cached texture identity – rebuilt whenever pixel dimensions change.
    texture: Option<wgpu::Texture>,
    texture_view: Option<wgpu::TextureView>,
    texture_width: u32,
    texture_height: u32,
    // Uniform buffer: [zoom, pan_x, pan_y, aspect_px (image_w/image_h), 4 background floats]
    uniform_buf: wgpu::Buffer,
    bind_group: Option<wgpu::BindGroup>,
    // Vertex buffer: 6 vertices × (2 pos + 2 uv) floats
    vertex_buf: wgpu::Buffer,
}

/// Per-frame uniforms matching the WGSL `Uniforms` struct exactly (std140 / 16-byte aligned).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// Orthographic projection matrix for centering and aspect-ratio fit
    proj: [[f32; 4]; 4],
    zoom:      f32,
    pan_x:     f32,
    pan_y:     f32,
    /// image width / image height
    img_aspect: f32,
    /// background RGBA (linear)
    bg: [f32; 4],
}

use cgmath::Matrix4;

impl shader::Primitive for ViewportPrimitive {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        storage: &mut shader::Storage,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let _scale_factor = viewport.scale_factor();
        
        // First call: create the pipeline and companion resources.
        if !storage.has::<ViewportPipeline>() {
            let shader_src = wgpu::include_wgsl!("viewport.wgsl");
            let shader_module = device.create_shader_module(shader_src);

            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("viewport_bgl"),
                entries: &[
                    // binding 0 – uniforms
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 1 – texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // binding 2 – sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("viewport_layout"),
                bind_group_layouts: &[&bgl],
                push_constant_ranges: &[],
            });

            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("viewport_pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader_module,
                    entry_point: "vs_main",
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: (4 * std::mem::size_of::<f32>()) as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    }],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader_module,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
            });

            let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("viewport_uniforms"),
                size: std::mem::size_of::<Uniforms>() as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            // Full-screen quad: two triangles covering clip-space.
            // (pos_x, pos_y, uv_x, uv_y)
            let quad: &[f32] = &[
                -1.0, -1.0,   0.0, 1.0,
                 1.0, -1.0,   1.0, 1.0,
                -1.0,  1.0,   0.0, 0.0,
                -1.0,  1.0,   0.0, 0.0,
                 1.0, -1.0,   1.0, 1.0,
                 1.0,  1.0,   1.0, 0.0,
            ];
            let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("viewport_verts"),
                contents: bytemuck::cast_slice(quad),
                usage: wgpu::BufferUsages::VERTEX,
            });

            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });

            storage.store(ViewportPipeline {
                pipeline,
                bind_group_layout: bgl,
                sampler,
                texture: None,
                texture_view: None,
                texture_width: 0,
                texture_height: 0,
                uniform_buf,
                bind_group: None,
                vertex_buf,
            });
        }

        let state = storage.get_mut::<ViewportPipeline>().unwrap();

        // Rebuild texture if resolution changed.
        if self.width != state.texture_width || self.height != state.texture_height || state.texture.is_none() {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("viewport_tex"),
                size: wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("viewport_bg"),
                layout: &state.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: state.uniform_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&state.sampler) },
                ],
            });

            state.texture_width = self.width;
            state.texture_height = self.height;
            state.texture_view = Some(view);
            state.texture = Some(tex);
            state.bind_group = Some(bind_group);
        }

        // Upload pixels every frame.
        if let Some(tex) = &state.texture {
            queue.write_texture(
                tex.as_image_copy(),
                &self.pixels,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * self.width),
                    rows_per_image: None,
                },
                wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
            );
        }

        // --- PHASE 134: Aspect-Ratio Centering & Projection Matrix ---
        let img_aspect = self.width as f32 / self.height as f32;
        let vp_aspect = bounds.width / bounds.height;
        
        let (fit_w, fit_h) = if img_aspect > vp_aspect {
            (1.0, 1.0 / (img_aspect / vp_aspect))
        } else {
            (img_aspect / vp_aspect, 1.0)
        };

        // Orthographic matrix that maps [-1, 1] to the fitted aspect ratio
        let matrix = Matrix4::from_nonuniform_scale(fit_w, fit_h, 1.0);
        
        let uniforms = Uniforms {
            proj: matrix.into(),
            zoom: self.zoom,
            pan_x: self.pan_x,
            pan_y: self.pan_y,
            img_aspect,
            bg: [self.background.r, self.background.g, self.background.b, self.background.a],
        };
        queue.write_buffer(&state.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        storage: &shader::Storage,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        if let Some(state) = storage.get::<ViewportPipeline>() {
            if let Some(bind_group) = &state.bind_group {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("viewport_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                // Set viewport and scissor to the physical bounds of the widget
                rpass.set_viewport(
                    clip_bounds.x as f32,
                    clip_bounds.y as f32,
                    clip_bounds.width as f32,
                    clip_bounds.height as f32,
                    0.0,
                    1.0,
                );
                rpass.set_scissor_rect(clip_bounds.x, clip_bounds.y, clip_bounds.width, clip_bounds.height);
                
                rpass.set_pipeline(&state.pipeline);
                rpass.set_bind_group(0, bind_group, &[]);
                rpass.set_vertex_buffer(0, state.vertex_buf.slice(..));
                rpass.draw(0..6, 0..1);
            }
        }
    }
}

// ─────────────────────────────── ViewportProgram ───────────────────────────

/// Phase 115: The native GPU image viewer.
/// Implements `iced::widget::shader::Program` – the scissor rect is respected natively.
pub struct ViewportProgram {
    pub pixels: Option<std::sync::Arc<[u8]>>,
    pub image_width: u32,
    pub image_height: u32,
    pub zoom: f32,
    pub offset: cgmath::Vector2<f32>,
    pub background_color: Color,
}

impl shader::Program<Message> for ViewportProgram {
    type State = ();
    type Primitive = ViewportPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: iced::mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        // Mirror the pan math from the old canvas renderer so zoom/pan feel identical.
        let image_aspect = self.image_width as f32 / self.image_height.max(1) as f32;
        let viewport_aspect = bounds.width / bounds.height;
        let (fitted_w, _fitted_h) = if image_aspect > viewport_aspect {
            (bounds.width, bounds.width / image_aspect)
        } else {
            (bounds.height * image_aspect, bounds.height)
        };
        // pan_x / pan_y in normalised image-space (matches GPU shader coordinate system).
        // Phase 116: Normalise pixels to [0, 1] for the GPU shader.
        // We divide pixels by viewport width to get 0-1 range.
        let pan_x = (self.offset.x * fitted_w) / bounds.width;
        let pan_y = (self.offset.y * fitted_w / image_aspect) / bounds.height;

        // Placeholder 1×1 white pixel if no image is loaded yet.
        let (pixels, w, h) = match &self.pixels {
            Some(p) if !p.is_empty() => (p.clone(), self.image_width, self.image_height),
            _ => {
                let white: std::sync::Arc<[u8]> = std::sync::Arc::from([255u8, 255, 255, 255].as_ref());
                (white, 1, 1)
            }
        };

        ViewportPrimitive {
            pixels,
            width: w,
            height: h,
            zoom: self.zoom,
            pan_x,
            pan_y,
            background: self.background_color,
        }
    }
}

// ─────────────────────────────── CropOverlay ───────────────────────────────

/// Transparent Canvas overlay drawn on top of ViewportProgram.
/// Only draws crop UI elements (dim mask, rule-of-thirds, handles).
pub struct CropOverlay {
    pub zoom: f32,
    pub offset: cgmath::Vector2<f32>,
    pub image_width: u32,
    pub image_height: u32,
    pub is_cropping: bool,
    pub crop: [f32; 4],
}

impl canvas::Program<Message> for CropOverlay {
    type State = (f32, f32, f32); // Phase 116: Track (width, height, scale_factor) for resize sensing

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<canvas::Geometry> {
        if !self.is_cropping {
            return vec![];
        }

        let viewport_width = bounds.width;
        let viewport_height = bounds.height;
        let image_aspect = self.image_width as f32 / self.image_height.max(1) as f32;
        let viewport_aspect = viewport_width / viewport_height;

        let (fitted_width, fitted_height) = if image_aspect > viewport_aspect {
            let w = viewport_width;
            let h = w / image_aspect;
            (w, h)
        } else {
            let h = viewport_height;
            let w = h * image_aspect;
            (w, h)
        };

        let center_x = viewport_width / 2.0;
        let center_y = viewport_height / 2.0;
        let zoomed_width = fitted_width * self.zoom;
        let zoomed_height = fitted_height * self.zoom;
        let pan_px_x = self.offset.x * fitted_width;
        let pan_px_y = self.offset.y * fitted_height;
        let image_x = center_x - (zoomed_width / 2.0) + pan_px_x;
        let image_y = center_y - (zoomed_height / 2.0) + pan_px_y;

        let image_bounds = Rectangle {
            x: image_x,
            y: image_y,
            width: zoomed_width,
            height: zoomed_height,
        };

        let crop_x = image_bounds.x + (self.crop[0] * image_bounds.width);
        let crop_y = image_bounds.y + (self.crop[1] * image_bounds.height);
        let crop_w = self.crop[2] * image_bounds.width;
        let crop_h = self.crop[3] * image_bounds.height;

        let mut overlay_frame = canvas::Frame::new(renderer, bounds.size());

        // Dim outer regions
        let dim = Color::from_rgba(0.0, 0.0, 0.0, 0.7);
        overlay_frame.fill_rectangle(iced::Point::new(image_bounds.x, image_bounds.y), iced::Size::new(image_bounds.width, crop_y - image_bounds.y), dim);
        overlay_frame.fill_rectangle(iced::Point::new(image_bounds.x, crop_y + crop_h), iced::Size::new(image_bounds.width, (image_bounds.y + image_bounds.height) - (crop_y + crop_h)), dim);
        overlay_frame.fill_rectangle(iced::Point::new(image_bounds.x, crop_y), iced::Size::new(crop_x - image_bounds.x, crop_h), dim);
        overlay_frame.fill_rectangle(iced::Point::new(crop_x + crop_w, crop_y), iced::Size::new((image_bounds.x + image_bounds.width) - (crop_x + crop_w), crop_h), dim);

        // Rule of thirds grid
        let third_w = crop_w / 3.0;
        let third_h = crop_h / 3.0;
        let grid_stroke = canvas::Stroke::default().with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.8)).with_width(1.0);
        overlay_frame.stroke(&canvas::Path::line(iced::Point::new(crop_x + third_w, crop_y), iced::Point::new(crop_x + third_w, crop_y + crop_h)), grid_stroke);
        overlay_frame.stroke(&canvas::Path::line(iced::Point::new(crop_x + third_w * 2.0, crop_y), iced::Point::new(crop_x + third_w * 2.0, crop_y + crop_h)), grid_stroke);
        overlay_frame.stroke(&canvas::Path::line(iced::Point::new(crop_x, crop_y + third_h), iced::Point::new(crop_x + crop_w, crop_y + third_h)), grid_stroke);
        overlay_frame.stroke(&canvas::Path::line(iced::Point::new(crop_x, crop_y + third_h * 2.0), iced::Point::new(crop_x + crop_w, crop_y + third_h * 2.0)), grid_stroke);

        // Crop border
        let border_rect = Rectangle { x: crop_x, y: crop_y, width: crop_w, height: crop_h };
        overlay_frame.stroke(&canvas::Path::rectangle(border_rect.position(), border_rect.size()), canvas::Stroke::default().with_color(Color::WHITE).with_width(2.0));

        // Corner handles
        let handle_size = 12.0;
        let draw_handle = |frame: &mut canvas::Frame, x: f32, y: f32| {
            let pos = iced::Point::new(x - handle_size / 2.0, y - handle_size / 2.0);
            let size = iced::Size::new(handle_size, handle_size);
            frame.fill_rectangle(pos, size, Color::WHITE);
            frame.stroke(&canvas::Path::rectangle(pos, size), canvas::Stroke::default().with_color(Color::BLACK).with_width(1.0));
        };
        draw_handle(&mut overlay_frame, crop_x, crop_y);
        draw_handle(&mut overlay_frame, crop_x + crop_w, crop_y);
        draw_handle(&mut overlay_frame, crop_x, crop_y + crop_h);
        draw_handle(&mut overlay_frame, crop_x + crop_w, crop_y + crop_h);

        vec![overlay_frame.into_geometry()]
    }

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        // Detect resize or DPI change
        let scale_factor = 1.0; 
        
        if state.0 != bounds.width || state.1 != bounds.height || (state.2 - scale_factor).abs() > 0.001 {
            *state = (bounds.width, bounds.height, scale_factor);
            return (canvas::event::Status::Captured, Some(Message::ViewportResized(bounds.width, bounds.height, scale_factor)));
        }

        if !self.is_cropping {
            return (canvas::event::Status::Ignored, None);
        }

        let cursor_position = match cursor.position_in(bounds) {
            Some(p) => p,
            None => return (canvas::event::Status::Ignored, None),
        };

        if let canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) = event {
            let viewport_width = bounds.width;
            let viewport_height = bounds.height;
            let image_aspect = self.image_width as f32 / self.image_height.max(1) as f32;
            let viewport_aspect = viewport_width / viewport_height;
            let (fitted_width, fitted_height) = if image_aspect > viewport_aspect {
                (viewport_width, viewport_width / image_aspect)
            } else {
                (viewport_height * image_aspect, viewport_height)
            };
            let center_x = viewport_width / 2.0;
            let center_y = viewport_height / 2.0;
            let zoomed_width = fitted_width * self.zoom;
            let zoomed_height = fitted_height * self.zoom;
            let pan_px_x = self.offset.x * fitted_width;
            let pan_px_y = self.offset.y * fitted_height;
            let image_x = center_x - (zoomed_width / 2.0) + pan_px_x;
            let image_y = center_y - (zoomed_height / 2.0) + pan_px_y;

            let image_bounds = Rectangle { x: image_x, y: image_y, width: zoomed_width, height: zoomed_height };
            let crop_x = image_bounds.x + (self.crop[0] * image_bounds.width);
            let crop_y = image_bounds.y + (self.crop[1] * image_bounds.height);
            let crop_w = self.crop[2] * image_bounds.width;
            let crop_h = self.crop[3] * image_bounds.height;
            let hr = 15.0_f32;
            let chk = |x: f32, y: f32| -> bool {
                let dx = cursor_position.x - x;
                let dy = cursor_position.y - y;
                dx * dx + dy * dy < hr * hr
            };

            if chk(crop_x, crop_y) { return (canvas::event::Status::Captured, Some(Message::CropHandleGrabbed(CropHandle::TopLeft, image_bounds))); }
            if chk(crop_x + crop_w, crop_y) { return (canvas::event::Status::Captured, Some(Message::CropHandleGrabbed(CropHandle::TopRight, image_bounds))); }
            if chk(crop_x, crop_y + crop_h) { return (canvas::event::Status::Captured, Some(Message::CropHandleGrabbed(CropHandle::BottomLeft, image_bounds))); }
            if chk(crop_x + crop_w, crop_y + crop_h) { return (canvas::event::Status::Captured, Some(Message::CropHandleGrabbed(CropHandle::BottomRight, image_bounds))); }

            if cursor_position.x >= crop_x && cursor_position.x <= crop_x + crop_w
                && cursor_position.y >= crop_y && cursor_position.y <= crop_y + crop_h {
                return (canvas::event::Status::Captured, Some(Message::CropHandleGrabbed(CropHandle::Body, image_bounds)));
            }
        }

        (canvas::event::Status::Ignored, None)
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> iced::mouse::Interaction {
        if !self.is_cropping { return iced::mouse::Interaction::default(); }
        let cursor_position = match cursor.position_in(bounds) {
            Some(p) => p,
            None => return iced::mouse::Interaction::default(),
        };

        let image_aspect = self.image_width as f32 / self.image_height.max(1) as f32;
        let vp_aspect = bounds.width / bounds.height;
        let (fw, fh) = if image_aspect > vp_aspect { (bounds.width, bounds.width / image_aspect) } else { (bounds.height * image_aspect, bounds.height) };
        let cx = bounds.width / 2.0;
        let cy = bounds.height / 2.0;
        let zw = fw * self.zoom;
        let zh = fh * self.zoom;
        let px_x = self.offset.x * fw;
        let px_y = self.offset.y * fh;
        let image_bounds = Rectangle { x: cx - zw / 2.0 + px_x, y: cy - zh / 2.0 + px_y, width: zw, height: zh };

        let crop_x = image_bounds.x + (self.crop[0] * image_bounds.width);
        let crop_y = image_bounds.y + (self.crop[1] * image_bounds.height);
        let crop_w = self.crop[2] * image_bounds.width;
        let crop_h = self.crop[3] * image_bounds.height;
        let hr = 15.0_f32;
        let chk = |x: f32, y: f32| -> bool { let dx = cursor_position.x - x; let dy = cursor_position.y - y; dx*dx+dy*dy < hr*hr };

        if chk(crop_x, crop_y) || chk(crop_x + crop_w, crop_y + crop_h) { return iced::mouse::Interaction::ResizingDiagonallyDown; }
        if chk(crop_x + crop_w, crop_y) || chk(crop_x, crop_y + crop_h) { return iced::mouse::Interaction::ResizingDiagonallyUp; }
        if cursor_position.x >= crop_x && cursor_position.x <= crop_x + crop_w
            && cursor_position.y >= crop_y && cursor_position.y <= crop_y + crop_h {
            return iced::mouse::Interaction::Grab;
        }

        iced::mouse::Interaction::default()
    }
}
