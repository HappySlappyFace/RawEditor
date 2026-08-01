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

/// Draggable handles of the selected local adjustment mask
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaskHandle {
    /// Linear: ramp start pin (weight 1)
    LinearStart,
    /// Linear: ramp end pin (weight 0)
    LinearEnd,
    /// Radial: center pin (also moves the whole linear mask if hit on the line)
    Center,
    /// Radial: horizontal radius handle
    RadiusX,
    /// Radial: vertical radius handle
    RadiusY,
    /// Radial only, used transiently during initial placement: grows both
    /// radii together (aspect-corrected) so a single drag makes a circle
    /// instead of a flat horizontal sliver. Never hit-tested afterward —
    /// once placed, RadiusX/RadiusY resize a single axis as normal.
    RadiusUniform,
    /// Radial only: rotates the ellipse. Dragged via angle (atan2 around the
    /// screen-space center), not a UV delta like the other handles.
    Rotation,
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
    /// Pan offset, fitted-size fractions (see core::viewport::image_rect)
    pub offset_x: f32,
    pub offset_y: f32,
    /// Monotonic id of the pixel buffer's contents, bumped whenever the app
    /// replaces the preview bytes.
    ///
    /// `prepare` runs on every redraw, and redraws happen constantly — every
    /// `CursorMoved` event is forwarded as a message that mutates state, so
    /// merely moving the mouse over the window triggers one. Re-uploading the
    /// full preview each time cost megabytes of PCIe traffic per event. This
    /// counter lets `prepare` skip `write_texture` when the contents are
    /// unchanged.
    ///
    /// A counter rather than an `Arc::as_ptr` comparison: a freed allocation
    /// can be reused at the same address, which would silently skip a needed
    /// upload.
    pub generation: u64,
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
    /// Generation of the pixel data currently resident in `texture`.
    /// `None` means the texture has no valid contents yet (freshly created),
    /// which forces the next upload.
    uploaded_generation: Option<u64>,
    // Vertex buffer: 6 vertices × (2 pos + 2 uv) floats
    vertex_buf: wgpu::Buffer,
}

/// Per-frame uniforms matching the WGSL `Uniforms` struct exactly (std140 / 16-byte aligned).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// Maps the fixed [-1,1] quad to the image_rect placement on screen —
    /// letterbox fit, zoom, and pan are all baked in here (see prepare()).
    proj: [[f32; 4]; 4],
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
                uploaded_generation: None,
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
            // A brand-new texture holds nothing, so the contents must be
            // re-uploaded regardless of what generation was resident before.
            state.uploaded_generation = None;
        }

        // Upload pixels only when the buffer contents actually changed.
        // `prepare` runs on every redraw; without this guard the full preview
        // was re-uploaded on every mouse move (see ViewportPrimitive::generation).
        if state.uploaded_generation != Some(self.generation) {
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
                state.uploaded_generation = Some(self.generation);
            }
        }

        // The quad's on-screen placement (letterbox fit + zoom + pan) — the
        // SAME rectangle the crop/mask canvas overlay computes via
        // `CropOverlay::image_bounds`, so the shader and the overlay always
        // agree. Zoom grows this rect (rather than magnifying UV inside a
        // fixed quad), so on zoom the image physically covers more of the
        // viewport and any letterbox area shrinks — fixing the old "black
        // bars that never fill" bug on portrait images.
        let rect = crate::core::viewport::image_rect(
            self.width,
            self.height,
            bounds.width,
            bounds.height,
            self.zoom,
            cgmath::Vector2::new(self.offset_x, self.offset_y),
        );
        let sx = rect.width / bounds.width;
        let sy = rect.height / bounds.height;
        // Center of the rect, normalized to [0,1] then to clip space [-1,1].
        // Y is flipped: screen space grows downward, clip space grows upward.
        let tx = ((rect.x + rect.width / 2.0) / bounds.width) * 2.0 - 1.0;
        let ty = -(((rect.y + rect.height / 2.0) / bounds.height) * 2.0 - 1.0);

        let matrix = Matrix4::from_translation(cgmath::vec3(tx, ty, 0.0))
            * Matrix4::from_nonuniform_scale(sx, sy, 1.0);

        let uniforms = Uniforms { proj: matrix.into() };
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
    /// Bumped by the app whenever `pixels` is replaced. See
    /// `ViewportPrimitive::generation`.
    pub generation: u64,
}

impl shader::Program<Message> for ViewportProgram {
    type State = ();
    type Primitive = ViewportPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: iced::mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        // Placeholder 1×1 white pixel if no image is loaded yet.
        let mut is_placeholder = false;
        let (pixels, w, h) = match &self.pixels {
            Some(p) if !p.is_empty() => (p.clone(), self.image_width, self.image_height),
            _ => {
                is_placeholder = true;
                let white: std::sync::Arc<[u8]> = std::sync::Arc::from([255u8, 255, 255, 255].as_ref());
                (white, 1, 1)
            }
        };

        ViewportPrimitive {
            pixels,
            width: w,
            height: h,
            zoom: self.zoom,
            offset_x: self.offset.x,
            offset_y: self.offset.y,
            // Generation 0 is reserved for the placeholder, whose single white
            // pixel never changes; real previews start at 1.
            generation: if is_placeholder { 0 } else { self.generation },
        }
    }
}

// ─────────────────────────────── CropOverlay ───────────────────────────────

/// Click radius, in logical pixels, for every draggable pin in the overlay
/// (crop corners and mask handles). Pins that can coexist along the same
/// direction must be spaced by more than twice this to stay separable.
const HANDLE_HIT_RADIUS: f32 = 15.0;

/// Transparent Canvas overlay drawn on top of ViewportProgram.
/// Only draws crop UI elements (dim mask, rule-of-thirds, handles).
pub struct CropOverlay {
    pub zoom: f32,
    pub offset: cgmath::Vector2<f32>,
    pub image_width: u32,
    pub image_height: u32,
    pub is_cropping: bool,
    /// WB eyedropper active: a left-click emits Message::WbPicked with the
    /// display-space UV of the click (letterbox/zoom/pan corrected).
    pub is_wb_picking: bool,
    pub crop: [f32; 4],
    /// Mask placement mode active: a left press emits MaskPlacementStarted
    /// with the full-image UV (crop remap applied).
    pub is_mask_placing: bool,
    /// The selected mask (geometry drawn + handles hit-tested). Copy of the
    /// MaskParams — geometry is full-image display UV, same space as `crop`.
    pub selected_mask: Option<crate::core::types::MaskParams>,
}

impl CropOverlay {
    /// Screen-space rectangle the (zoomed, panned, letterboxed) image occupies
    /// within the viewport. Thin wrapper around the shared transform
    /// (`core::viewport::image_rect`) that the display shader and all input
    /// math also use — draw/update/mouse_interaction all rely on this.
    fn image_bounds(&self, viewport_width: f32, viewport_height: f32) -> Rectangle {
        crate::core::viewport::image_rect(
            self.image_width,
            self.image_height,
            viewport_width,
            viewport_height,
            self.zoom,
            self.offset,
        )
    }

    /// Full-image UV → screen point. When not cropping (the only state mask
    /// editing runs in), the displayed image is the crop sub-rect, so the crop
    /// remap applies. Affine, so UV-space lines map to straight screen lines.
    fn uv_to_screen(&self, u: f32, v: f32, ib: &Rectangle) -> iced::Point {
        let view_u = (u - self.crop[0]) / self.crop[2].max(1e-6);
        let view_v = (v - self.crop[1]) / self.crop[3].max(1e-6);
        iced::Point::new(ib.x + view_u * ib.width, ib.y + view_v * ib.height)
    }

    /// Screen point → full-image UV (inverse of `uv_to_screen`).
    fn screen_to_uv(&self, p: iced::Point, ib: &Rectangle) -> (f32, f32) {
        let view_u = (p.x - ib.x) / ib.width.max(1.0);
        let view_v = (p.y - ib.y) / ib.height.max(1.0);
        (
            self.crop[0] + view_u * self.crop[2],
            self.crop[1] + view_v * self.crop[3],
        )
    }

    /// Draw the selected mask's geometry: pins + connecting line + the two
    /// weight iso-lines for linear masks; outer/feather ellipses for radial.
    /// Everything is computed in UV space and mapped through `uv_to_screen`,
    /// so the overlay matches the shader's mask exactly at any zoom/pan/crop.
    fn draw_mask_overlay(
        &self,
        renderer: &iced::Renderer,
        bounds: Rectangle,
    ) -> Vec<canvas::Geometry> {
        let Some(m) = self.selected_mask else {
            return vec![];
        };

        let ib = self.image_bounds(bounds.width, bounds.height);
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        let stroke = canvas::Stroke::default()
            .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.9))
            .with_width(1.5);
        let faint = canvas::Stroke::default()
            .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.45))
            .with_width(1.0);

        let handle_size = 12.0;
        let draw_pin = |frame: &mut canvas::Frame, p: iced::Point| {
            let pos = iced::Point::new(p.x - handle_size / 2.0, p.y - handle_size / 2.0);
            let size = iced::Size::new(handle_size, handle_size);
            frame.fill_rectangle(pos, size, Color::WHITE);
            frame.stroke(
                &canvas::Path::rectangle(pos, size),
                canvas::Stroke::default()
                    .with_color(Color::BLACK)
                    .with_width(1.0),
            );
        };

        if m.mask_type == 0 {
            // Linear: iso-lines are perpendicular to B−A in UV space. Mapping
            // UV endpoints through the affine uv_to_screen keeps them exact
            // even though UV→screen scales axes unequally.
            let (dx, dy) = (m.bx - m.ax, m.by - m.ay);
            let len = (dx * dx + dy * dy).sqrt();
            if len > 1e-5 {
                // Perpendicular direction in UV, extended well past the image
                let (px, py) = (-dy / len, dx / len);
                let ext = 4.0;
                let iso = |cx: f32, cy: f32| {
                    canvas::Path::line(
                        self.uv_to_screen(cx - px * ext, cy - py * ext, &ib),
                        self.uv_to_screen(cx + px * ext, cy + py * ext, &ib),
                    )
                };
                frame.stroke(&iso(m.ax, m.ay), stroke);
                frame.stroke(&iso(m.bx, m.by), stroke);
                frame.stroke(
                    &canvas::Path::line(
                        self.uv_to_screen(m.ax, m.ay, &ib),
                        self.uv_to_screen(m.bx, m.by, &ib),
                    ),
                    faint,
                );
            }
            draw_pin(&mut frame, self.uv_to_screen(m.ax, m.ay, &ib));
            draw_pin(&mut frame, self.uv_to_screen(m.bx, m.by, &ib));
            draw_pin(
                &mut frame,
                self.uv_to_screen((m.ax + m.bx) * 0.5, (m.ay + m.by) * 0.5, &ib),
            );
        } else {
            // Radial: ellipse in UV space, screen radii already account for
            // the UV→screen aspect distortion (same conversion RadiusUniform
            // uses at placement time). rx/ry being genuine screen-space
            // pixel radii is why the rotation below needs NO further aspect
            // correction — unlike mask_weight's UV-space math, this is
            // already isotropic.
            let center = self.uv_to_screen(m.ax, m.ay, &ib);
            let rx = m.bx / self.crop[2].max(1e-6) * ib.width;
            let ry = m.by / self.crop[3].max(1e-6) * ib.height;
            let rotation = iced::Radians(m.rotation.to_radians());
            let ellipse = |radii: iced::Vector| {
                canvas::Path::new(|b| {
                    b.ellipse(canvas::path::arc::Elliptical {
                        center,
                        radii,
                        rotation,
                        start_angle: iced::Radians(0.0),
                        end_angle: iced::Radians(std::f32::consts::TAU),
                    });
                })
            };
            frame.stroke(&ellipse(iced::Vector::new(rx, ry)), stroke);
            // Inner feather edge: full weight inside, falloff between the two
            let inner = 1.0 - m.feather.clamp(0.0, 0.999);
            if inner > 0.01 && m.feather > 0.01 {
                frame.stroke(&ellipse(iced::Vector::new(rx * inner, ry * inner)), faint);
            }
            draw_pin(&mut frame, center);
            draw_pin(&mut frame, center + Self::rotate_screen_vector(m.rotation, rx, 0.0));
            draw_pin(&mut frame, center + Self::rotate_screen_vector(m.rotation, 0.0, ry));

            // Rotation handle: further out along the ellipse's own (rotated)
            // major-radius direction, with a thin line back to center as a
            // visual angle indicator.
            let handle_dist = Self::rotation_handle_dist(rx, ry);
            let rotation_handle = center + Self::rotate_screen_vector(m.rotation, handle_dist, 0.0);
            frame.stroke(&canvas::Path::line(center, rotation_handle), faint);
            draw_pin(&mut frame, rotation_handle);
        }

        vec![frame.into_geometry()]
    }

    /// Rotate a screen-space vector by `degrees`, matching iced's
    /// `Elliptical.rotation` convention (clockwise for positive angles,
    /// since screen Y increases downward) — the standard rotation matrix
    /// applied directly in screen coordinates, no aspect correction needed
    /// since screen space is already isotropic (unlike UV space).
    fn rotate_screen_vector(degrees: f32, x: f32, y: f32) -> iced::Vector {
        let r = degrees.to_radians();
        let (c, s) = (r.cos(), r.sin());
        iced::Vector::new(x * c - y * s, x * s + y * c)
    }

    /// Distance from the ellipse center to the rotation pin, along the
    /// ellipse's own major-radius direction.
    ///
    /// Additive, not proportional: a multiplicative `rx.max(ry) * 1.25` put the
    /// rotation pin only `0.25 * rx` from the RadiusX pin, so on any mask under
    /// ~60px on screen (i.e. every freshly placed one) the two hit circles
    /// overlapped and the earlier-listed RadiusX swallowed clicks meant for the
    /// rotation pin. Keep the gap above `2 * HANDLE_HIT_RADIUS`.
    fn rotation_handle_dist(rx: f32, ry: f32) -> f32 {
        rx.max(ry) + 3.0 * HANDLE_HIT_RADIUS
    }

    /// Screen positions of the selected mask's drag handles, in hit-test
    /// priority order (geometry handles before the center pin).
    fn mask_handle_positions(&self, ib: &Rectangle) -> Vec<(MaskHandle, iced::Point)> {
        let Some(m) = self.selected_mask else {
            return vec![];
        };
        if m.mask_type == 0 {
            vec![
                (MaskHandle::LinearStart, self.uv_to_screen(m.ax, m.ay, ib)),
                (MaskHandle::LinearEnd, self.uv_to_screen(m.bx, m.by, ib)),
                (
                    MaskHandle::Center,
                    self.uv_to_screen((m.ax + m.bx) * 0.5, (m.ay + m.by) * 0.5, ib),
                ),
            ]
        } else {
            // Handle positions rotate with the ellipse (matching
            // draw_mask_overlay) so they stay exactly on its visual
            // boundary — and so dragging RadiusX/RadiusY still moves the
            // handle in the direction it's visually pointing at any
            // rotation (apply_mask_drag projects the drag delta onto these
            // same rotated local axes).
            let center = self.uv_to_screen(m.ax, m.ay, ib);
            let rx = m.bx / self.crop[2].max(1e-6) * ib.width;
            let ry = m.by / self.crop[3].max(1e-6) * ib.height;
            let handle_dist = Self::rotation_handle_dist(rx, ry);
            // Rotation before RadiusX: they share a direction from the center,
            // so if a degenerate mask ever collapses the gap between them, the
            // outer pin (the one the user aimed at) wins the first-match scan.
            vec![
                (MaskHandle::Rotation, center + Self::rotate_screen_vector(m.rotation, handle_dist, 0.0)),
                (MaskHandle::RadiusX, center + Self::rotate_screen_vector(m.rotation, rx, 0.0)),
                (MaskHandle::RadiusY, center + Self::rotate_screen_vector(m.rotation, 0.0, ry)),
                (MaskHandle::Center, center),
            ]
        }
    }
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
            return self.draw_mask_overlay(renderer, bounds);
        }

        let image_bounds = self.image_bounds(bounds.width, bounds.height);

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

        if !self.is_cropping
            && !self.is_wb_picking
            && !self.is_mask_placing
            && self.selected_mask.is_none()
        {
            return (canvas::event::Status::Ignored, None);
        }

        let cursor_position = match cursor.position_in(bounds) {
            Some(p) => p,
            None => return (canvas::event::Status::Ignored, None),
        };

        // WB eyedropper: emit the display-space UV of a left click.
        if self.is_wb_picking {
            if let canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) = event {
                let image_bounds = self.image_bounds(bounds.width, bounds.height);
                let u = (cursor_position.x - image_bounds.x) / image_bounds.width.max(1.0);
                let v = (cursor_position.y - image_bounds.y) / image_bounds.height.max(1.0);
                if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) {
                    return (canvas::event::Status::Captured, Some(Message::WbPicked(u, v)));
                }
            }
            return (canvas::event::Status::Ignored, None);
        }

        // Mask placement: a left press creates the mask at the pressed point
        // (the handler converts the press into an in-flight handle drag so the
        // same gesture sizes the new mask).
        if self.is_mask_placing {
            if let canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) = event {
                let image_bounds = self.image_bounds(bounds.width, bounds.height);
                let view_u = (cursor_position.x - image_bounds.x) / image_bounds.width.max(1.0);
                let view_v = (cursor_position.y - image_bounds.y) / image_bounds.height.max(1.0);
                if (0.0..=1.0).contains(&view_u) && (0.0..=1.0).contains(&view_v) {
                    let (u, v) = self.screen_to_uv(cursor_position, &image_bounds);
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::MaskPlacementStarted(u, v)),
                    );
                }
            }
            return (canvas::event::Status::Ignored, None);
        }

        // Selected mask: hit-test its handles (mask editing and crop mode are
        // mutually exclusive, so is_cropping is false on this path).
        if !self.is_cropping && self.selected_mask.is_some() {
            if let canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) = event {
                let image_bounds = self.image_bounds(bounds.width, bounds.height);
                let hr = HANDLE_HIT_RADIUS;
                for (handle, p) in self.mask_handle_positions(&image_bounds) {
                    let dx = cursor_position.x - p.x;
                    let dy = cursor_position.y - p.y;
                    if dx * dx + dy * dy < hr * hr {
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::MaskHandleGrabbed(handle, image_bounds)),
                        );
                    }
                }
            }
            return (canvas::event::Status::Ignored, None);
        }

        if let canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) = event {
            let image_bounds = self.image_bounds(bounds.width, bounds.height);
            let crop_x = image_bounds.x + (self.crop[0] * image_bounds.width);
            let crop_y = image_bounds.y + (self.crop[1] * image_bounds.height);
            let crop_w = self.crop[2] * image_bounds.width;
            let crop_h = self.crop[3] * image_bounds.height;
            let hr = HANDLE_HIT_RADIUS;
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
        if self.is_wb_picking || self.is_mask_placing {
            return iced::mouse::Interaction::Crosshair;
        }
        if !self.is_cropping {
            // Distinct cursor per handle type over the selected mask's
            // handles. iced has no dedicated "rotate" cursor, so Rotation
            // uses Move (the closest "manipulate this element" hint) rather
            // than Crosshair, which already means "placement mode" elsewhere.
            // Resize cursors are a static approximation — they don't tilt
            // to match the mask's actual rotation angle.
            if self.selected_mask.is_some() {
                if let Some(p) = cursor.position_in(bounds) {
                    let ib = self.image_bounds(bounds.width, bounds.height);
                    let hr = HANDLE_HIT_RADIUS;
                    for (handle, hp) in self.mask_handle_positions(&ib) {
                        let dx = p.x - hp.x;
                        let dy = p.y - hp.y;
                        if dx * dx + dy * dy < hr * hr {
                            return match handle {
                                MaskHandle::RadiusX => iced::mouse::Interaction::ResizingHorizontally,
                                MaskHandle::RadiusY => iced::mouse::Interaction::ResizingVertically,
                                MaskHandle::Rotation => iced::mouse::Interaction::Move,
                                _ => iced::mouse::Interaction::Grab,
                            };
                        }
                    }
                }
            }
            return iced::mouse::Interaction::default();
        }
        let cursor_position = match cursor.position_in(bounds) {
            Some(p) => p,
            None => return iced::mouse::Interaction::default(),
        };

        let image_bounds = self.image_bounds(bounds.width, bounds.height);

        let crop_x = image_bounds.x + (self.crop[0] * image_bounds.width);
        let crop_y = image_bounds.y + (self.crop[1] * image_bounds.height);
        let crop_w = self.crop[2] * image_bounds.width;
        let crop_h = self.crop[3] * image_bounds.height;
        let hr = HANDLE_HIT_RADIUS;
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
