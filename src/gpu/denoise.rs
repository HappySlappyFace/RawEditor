//! Noise reduction: a cached pre-pass between the demosaic and the colour pass.
//!
//! The colour pass re-runs on every slider tick, so anything expensive there
//! is paid per mouse-move. Denoising only depends on its own three sliders,
//! so it runs here, once per change, into a texture the colour pass then
//! samples. Exposure, WB, tone, masks… never re-run it.
//!
//! ## Space
//! Camera RGB × as-shot WB, split into an opponent space
//! (`Y = (r+2g+b)/4`, `U = (r−b)/2`, `V = (r+b)/2 − g`) so luma and chroma can
//! be treated separately — chroma noise is blotchy and low-frequency, luma
//! noise is fine grain, and the eye tolerates smoothing of the first far more
//! than the second. `Y` is green-weighted like perceived luminance, and `U`
//! and `V` are both orthogonal to it, so smoothing chroma cannot move
//! brightness. (With an equal-weight `Y`, chroma smoothing traded green for
//! magenta at constant channel mean and visibly altered luma texture —
//! measurably worse luma PSNR with colour NR on.)
//!
//! Every *weight* is computed in the variance-stabilised space of the image's
//! own noise model (`core::noise`), where noise has unit variance at every
//! brightness — so thresholds are in units of σ and mean the same at ISO 100
//! and ISO 6400. Every *average* is taken of the linear values, which keeps
//! the result unbiased (averaging square-root-domain values and squaring back
//! darkens shadows).
//!
//! ## Passes
//! | pass         | does                                                     |
//! |--------------|----------------------------------------------------------|
//! | `fs_prep`    | camera RGB → (Ys, Y, U, V); Ys = stabilised luma          |
//! | `fs_nlm`     | non-local means on luma (3×3 patches, 9×9 search)         |
//! | `fs_scale`   | edge-avoiding à-trous, one pass per scale: U/V, and luma  |
//! |              | from the second scale up                                  |
//! | `fs_compose` | blend with the original by the slider amounts → camera RGB|
//!
//! Non-local means keeps texture that a bilateral filter would smear: it
//! averages pixels whose *neighbourhoods* match, not just whose values do.
//! The à-trous chain (B3-spline kernel, step doubling per scale, weights that
//! stop at luma and chroma edges) reaches blotches tens of pixels wide at
//! the cost of 25 taps per scale.
//!
//! Luma rides the same chain from step 2 up, with an edge stop that tightens
//! per scale. NLM's 9×9 window only reaches fine grain; at full resolution
//! (export, 100% zoom) high-ISO luma noise also has a coarse, mottled
//! component that it leaves behind as blotches in skies and walls. The taps
//! are already loaded for chroma, so this costs arithmetic only.

use iced_wgpu::wgpu;

use crate::core::noise::NoiseModel;

/// Per-pass uniforms. Each pass gets its own copy at a 256-byte stride in one
/// buffer, selected with a dynamic offset.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuDenoiseParams {
    pub wb: [f32; 4],
    pub inv_wb: [f32; 4],
    /// Generalised Anscombe coefficients per colour: s = k·sqrt(a·x + c0).
    pub vst_a: [f32; 4],
    pub vst_c0: [f32; 4],
    pub vst_k: [f32; 4],
    /// NLM: patch distance treated as pure noise (subtracted before decay).
    pub luma_floor: f32,
    /// NLM: 1/h² — how fast weight decays past the floor.
    pub luma_inv_h2: f32,
    pub luma_amount: f32,
    pub chroma_amount: f32,
    /// À-trous: 1/(2σ²) of the luma edge stop, in stabilised units.
    pub luma_edge: f32,
    /// À-trous: 1/(2σ²) of the chroma edge stop for this scale.
    pub chroma_edge: f32,
    /// À-trous dilation (1, 2, 4, …).
    pub step: i32,
    /// NLM search radius in pixels.
    pub radius: i32,
    /// À-trous: 1/(2σ²) of the stop for averaging LUMA at this scale, in
    /// stabilised units — the coarse-grain counterpart of the NLM pass.
    pub luma_scale_edge: f32,
    /// À-trous: how much of this scale's luma average is taken (0 = luma
    /// untouched at this scale).
    pub luma_scale_mix: f32,
    pub _pad: [f32; 2],
}

/// Uniform slot stride — `min_uniform_buffer_offset_alignment` is ≤ 256 on
/// every adapter wgpu supports.
const SLOT: u64 = 256;
/// Maximum chroma scales.
pub const MAX_LEVELS: usize = 6;
/// prep + nlm + scales + compose.
const MAX_PASSES: usize = MAX_LEVELS + 3;

/// The sliders that drive this stage. When all are zero the stage is
/// skipped entirely and the colour pass samples the demosaic directly.
///
/// AI denoise replaces it: with `ai_denoise` applied the mosaic is already
/// clean, so these read as zero (the stage is skipped) whatever the sliders
/// say. The sliders keep their values for when AI denoise is removed. This
/// is the one place that rule lives; every render path builds its settings
/// here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DenoiseSettings {
    pub luma: f32,
    pub detail: f32,
    pub color: f32,
}

impl DenoiseSettings {
    pub fn from_params(p: &crate::core::types::EditParams) -> Self {
        if p.ai_denoise > 0.0 {
            return Self { luma: 0.0, detail: p.luma_detail.clamp(0.0, 1.0), color: 0.0 };
        }
        Self {
            luma: p.luma_noise.clamp(0.0, 1.0),
            detail: p.luma_detail.clamp(0.0, 1.0),
            color: p.color_noise.clamp(0.0, 1.0),
        }
    }

    pub fn is_active(&self) -> bool {
        self.luma > 0.0 || self.color > 0.0
    }
}

/// One planned pass: which entry point, and its uniforms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PassKind {
    Prep,
    Nlm,
    /// One à-trous scale (chroma and/or coarse luma).
    Scale,
    Compose,
}

/// NLM search radius, in pixels.
const NLM_RADIUS: i32 = 4;
/// NLM patch half-size (3×3 patches).
const NLM_PATCH: i32 = 1;

/// À-trous scale count for a working texture subsampled by `stride`.
/// Chroma blotches have a fixed size on the sensor: fewer scales reach them
/// on a subsampled preview.
fn scale_levels(stride: f32) -> i32 {
    let extra = stride.max(1.0).log2().round() as i32;
    (6 - extra).clamp(2, MAX_LEVELS as i32)
}

/// How far, in pixels, an output pixel's value can depend on: NLM's search
/// window plus its patch, then each à-trous scale's 5×5 kernel at a step that
/// doubles. Denoising a window padded by this much gives exactly the pixels a
/// whole-image run gives inside it — what lets the detail loupe
/// (`gpu::loupe`) denoise a crop.
pub fn reach(stride: f32) -> u32 {
    let scales: i32 = (0..scale_levels(stride)).map(|level| 2 * (1 << level)).sum();
    (NLM_RADIUS + NLM_PATCH + scales) as u32
}

/// Turn slider settings + the image's noise model into the pass list.
///
/// `stride` is how far the working texture is subsampled from the sensor
/// (1 at full resolution, ~3 for the develop preview). Chroma blotches have a
/// fixed size *on the sensor*, so a full-resolution render needs more à-trous
/// scales than the preview to reach them — otherwise export would look
/// noisier than the screen.
pub fn plan(
    s: DenoiseSettings,
    model: &NoiseModel,
    wb: [f32; 3],
    stride: f32,
) -> Vec<(PassKind, GpuDenoiseParams)> {
    let mut base = GpuDenoiseParams {
        wb: [wb[0], wb[1], wb[2], 0.0],
        inv_wb: [1.0 / wb[0], 1.0 / wb[1], 1.0 / wb[2], 0.0],
        ..Default::default()
    };
    for c in 0..3 {
        let (a, c0, k) = model.vst(c);
        base.vst_a[c] = a;
        base.vst_c0[c] = c0;
        base.vst_k[c] = k;
    }

    // Stabilised luma is the mean of three unit-variance channels, and the
    // demosaic's interpolation correlates them further; ~0.5σ is what flat
    // regions measure.
    const SIGMA_YS: f32 = 0.5;
    // Tuned on ISO 1600 frames: at 0.5 roughly half the grain goes and fine
    // knit texture survives; only near 1.0 does it approach the smooth look.
    // Re-checked against ground truth (`gpu::eval_tests::nr_sweep`) once the
    // noise model stopped counting texture as noise: luma PSNR peaks between
    // 0.5 (ISO 800) and 0.75 (ISO 12800), so the slider's useful range is its
    // whole travel rather than its first quarter.
    let f = s.luma;
    base.luma_floor = 2.0 * SIGMA_YS * SIGMA_YS * (0.4 + 1.2 * f);
    let h = SIGMA_YS * (0.35 + 1.3 * f) * (1.5 - s.detail);
    base.luma_inv_h2 = 1.0 / (h * h).max(1e-6);
    // Blending a little of the original back reads as fine grain rather
    // than the waxy flatness of a fully averaged result.
    base.luma_amount = (f * 1.5).min(1.0);
    base.radius = NLM_RADIUS;

    let g = s.color;
    base.chroma_amount = (g * 2.0).min(1.0);
    // Luma edges stop the chroma blur at ~3σ of (already denoised) luma.
    base.luma_edge = 1.0 / (2.0 * 1.5f32.powi(2));

    let mut passes = vec![(PassKind::Prep, base)];
    if f > 0.0 {
        passes.push((PassKind::Nlm, base));
    }
    if g > 0.0 || f > 0.0 {
        for level in 0..scale_levels(stride) {
            let mut p = base;
            p.step = 1 << level;
            // Chroma differences beyond this are real colour edges. Shrinks
            // per scale: each pass has already averaged away noise.
            let sigma_c = (1.0 + 6.0 * g) / (1u32 << level) as f32;
            // 0 = leave chroma alone at this scale (luma-only plan).
            p.chroma_edge = if g > 0.0 { 1.0 / (2.0 * sigma_c * sigma_c) } else { 0.0 };
            // Coarse luma from the second scale up (the first is NLM's
            // territory). Its edge stop is in the same stabilised units as
            // the NLM's and halves per scale like chroma's; Detail narrows
            // it exactly as it narrows the NLM. Measured on synthetic ISO
            // 3200–12800 frames this removes the mottle NLM leaves at full
            // resolution without costing PSNR on facades and foliage.
            if f > 0.0 && level >= 1 {
                let sigma_l = SIGMA_YS * (0.25 + f) * (1.5 - s.detail) / (1u32 << level) as f32;
                p.luma_scale_edge = 1.0 / (2.0 * sigma_l * sigma_l);
                p.luma_scale_mix = (f * 1.5).min(1.0);
            }
            passes.push((PassKind::Scale, p));
        }
    }
    passes.push((PassKind::Compose, base));
    passes
}

pub const DENOISE_SHADER: &str = r#"
struct DenoiseParams {
    wb: vec4<f32>,
    inv_wb: vec4<f32>,
    vst_a: vec4<f32>,
    vst_c0: vec4<f32>,
    vst_k: vec4<f32>,
    luma_floor: f32,
    luma_inv_h2: f32,
    luma_amount: f32,
    chroma_amount: f32,
    luma_edge: f32,
    chroma_edge: f32,
    step: i32,
    radius: i32,
    luma_scale_edge: f32,
    luma_scale_mix: f32,
    pad0: f32,
    pad1: f32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var orig_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> np: DenoiseParams;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(vi & 1u) * 4 - 1);
    let y = f32(i32(vi >> 1u) * 4 - 1);
    return vec4<f32>(x, -y, 0.0, 1.0);
}

fn src(p: vec2<i32>) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(src_tex)) - vec2<i32>(1);
    return textureLoad(src_tex, clamp(p, vec2<i32>(0), d), 0);
}

// Variance-stabilising transform: unit noise variance per channel.
fn vst(x: vec3<f32>) -> vec3<f32> {
    return np.vst_k.rgb * sqrt(max(np.vst_a.rgb * x + np.vst_c0.rgb, vec3<f32>(0.0)));
}

fn to_yuv(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(0.25 * c.r + 0.5 * c.g + 0.25 * c.b, (c.r - c.b) * 0.5, (c.r + c.b) * 0.5 - c.g);
}

fn from_yuv(v: vec3<f32>) -> vec3<f32> {
    let g = v.x - 0.5 * v.z;
    let rb = v.x + 0.5 * v.z;
    return vec3<f32>(rb + v.y, g, rb - v.y);
}

// Stabilised chroma of a linear YUV triple, each component scaled to unit
// noise (U has variance 1/2, V has 3/2 for unit-variance channels).
fn stab_chroma(yuv: vec3<f32>) -> vec2<f32> {
    let s = vst(max(from_yuv(yuv), vec3<f32>(0.0)));
    return vec2<f32>((s.r - s.b) * 0.70710678, ((s.r + s.b) * 0.5 - s.g) * 0.81649658);
}

@fragment
fn fs_prep(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let x = max(src(p).rgb, vec3<f32>(0.0)) * np.wb.rgb;
    let s = vst(x);
    return vec4<f32>((s.r + s.g + s.b) / 3.0, to_yuv(x));
}

// Non-local means on luma. Each candidate in the search window is weighted by
// how well its 3×3 neighbourhood of stabilised luma matches the centre's;
// patch distances up to `luma_floor` (what pure noise produces) count fully.
@fragment
fn fs_nlm(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    var cp: array<f32, 9>;
    for (var k = 0; k < 9; k++) {
        cp[k] = src(p + vec2<i32>(k % 3 - 1, k / 3 - 1)).x;
    }
    let r = np.radius;
    var acc_y = 0.0;
    var acc_s = 0.0;
    var wsum = 0.0;
    var wmax = 0.0;
    for (var oy = -r; oy <= r; oy++) {
        for (var ox = -r; ox <= r; ox++) {
            if (ox == 0 && oy == 0) { continue; }
            let q = p + vec2<i32>(ox, oy);
            var d2 = 0.0;
            for (var k = 0; k < 9; k++) {
                let diff = cp[k] - src(q + vec2<i32>(k % 3 - 1, k / 3 - 1)).x;
                d2 += diff * diff;
            }
            d2 = d2 / 9.0;
            let w = exp(-max(d2 - np.luma_floor, 0.0) * np.luma_inv_h2);
            let n = src(q);
            acc_s += w * n.x;
            acc_y += w * n.y;
            wsum += w;
            wmax = max(wmax, w);
        }
    }
    // The centre would always match itself perfectly; giving it weight 1
    // instead of the best neighbour's under-smooths textured areas.
    let c = src(p);
    let wc = max(wmax, 1e-3);
    acc_s += wc * c.x;
    acc_y += wc * c.y;
    wsum += wc;
    return vec4<f32>(acc_s / wsum, acc_y / wsum, c.z, c.w);
}

fn b3(i: i32) -> f32 {
    let a = abs(i);
    return select(select(1.0 / 16.0, 4.0 / 16.0, a == 1), 6.0 / 16.0, a == 0);
}

// One scale of edge-avoiding à-trous: a 5×5 B3 spline with holes of `step`.
// Chroma is weighted down across luma edges and across chroma differences
// larger than noise, so colour does not bleed between objects; luma (when
// `luma_scale_mix` > 0) across stabilised-luma differences larger than this
// scale's residual noise.
@fragment
fn fs_scale(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let c = src(p);
    let cs = stab_chroma(c.yzw);
    var acc = vec2<f32>(0.0);
    var wsum = 0.0;
    var lacc = vec2<f32>(0.0);
    var lsum = 0.0;
    for (var j = -2; j <= 2; j++) {
        for (var i = -2; i <= 2; i++) {
            let n = src(p + vec2<i32>(i, j) * np.step);
            let dl = n.x - c.x;
            let dc = stab_chroma(n.yzw) - cs;
            let k = b3(i) * b3(j);
            let w = k * exp(-dl * dl * np.luma_edge - dot(dc, dc) * np.chroma_edge);
            acc += w * n.zw;
            wsum += w;
            // Luma at this scale: an edge-stopped average of what the NLM
            // pass left — the coarse mottling its 9×9 window cannot reach.
            let wl = k * exp(-dl * dl * np.luma_scale_edge);
            lacc += wl * n.xy;
            lsum += wl;
        }
    }
    let l = mix(c.xy, lacc / max(lsum, 1e-8), np.luma_scale_mix);
    let uv = select(c.zw, acc / max(wsum, 1e-8), np.chroma_edge > 0.0);
    return vec4<f32>(l, uv);
}

@fragment
fn fs_compose(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let o = to_yuv(max(textureLoad(orig_tex, p, 0).rgb, vec3<f32>(0.0)) * np.wb.rgb);
    let r = src(p);
    let y = mix(o.x, r.y, np.luma_amount);
    let uv = mix(o.yz, r.zw, np.chroma_amount);
    let rgb = max(from_yuv(vec3<f32>(y, uv)), vec3<f32>(0.0)) * np.inv_wb.rgb;
    return vec4<f32>(rgb, 1.0);
}
"#;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Pipelines for the four entry points; one shared bind group layout.
#[derive(Debug)]
pub struct Denoise {
    layout: wgpu::BindGroupLayout,
    prep: wgpu::RenderPipeline,
    nlm: wgpu::RenderPipeline,
    scale: wgpu::RenderPipeline,
    compose: wgpu::RenderPipeline,
}

/// Per-image ping-pong targets and the bind groups that read them.
#[derive(Debug)]
pub struct DenoiseTargets {
    views: [wgpu::TextureView; 2],
    uniforms: wgpu::Buffer,
    /// Reading the demosaic, A, B — each with the demosaic as `orig`.
    from_base: wgpu::BindGroup,
    from: [wgpu::BindGroup; 2],
}

impl DenoiseTargets {
    pub fn view(&self, i: usize) -> &wgpu::TextureView {
        &self.views[i]
    }
}

impl Denoise {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Denoise"),
            source: wgpu::ShaderSource::Wgsl(DENOISE_SHADER.into()),
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Denoise layout"),
            entries: &[
                tex(0),
                tex(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<GpuDenoiseParams>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Denoise pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let make = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &module, entry_point: "vs_main", buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: entry,
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
            })
        };
        Self {
            prep: make("fs_prep"),
            nlm: make("fs_nlm"),
            scale: make("fs_scale"),
            compose: make("fs_compose"),
            layout,
        }
    }

    /// Allocate the ping-pong pair for an image whose demosaic is `base`.
    pub fn targets(
        &self,
        device: &wgpu::Device,
        base: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> DenoiseTargets {
        let make = || {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("Denoise target"),
                    size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let views = [make(), make()];
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Denoise uniforms"),
            size: SLOT * MAX_PASSES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = |src: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Denoise bind group"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(base) },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &uniforms,
                            offset: 0,
                            size: wgpu::BufferSize::new(std::mem::size_of::<GpuDenoiseParams>() as u64),
                        }),
                    },
                ],
            })
        };
        let from_base = bind(base);
        let from = [bind(&views[0]), bind(&views[1])];
        DenoiseTargets { views, uniforms, from_base, from }
    }

    /// Encode and submit `passes`. Returns the index (0 or 1) of the target
    /// holding the camera-RGB result.
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        targets: &DenoiseTargets,
        passes: &[(PassKind, GpuDenoiseParams)],
    ) -> usize {
        let mut slots = vec![0u8; SLOT as usize * passes.len()];
        for (i, (_, p)) in passes.iter().enumerate() {
            let bytes = bytemuck::bytes_of(p);
            slots[i * SLOT as usize..][..bytes.len()].copy_from_slice(bytes);
        }
        queue.write_buffer(&targets.uniforms, 0, &slots);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Denoise encoder"),
        });
        // `cur` = target holding the latest output; None = still the demosaic.
        let mut cur: Option<usize> = None;
        for (i, (kind, _)) in passes.iter().enumerate() {
            let out = match cur {
                Some(c) => 1 - c,
                None => 0,
            };
            let bind_group = match cur {
                Some(c) => &targets.from[c],
                None => &targets.from_base,
            };
            let pipeline = match kind {
                PassKind::Prep => &self.prep,
                PassKind::Nlm => &self.nlm,
                PassKind::Scale => &self.scale,
                PassKind::Compose => &self.compose,
            };
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Denoise pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.views[out],
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
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, bind_group, &[(i as u64 * SLOT) as u32]);
            rpass.draw(0..3, 0..1);
            drop(rpass);
            cur = Some(out);
        }
        queue.submit(Some(encoder.finish()));
        cur.unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn ai_denoise_replaces_the_classic_stage() {
        let mut p = crate::core::types::EditParams::default();
        p.luma_noise = 0.6;
        assert!(DenoiseSettings::from_params(&p).is_active(), "the default colour NR and luma are on");
        p.ai_denoise = 0.4;
        assert!(!DenoiseSettings::from_params(&p).is_active(), "AI denoise applied: the stage is skipped");
        assert_eq!(p.luma_noise, 0.6, "the sliders keep their values for when it is removed");
        p.ai_denoise = 0.0;
        assert!(DenoiseSettings::from_params(&p).is_active());
    }

    use super::*;

    #[test]
    fn denoise_shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(DENOISE_SHADER)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(DENOISE_SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("denoise shader failed validation: {e:?}"));
    }

    #[test]
    fn params_fit_one_uniform_slot() {
        let size = std::mem::size_of::<GpuDenoiseParams>();
        assert_eq!(size % 16, 0);
        assert!(size as u64 <= SLOT);
    }

    fn settings(luma: f32, color: f32) -> DenoiseSettings {
        DenoiseSettings { luma, detail: 0.5, color }
    }

    #[test]
    fn plan_always_brackets_with_prep_and_compose() {
        let p = plan(settings(0.4, 0.3), &NoiseModel::default(), [2.0, 1.0, 1.5], 3.0);
        assert_eq!(p.first().unwrap().0, PassKind::Prep);
        assert_eq!(p.last().unwrap().0, PassKind::Compose);
        assert!(p.len() <= MAX_PASSES);
    }

    #[test]
    fn disabled_halves_add_no_work() {
        // Luma only: NLM runs, and no scale touches chroma.
        let luma_only = plan(settings(0.5, 0.0), &NoiseModel::default(), [1.0; 3], 1.0);
        assert!(luma_only.iter().any(|(k, _)| *k == PassKind::Nlm));
        assert!(luma_only.iter().all(|(_, p)| p.chroma_edge == 0.0));
        // Chroma only: no NLM, and no scale mixes any luma.
        let chroma_only = plan(settings(0.0, 0.5), &NoiseModel::default(), [1.0; 3], 1.0);
        assert!(chroma_only.iter().all(|(k, _)| *k != PassKind::Nlm));
        assert!(chroma_only.iter().all(|(_, p)| p.luma_scale_mix == 0.0));
    }

    /// Coarse luma starts at the second scale (the first is the NLM's) and
    /// its edge stop tightens per scale, as the residual noise shrinks.
    #[test]
    fn coarse_luma_skips_the_first_scale_and_tightens() {
        let p = plan(settings(0.7, 0.0), &NoiseModel::default(), [1.0; 3], 1.0);
        let scales: Vec<&GpuDenoiseParams> =
            p.iter().filter(|(k, _)| *k == PassKind::Scale).map(|(_, p)| p).collect();
        assert_eq!(scales[0].luma_scale_mix, 0.0);
        for w in scales[1..].windows(2) {
            assert!(w[0].luma_scale_mix > 0.0);
            assert!(w[1].luma_scale_edge > w[0].luma_scale_edge);
        }
    }

    /// Chroma blotches are a fixed size on the sensor, so full resolution
    /// needs more scales than a 3× subsampled preview to reach them.
    #[test]
    fn full_resolution_uses_more_chroma_scales() {
        let count = |stride| {
            plan(settings(0.0, 0.5), &NoiseModel::default(), [1.0; 3], stride)
                .iter()
                .filter(|(k, _)| *k == PassKind::Scale)
                .count()
        };
        assert!(count(1.0) > count(3.0));
        assert!(count(1.0) <= MAX_LEVELS);
    }

    /// 5 (NLM window + patch) + 2·(1+2+4+8+16+32) at full resolution; the
    /// subsampled preview runs fewer scales and so reaches less far.
    #[test]
    fn reach_covers_nlm_and_every_scale() {
        assert_eq!(reach(1.0), 5 + 126);
        assert_eq!(reach(3.0), 5 + 30);
        // And it agrees with the plan it describes.
        let model = NoiseModel::default();
        let s = DenoiseSettings { luma: 0.5, detail: 0.5, color: 0.5 };
        let steps: i32 = plan(s, &model, [1.0; 3], 1.0)
            .iter()
            .filter(|(k, _)| *k == PassKind::Scale)
            .map(|(_, p)| 2 * p.step)
            .sum();
        assert_eq!(reach(1.0) as i32, NLM_RADIUS + NLM_PATCH + steps);
    }

    #[test]
    fn scales_double_their_step() {
        let p = plan(settings(0.0, 1.0), &NoiseModel::default(), [1.0; 3], 1.0);
        let steps: Vec<i32> = p.iter().filter(|(k, _)| *k == PassKind::Scale).map(|(_, p)| p.step).collect();
        for w in steps.windows(2) {
            assert_eq!(w[1], w[0] * 2);
        }
        assert_eq!(steps[0], 1);
    }
}
