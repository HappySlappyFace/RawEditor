//! Rasterises brush strokes (`core::brush`) into per-mask layers.
//!
//! Each mask owns one layer of an R16Float array texture holding a signed
//! value `L`: painting moves it towards +1, erasing towards −1. The colour
//! shader combines it with the mask's own shape as `clamp(shape + L, 0, 1)`,
//! so the same strokes can extend a gradient, cut holes in it, or — on a
//! brush mask, whose shape is 0 — be the whole mask.
//!
//! One render pass per stroke, scissored to its bounding box. The fragment
//! takes the MAX coverage over the stroke's capsule segments (so joints don't
//! double up) and alpha-blends towards ±1 (so later strokes paint over
//! earlier ones, erase-after-add included, and flow builds up). Runs only
//! when the strokes change; the colour pass just samples the layer.

use iced_wgpu::wgpu;

use crate::core::brush::{BrushData, MAX_POINTS};
use crate::core::types::MAX_MASKS;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// Longest side of a layer. Brush edges are soft; past this a layer only
/// costs memory (8 layers × 2048 × 1365 × 2 B ≈ 45 MB).
pub const MAX_LAYER_SIDE: u32 = 2048;
const SLOT: u64 = 256;

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuStroke {
    start: u32,
    len: u32,
    radius: f32,
    inner: f32,
    flow: f32,
    /// +1 to paint, −1 to erase (`target` is a reserved word in WGSL).
    goal: f32,
    aspect: f32,
    _pad: f32,
    size: [f32; 2],
    _pad2: [f32; 2],
}

pub const BRUSH_SHADER: &str = r#"
struct Stroke {
    start: u32,
    len: u32,
    radius: f32,
    inner: f32,
    flow: f32,
    goal: f32,
    aspect: f32,
    pad: f32,
    size: vec2<f32>,
    pad2: vec2<f32>,
}

@group(0) @binding(0) var<storage, read> points: array<vec2<f32>>;
@group(0) @binding(1) var<uniform> st: Stroke;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(vi & 1u) * 4 - 1);
    let y = f32(i32(vi >> 1u) * 4 - 1);
    return vec4<f32>(x, -y, 0.0, 1.0);
}

// Distance from q to segment ab.
fn seg_dist(q: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = clamp(dot(q - a, ab) / max(dot(ab, ab), 1e-12), 0.0, 1.0);
    return length(q - (a + t * ab));
}

@fragment
fn fs_stroke(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // Isotropic space: u scaled by width/height, v as is (radius is in v units).
    let uv = pos.xy / st.size;
    let q = vec2<f32>(uv.x * st.aspect, uv.y);
    let iso = vec2<f32>(st.aspect, 1.0);
    var d = 1e9;
    let a0 = points[st.start] * iso;
    if (st.len < 2u) {
        d = length(q - a0);
    } else {
        var a = a0;
        for (var i = 1u; i < st.len; i++) {
            let b = points[st.start + i] * iso;
            d = min(d, seg_dist(q, a, b));
            a = b;
        }
    }
    let cov = (1.0 - smoothstep(st.inner, st.radius, d)) * st.flow;
    return vec4<f32>(st.goal, 0.0, 0.0, cov);
}
"#;

#[derive(Debug)]
pub struct BrushRaster {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

/// One image's layer array plus the buffers the passes read.
#[derive(Debug)]
pub struct BrushLayers {
    #[allow(dead_code)] // owns the allocation behind the views
    texture: wgpu::Texture,
    /// All layers, for the colour pass to sample.
    pub array_view: wgpu::TextureView,
    layer_views: Vec<wgpu::TextureView>,
    points: wgpu::Buffer,
    strokes: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
impl BrushLayers {
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
}

/// Which mask layers must be redrawn to go from `prev` to `cur`: those whose
/// own strokes (in order, with their points) differ. Stroke offsets into the
/// shared point array are ignored, since another mask's edit shifts them.
pub fn changed_masks(prev: Option<&BrushData>, cur: &BrushData) -> [bool; MAX_MASKS] {
    let Some(prev) = prev else {
        return [true; MAX_MASKS];
    };
    let mut dirty = [false; MAX_MASKS];
    for (mask, d) in dirty.iter_mut().enumerate() {
        let mut a = prev.live().iter().filter(|s| s.mask as usize == mask);
        let mut b = cur.live().iter().filter(|s| s.mask as usize == mask);
        *d = loop {
            match (a.next(), b.next()) {
                (None, None) => break false,
                (Some(x), Some(y)) => {
                    let same = x.erase == y.erase
                        && x.radius == y.radius
                        && x.feather == y.feather
                        && x.flow == y.flow
                        && prev.points_of(x) == cur.points_of(y);
                    if !same {
                        break true;
                    }
                }
                _ => break true,
            }
        };
    }
    dirty
}

/// Layer size for an image displayed at `w × h`: same aspect, long side
/// capped — never larger than the working texture, which has no more detail.
pub fn layer_size(w: u32, h: u32) -> (u32, u32) {
    let (w, h) = (w.max(1), h.max(1));
    let long = w.max(h);
    let s = (MAX_LAYER_SIDE as f32 / long as f32).min(1.0);
    (((w as f32 * s).round() as u32).max(1), ((h as f32 * s).round() as u32).max(1))
}

/// A 1×1 array standing in for the layers until anything is painted, so the
/// colour pass's bind group is always complete.
pub fn placeholder(device: &wgpu::Device) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("Brush layers (empty)"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: MAX_MASKS as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
}

impl BrushRaster {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Brush raster"),
            source: wgpu::ShaderSource::Wgsl(BRUSH_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Brush layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<GpuStroke>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Brush pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Brush stroke"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: "vs_main", buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: "fs_stroke",
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    // L' = L·(1 − cov) + target·cov
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::RED,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        Self { pipeline, layout }
    }

    pub fn layers(&self, device: &wgpu::Device, width: u32, height: u32) -> BrushLayers {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Brush layers"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: MAX_MASKS as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            // COPY_SRC only so tests can read the layers back.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | if cfg!(test) { wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::empty() },
            view_formats: &[],
        });
        let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let layer_views = (0..MAX_MASKS as u32)
            .map(|i| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let points = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Brush points"),
            size: (MAX_POINTS * 8) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let strokes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Brush strokes"),
            size: SLOT * crate::core::brush::MAX_STROKES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Brush bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: points.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &strokes,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<GpuStroke>() as u64),
                    }),
                },
            ],
        });
        BrushLayers { texture, array_view, layer_views, points, strokes, bind_group, width, height }
    }

    /// Redraw the layers whose strokes differ from `prev` (what the layers
    /// currently hold; `None` = unknown, redraw all), then submit. A layer
    /// is cleared and redrawn from all of its own strokes, so the result is
    /// identical to a full redraw.
    pub fn rasterize(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layers: &BrushLayers,
        data: &BrushData,
        prev: Option<&BrushData>,
    ) {
        let dirty = changed_masks(prev, data);
        let (w, h) = (layers.width as f32, layers.height as f32);
        let aspect = w / h;
        let n = (data.point_count as usize).min(MAX_POINTS);
        if n > 0 {
            queue.write_buffer(&layers.points, 0, bytemuck::cast_slice(&data.points[..n]));
        }
        let strokes = data.live();
        let mut slots = vec![0u8; SLOT as usize * strokes.len().max(1)];
        for (i, s) in strokes.iter().enumerate() {
            let g = GpuStroke {
                start: s.start,
                len: s.len,
                radius: s.radius,
                inner: s.radius * (1.0 - s.feather.clamp(0.0, 1.0)),
                flow: s.flow,
                goal: if s.erase { -1.0 } else { 1.0 },
                aspect,
                _pad: 0.0,
                size: [w, h],
                _pad2: [0.0; 2],
            };
            let bytes = bytemuck::bytes_of(&g);
            slots[i * SLOT as usize..][..bytes.len()].copy_from_slice(bytes);
        }
        queue.write_buffer(&layers.strokes, 0, &slots);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Brush encoder"),
        });
        for (view, _) in layers.layer_views.iter().zip(&dirty).filter(|(_, d)| **d) {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Brush clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        for (i, s) in strokes.iter().enumerate() {
            let mask = s.mask as usize;
            if mask >= MAX_MASKS || s.len == 0 || !dirty[mask] {
                continue;
            }
            let Some((x, y, sw, sh)) = scissor(data.points_of(s), s.radius, aspect, layers.width, layers.height) else {
                continue;
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Brush stroke"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &layers.layer_views[mask],
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &layers.bind_group, &[(i as u64 * SLOT) as u32]);
            pass.set_scissor_rect(x, y, sw, sh);
            pass.draw(0..3, 0..1);
        }
        queue.submit(Some(encoder.finish()));
    }
}

/// Pixel bounding box of a stroke (points ± radius), clipped to the layer.
fn scissor(points: &[[f32; 2]], radius: f32, aspect: f32, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let (mut u0, mut v0, mut u1, mut v1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in points {
        u0 = u0.min(p[0]);
        u1 = u1.max(p[0]);
        v0 = v0.min(p[1]);
        v1 = v1.max(p[1]);
    }
    let ru = radius / aspect.max(1e-6);
    let x0 = (((u0 - ru) * w as f32).floor().max(0.0)) as u32;
    let y0 = (((v0 - radius) * h as f32).floor().max(0.0)) as u32;
    let x1 = (((u1 + ru) * w as f32).ceil().min(w as f32)) as u32;
    let y1 = (((v1 + radius) * h as f32).ceil().min(h as f32)) as u32;
    (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brush_shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(BRUSH_SHADER)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(BRUSH_SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("brush shader failed validation: {e:?}"));
    }

    #[test]
    fn only_the_edited_masks_are_redrawn() {
        let stroke = |b: &mut BrushData, mask: u32, pts: &[[f32; 2]]| {
            b.begin(mask, false, 0.05, 0.5, 1.0).unwrap();
            for p in pts {
                b.push_raw(*p);
            }
        };
        let mut a = BrushData::default();
        stroke(&mut a, 0, &[[0.1, 0.1], [0.2, 0.2]]);
        stroke(&mut a, 3, &[[0.5, 0.5], [0.6, 0.6]]);
        assert_eq!(changed_masks(None, &a), [true; MAX_MASKS]);
        assert_eq!(changed_masks(Some(&a), &a), [false; MAX_MASKS]);
        // Another stroke on mask 3 shifts nothing of mask 0's.
        let mut b = a;
        stroke(&mut b, 3, &[[0.7, 0.7]]);
        let d = changed_masks(Some(&a), &b);
        assert!(d[3] && !d[0] && d.iter().filter(|x| **x).count() == 1);
        // Removing mask 0 empties it and leaves mask 3 alone (its points moved).
        let mut c = a;
        c.clear_mask(0);
        let d = changed_masks(Some(&a), &c);
        assert!(d[0] && !d[3]);
    }

    #[test]
    fn stroke_uniform_fits_a_slot() {
        assert_eq!(std::mem::size_of::<GpuStroke>() % 16, 0);
        assert!(std::mem::size_of::<GpuStroke>() as u64 <= SLOT);
    }

    #[test]
    fn layer_size_keeps_aspect_and_caps_the_long_side() {
        assert_eq!(layer_size(6000, 4000), (2048, 1365));
        assert_eq!(layer_size(1000, 1500), (1000, 1500));
    }

    #[test]
    fn scissor_covers_the_stroke_and_clips_to_the_layer() {
        let r = scissor(&[[0.5, 0.5]], 0.1, 1.5, 1500, 1000).unwrap();
        // ±0.1 in v = ±100 px; ±0.1/1.5 in u = ±100 px.
        assert_eq!(r, (650, 400, 200, 200));
        let edge = scissor(&[[0.0, 0.0]], 0.1, 1.0, 100, 100).unwrap();
        assert_eq!((edge.0, edge.1), (0, 0));
        assert!(scissor(&[[2.0, 2.0]], 0.01, 1.0, 100, 100).is_none());
    }
}
