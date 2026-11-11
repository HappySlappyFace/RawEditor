//! Texture and Clarity: a cached pre-pass that splits log-luminance into
//! bands, so the colour pass can scale each band with a couple of texture
//! samples.
//!
//! ## Decomposition
//! `L = log2(luma)`, then three bases, each smoother than the last:
//!
//! | base | scale (of the long edge) | built by                           |
//! |------|--------------------------|------------------------------------|
//! | B0   | `FINE`    ≈ 1/4000       | edge-avoiding à-trous (full res)   |
//! | B1   | `TEXTURE` ≈ 1/600        | edge-avoiding à-trous (full res)   |
//! | B2   | `CLARITY` ≈ 1/60         | fast guided filter of B1 (coarse)  |
//!
//! The band texture holds the running level in `x`, B0 in `y`, B1 in `z` and
//! L itself in `w`.
//!
//! Texture scales `B0 − B1` (skin, bark, foliage — but not pixel noise, which
//! is below B0), Clarity `B1 − B2` (mid-scale structure) — both by a gain on
//! linear RGB (see the colour shader): a local exposure change, so hue and
//! saturation are untouched, and scene-linear before the tone curve, so the
//! shoulder rolls off whatever it pushes past white.
//!
//! **Why log, and why it is cached.** In log space exposure is an offset and
//! white balance (nearly) one too, so the bands do not change with either
//! slider: this stage depends only on its source image and re-runs only when
//! that does (a new denoise result, a new image). The Texture and Clarity
//! sliders themselves are uniforms of the colour pass.
//!
//! **Why a guided filter for Clarity.** A plain blur as the Clarity base puts
//! a halo along every strong edge (a skyline goes bright on one side, dark on
//! the other). Edge-avoiding à-trous stops that, but at Clarity's scale its
//! taps are tens of pixels apart and jump over thin structures, which leaves
//! streaks parallel to straight, low-contrast edges. The guided filter (He,
//! Sun & Tang) instead fits `B2 ≈ a·B1 + b` locally, with plain Gaussian
//! statistics: where B1 varies far more than `CLARITY_EPS` — an edge — `a`
//! goes to 1 and B2 follows it, so the band vanishes there; where it varies
//! less, B2 is the local mean. Unlike edge stops the coefficients vary
//! smoothly with position, so nothing can streak. Like dehaze it is the *fast* variant: `a` and
//! `b` are smooth, so they are computed on a `CLARITY_GRID` grid and
//! evaluated against the full-resolution B1 in the colour pass, which keeps
//! edges sharp at a fraction of the cost. Filtering B1 rather than L keeps
//! the bands nested: texture detail never counts as an "edge" for Clarity.
//!
//! **B2 is also Highlights/Shadows' surround.** Those sliders choose their
//! gain by how bright a pixel's *surroundings* are, not the pixel: `B2 − L`
//! carries the pixel to its surround, so a region is lifted or pulled down
//! as a whole and the detail inside it (the pixel's ratio to the surround)
//! is kept. B2's edge-preservation is what keeps that from haloing: across
//! an edge stronger than `CLARITY_EPS` the surround follows the edge. So the
//! stage also runs when only Highlights or Shadows is set.
//!
//! **Scales are fractions of the image**, so the develop preview, a 100%
//! zoom and the export pick out the same features. The working texture is
//! analysed at full size up to `MAX_ANALYSIS_PIXELS`, then at half size —
//! B0 at a full-resolution 24 MP render is finer than a pixel anyway, and
//! the stage keeps two 16-bit RGBA targets alive.

use iced_wgpu::wgpu;

/// Smoothing scales as fractions of the image's long edge (σ of the
/// cumulative kernel). See the module table.
pub const FINE: f32 = 1.0 / 4000.0;
pub const TEXTURE: f32 = 1.0 / 600.0;
pub const CLARITY: f32 = 1.0 / 60.0;
/// Edge stop of the fine levels, in stops (log2 units): steps in L larger
/// than about twice this are not averaged across.
const TEXTURE_EDGE: f32 = 1.0;
/// Cells along the long edge of the Clarity grid (σ ≈ 4 cells).
pub const CLARITY_GRID: u32 = 256;
/// Guided-filter regulariser, in stops²: local structure with a standard
/// deviation well above √ε ≈ 0.7 stop is treated as an edge and keeps its
/// contrast; below it, structure is what Clarity scales. Smaller values
/// protect more and enhance less: at 0.0625 a city facade (windows ~1–2
/// stops) gained 5% mid-scale contrast at +1, at 0.5 it gains ~20%, and a
/// skyline still shows no halo.
const CLARITY_EPS: f32 = 0.5;
/// Above this many working pixels the analysis runs at half resolution.
pub const MAX_ANALYSIS_PIXELS: u32 = 12_000_000;

/// Per-pass uniforms, one 256-byte slot each (dynamic offset).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuLocalParams {
    /// Camera RGB → luma weights (as-shot WB folded in), and the log
    /// epsilon in `w` — a floor that keeps shadow noise from reading as
    /// structure in log space. Prep only.
    pub luma_w: [f32; 4],
    /// 1 / this pass's target size, for sample positions.
    pub inv_size: [f32; 2],
    /// À-trous dilation.
    pub step: i32,
    /// Bit 0: this level's result is B0; bit 1: it is B1.
    pub snap: u32,
    /// 1 / the source texture's size (down pass).
    pub src_texel: [f32; 2],
    /// 1/(2σ²) of the edge stop, σ in stops (fine levels).
    pub inv_2s2: f32,
    /// Guided-filter ε (coefficient pass).
    pub eps: f32,
    /// Bilinear taps per axis across one coarse cell (down pass).
    pub taps: i32,
    pub _pad: [f32; 3],
}

const SLOT: u64 = 256;
/// Upper bound on passes of either chain.
pub const MAX_LEVELS: usize = 8;
/// prep + fine levels + down + blur + coef + blur.
const MAX_PASSES: usize = 3 + 3 * MAX_LEVELS;

/// Analysis size for a working texture of `w`×`h`.
pub fn analysis_size(w: u32, h: u32) -> (u32, u32) {
    let (mut w, mut h) = (w, h);
    // Halve until it fits (a single halving left a 100 MP mosaic at 25 MP).
    while w.saturating_mul(h) > MAX_ANALYSIS_PIXELS && w.max(h) > 1 {
        w = w.div_ceil(2);
        h = h.div_ceil(2);
    }
    (w, h)
}

/// Clarity grid for an analysis texture of `w`×`h` (never finer than it).
pub fn coarse_size(w: u32, h: u32) -> (u32, u32) {
    let long = w.max(h).max(1);
    let scale = (CLARITY_GRID as f32 / long as f32).min(1.0);
    (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1))
}

/// À-trous steps whose cumulative σ (`sqrt(Σ step²)` — a B3 tap row has unit
/// variance per unit step) first reaches each target in turn, starting from
/// `var` already accumulated. Steps at most double (skipping a scale leaves
/// à-trous "holes" that show as a grid); a smaller final step tops σ up. A
/// target under ~¾ px is finer than the texture can express and is skipped.
/// Returns the steps and, per target, how many of them it took to reach it.
fn schedule(targets: &[f32]) -> (Vec<i32>, Vec<usize>) {
    let mut steps: Vec<i32> = Vec::new();
    let mut reached = Vec::new();
    let (mut var, mut prev) = (0.0f32, 0i32);
    for &target in targets {
        while target >= 0.75 && var.sqrt() < 0.95 * target && steps.len() < MAX_LEVELS {
            let want = (target * target - var).sqrt().round() as i32;
            let step = if prev == 0 { 1 } else { want.clamp(1, prev * 2) };
            steps.push(step);
            var += (step * step) as f32;
            prev = step;
        }
        reached.push(steps.len());
    }
    (steps, reached)
}

/// The full-resolution levels for an analysis texture whose long edge is
/// `long` pixels, with the snapshot bits on the level that reaches each of
/// B0 and B1. With no level at all before a snapshot its target is below a
/// pixel, and the prep pass's unsmoothed L — already in every channel — is
/// the right answer.
pub fn plan(long: f32, inv_size: [f32; 2]) -> Vec<GpuLocalParams> {
    let (steps, reached) = schedule(&[FINE * long, TEXTURE * long]);
    let mut levels: Vec<GpuLocalParams> = steps
        .iter()
        .map(|&step| GpuLocalParams {
            inv_size,
            step,
            inv_2s2: 1.0 / (2.0 * TEXTURE_EDGE * TEXTURE_EDGE),
            ..Default::default()
        })
        .collect();
    for (band, &n) in reached.iter().enumerate() {
        if n > 0 {
            levels[n - 1].snap |= 1 << band;
        }
    }
    levels
}

/// The coarse Clarity chain for an analysis texture of `aw`×`ah`: the down
/// pass, the Gaussian blur steps (run twice — on the moments, then on the
/// coefficients), and the coefficient pass.
pub fn coarse_plan(aw: u32, ah: u32) -> (GpuLocalParams, Vec<GpuLocalParams>, GpuLocalParams) {
    let (cw, ch) = coarse_size(aw, ah);
    let inv_size = [1.0 / cw as f32, 1.0 / ch as f32];
    // Each cell is ~k analysis pixels across; a bilinear tap covers two.
    let k = aw as f32 / cw as f32;
    let down = GpuLocalParams {
        inv_size,
        src_texel: [1.0 / aw as f32, 1.0 / ah as f32],
        taps: ((k / 2.0).ceil() as i32).clamp(1, 16),
        ..Default::default()
    };
    // The cell average already blurs by ~1/√12 of a cell.
    let sigma = (CLARITY * cw.max(ch) as f32).powi(2) - 1.0 / 12.0;
    let (steps, _) = schedule(&[sigma.max(0.0).sqrt()]);
    let blur = steps.iter().map(|&step| GpuLocalParams { inv_size, step, ..Default::default() }).collect();
    let coef = GpuLocalParams { inv_size, eps: CLARITY_EPS, ..Default::default() };
    (down, blur, coef)
}

pub const LOCAL_SHADER: &str = r#"
struct LocalParams {
    luma_w: vec4<f32>,
    inv_size: vec2<f32>,
    step: i32,
    snap: u32,
    src_texel: vec2<f32>,
    inv_2s2: f32,
    eps: f32,
    taps: i32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;
@group(0) @binding(2) var<uniform> lp: LocalParams;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(vi & 1u) * 4 - 1);
    let y = f32(i32(vi >> 1u) * 4 - 1);
    return vec4<f32>(x, -y, 0.0, 1.0);
}

// Source (camera RGB) → log2 luma, in every channel. Sampled bilinearly at
// this texel's centre, which at half size is exactly a 2×2 average. `w`
// keeps L itself through every level: Highlights/Shadows divide it out of
// the pixel to find its surround (colour shader, Step 4).
@fragment
fn fs_prep(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src_tex, src_sampler, pos.xy * lp.inv_size, 0.0).rgb;
    let l = log2(max(dot(max(c, vec3<f32>(0.0)), lp.luma_w.rgb), 0.0) + lp.luma_w.w);
    return vec4<f32>(l, l, l, l);
}

fn b3(i: i32) -> f32 {
    let a = abs(i);
    return select(select(1.0 / 16.0, 4.0 / 16.0, a == 1), 6.0 / 16.0, a == 0);
}

fn load(p: vec2<i32>) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(src_tex)) - vec2<i32>(1);
    return textureLoad(src_tex, clamp(p, vec2<i32>(0), d), 0);
}

// One edge-avoiding à-trous level on x; y and z take the B0/B1 snapshots.
@fragment
fn fs_level(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let c = load(p);
    var acc = 0.0;
    var wsum = 0.0;
    for (var j = -2; j <= 2; j++) {
        for (var i = -2; i <= 2; i++) {
            let n = load(p + vec2<i32>(i, j) * lp.step).x;
            let dl = n - c.x;
            let w = b3(i) * b3(j) * exp(-dl * dl * lp.inv_2s2);
            acc += w * n;
            wsum += w;
        }
    }
    let l = acc / max(wsum, 1e-8);
    return vec4<f32>(
        l,
        select(c.y, l, (lp.snap & 1u) != 0u),
        select(c.z, l, (lp.snap & 2u) != 0u),
        c.w,
    );
}

// Full-resolution B1 (z) → this coarse cell's mean of B1 and of B1².
@fragment
fn fs_down(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy * lp.inv_size;
    let cell = lp.inv_size;
    let n = lp.taps;
    var m = 0.0;
    var s = 0.0;
    for (var j = 0; j < n; j++) {
        for (var i = 0; i < n; i++) {
            let o = (vec2<f32>(f32(i), f32(j)) + 0.5) / f32(n) - 0.5;
            let v = textureSampleLevel(src_tex, src_sampler, uv + o * cell, 0.0).z;
            m += v;
            s += v * v;
        }
    }
    let k = f32(n * n);
    return vec4<f32>(m / k, s / k, 0.0, 0.0);
}

// Plain (Gaussian-like) à-trous on every channel: the guided filter's means.
@fragment
fn fs_blur(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    var acc = vec4<f32>(0.0);
    for (var j = -2; j <= 2; j++) {
        for (var i = -2; i <= 2; i++) {
            acc += b3(i) * b3(j) * load(p + vec2<i32>(i, j) * lp.step);
        }
    }
    return acc;
}

// Local means of B1 and B1² → guided-filter coefficients: B2 ≈ a·B1 + b.
@fragment
fn fs_coef(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(pos.xy));
    let var_ = max(c.y - c.x * c.x, 0.0);
    let a = var_ / (var_ + lp.eps);
    return vec4<f32>(a, c.x * (1.0 - a), 0.0, 0.0);
}
"#;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The pipelines and their shared layout.
#[derive(Debug)]
pub struct LocalContrast {
    layout: wgpu::BindGroupLayout,
    prep: wgpu::RenderPipeline,
    level: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    coef: wgpu::RenderPipeline,
}

/// Per-image targets: the full-resolution ping-pong pair (B0/B1) and the
/// coarse pair (Clarity coefficients), with bind groups reading each.
#[derive(Debug)]
pub struct LocalTargets {
    views: [wgpu::TextureView; 2],
    coarse: [wgpu::TextureView; 2],
    uniforms: wgpu::Buffer,
    from: [wgpu::BindGroup; 2],
    from_coarse: [wgpu::BindGroup; 2],
}

impl LocalTargets {
    pub fn view(&self, i: usize) -> &wgpu::TextureView {
        &self.views[i]
    }

    pub fn coarse_view(&self, i: usize) -> &wgpu::TextureView {
        &self.coarse[i]
    }
}

/// Which targets hold a run's results.
#[derive(Debug, Clone, Copy)]
pub struct LocalOutput {
    /// `(B1, B0, B1, -)` at analysis size.
    pub bands: usize,
    /// `(a, b, -, -)` on the Clarity grid: B2 = a·B1 + b.
    pub clarity: usize,
}

fn target(device: &wgpu::Device, label: &str, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

impl LocalContrast {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Local contrast"),
            source: wgpu::ShaderSource::Wgsl(LOCAL_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Local contrast layout"),
            entries: &[
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<GpuLocalParams>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Local contrast pipeline layout"),
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
            level: make("fs_level"),
            down: make("fs_down"),
            blur: make("fs_blur"),
            coef: make("fs_coef"),
            layout,
        }
    }

    fn bind(
        &self,
        device: &wgpu::Device,
        src: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        uniforms: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Local contrast bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: uniforms,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<GpuLocalParams>() as u64),
                    }),
                },
            ],
        })
    }

    /// Allocate the targets for an analysis texture of `width`×`height`.
    pub fn targets(&self, device: &wgpu::Device, sampler: &wgpu::Sampler, width: u32, height: u32) -> LocalTargets {
        let (cw, ch) = coarse_size(width, height);
        let views = [0, 1].map(|_| target(device, "Local contrast bands", width, height));
        let coarse = [0, 1].map(|_| target(device, "Clarity coefficients", cw, ch));
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Local contrast uniforms"),
            size: SLOT * MAX_PASSES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let from = [0, 1].map(|i| self.bind(device, &views[i], sampler, &uniforms));
        let from_coarse = [0, 1].map(|i| self.bind(device, &coarse[i], sampler, &uniforms));
        LocalTargets { views, coarse, uniforms, from, from_coarse }
    }

    /// Encode and submit the whole stage reading `source` (camera RGB at
    /// working size), for an analysis texture of `aw`×`ah`.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: &wgpu::Sampler,
        targets: &LocalTargets,
        source: &wgpu::TextureView,
        luma_w: [f32; 4],
        (aw, ah): (u32, u32),
    ) -> LocalOutput {
        let inv_size = [1.0 / aw as f32, 1.0 / ah as f32];
        let prep = GpuLocalParams { luma_w, inv_size, ..Default::default() };
        let levels = plan(aw.max(ah) as f32, inv_size);
        let (down, blur, coef) = coarse_plan(aw, ah);

        // (pipeline, reads, writes, uniforms) in order. Each side is the
        // source, the full-resolution pair or the coarse pair.
        #[derive(Clone, Copy, PartialEq)]
        enum Tex {
            Source,
            Bands,
            Coarse,
        }
        let mut passes: Vec<(&wgpu::RenderPipeline, Tex, Tex, GpuLocalParams)> =
            vec![(&self.prep, Tex::Source, Tex::Bands, prep)];
        passes.extend(levels.iter().map(|l| (&self.level, Tex::Bands, Tex::Bands, *l)));
        passes.push((&self.down, Tex::Bands, Tex::Coarse, down));
        passes.extend(blur.iter().map(|l| (&self.blur, Tex::Coarse, Tex::Coarse, *l)));
        passes.push((&self.coef, Tex::Coarse, Tex::Coarse, coef));
        passes.extend(blur.iter().map(|l| (&self.blur, Tex::Coarse, Tex::Coarse, *l)));

        let mut slots = vec![0u8; SLOT as usize * passes.len()];
        for (i, (_, _, _, p)) in passes.iter().enumerate() {
            let bytes = bytemuck::bytes_of(p);
            slots[i * SLOT as usize..][..bytes.len()].copy_from_slice(bytes);
        }
        queue.write_buffer(&targets.uniforms, 0, &slots);
        let from_source = self.bind(device, source, sampler, &targets.uniforms);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Local contrast encoder"),
        });
        // Latest output on each pair; `None` = nothing written yet.
        let (mut bands, mut coarse): (Option<usize>, Option<usize>) = (None, None);
        for (i, &(pipeline, reads, writes, _)) in passes.iter().enumerate() {
            let bind_group = match reads {
                Tex::Source => &from_source,
                Tex::Bands => &targets.from[bands.unwrap_or(0)],
                Tex::Coarse => &targets.from_coarse[coarse.unwrap_or(0)],
            };
            let (latest, views) = if writes == Tex::Coarse {
                (&mut coarse, &targets.coarse)
            } else {
                (&mut bands, &targets.views)
            };
            let dst = latest.map_or(0, |c| 1 - c);
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Local contrast pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &views[dst],
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, bind_group, &[(i as u64 * SLOT) as u32]);
            rpass.draw(0..3, 0..1);
            drop(rpass);
            *latest = Some(dst);
        }
        queue.submit(Some(encoder.finish()));
        LocalOutput { bands: bands.unwrap_or(0), clarity: coarse.unwrap_or(0) }
    }
}

/// A 1×1 stand-in bound while Texture and Clarity are both zero.
pub fn placeholder(device: &wgpu::Device) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("Local contrast (unused)"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(LOCAL_SHADER)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(LOCAL_SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("local contrast shader failed validation: {e:?}"));
    }

    /// Mirrors the WGSL struct; the uniform is read at these offsets.
    #[test]
    fn params_layout_matches_wgsl() {
        let size = std::mem::size_of::<GpuLocalParams>();
        assert_eq!(size, 64);
        assert!(size as u64 <= SLOT);
        assert_eq!(std::mem::offset_of!(GpuLocalParams, src_texel), 32);
        assert_eq!(std::mem::offset_of!(GpuLocalParams, taps), 48);
    }

    fn sigma(steps: &[GpuLocalParams]) -> f32 {
        steps.iter().map(|l| (l.step * l.step) as f32).sum::<f32>().sqrt()
    }

    /// B1's snapshot must sit at its target scale, and Clarity's blur at its
    /// own — on any size of render. This is what makes the preview and the
    /// export enhance the same features.
    #[test]
    fn bands_land_on_their_scales() {
        for long in [1280u32, 2560, 3000, 4128] {
            let p = plan(long as f32, [1.0; 2]);
            let b1 = p.iter().position(|l| l.snap & 2 != 0).expect("B1 snapshot");
            let s1 = sigma(&p[..=b1]);
            assert!((s1 / (TEXTURE * long as f32) - 1.0).abs() < 0.25, "{long}: texture σ {s1}");

            let (aw, ah) = (long, long * 2 / 3);
            let (_, blur, _) = coarse_plan(aw, ah);
            let (cw, _) = coarse_size(aw, ah);
            // In analysis pixels, including the cell average's own blur.
            let sc = (sigma(&blur).powi(2) + 1.0 / 12.0).sqrt() * aw as f32 / cw as f32;
            assert!((sc / (CLARITY * long as f32) - 1.0).abs() < 0.15, "{long}: clarity σ {sc}");
        }
    }

    #[test]
    fn steps_never_skip_a_scale() {
        for target in [2.0f32, 10.0, 40.0] {
            let (steps, _) = schedule(&[target]);
            assert_eq!(steps[0], 1);
            for w in steps.windows(2) {
                assert!(w[1] <= w[0] * 2, "{steps:?}");
            }
        }
    }

    /// At a small preview B0 is finer than a pixel: no level is spent on it
    /// and the unsmoothed L stands in.
    #[test]
    fn sub_pixel_fine_band_costs_nothing() {
        assert!(plan(1280.0, [1.0; 2]).iter().all(|l| l.snap & 1 == 0));
        assert_eq!(plan(8000.0, [1.0; 2]).iter().filter(|l| l.snap & 1 != 0).count(), 1);
    }

    /// The coarse grid is fixed in proportion to the image, so the guided
    /// filter sees the same neighbourhoods at every resolution.
    #[test]
    fn clarity_grid_is_resolution_independent() {
        assert_eq!(coarse_size(1280, 853), coarse_size(3000, 2000));
        assert_eq!(coarse_size(3000, 2000).0, CLARITY_GRID);
        assert_eq!(coarse_size(100, 50), (100, 50), "never finer than the analysis");
    }

    #[test]
    fn large_renders_analyse_at_half_size() {
        assert_eq!(analysis_size(2560, 1700), (2560, 1700));
        assert_eq!(analysis_size(6000, 4000), (3000, 2000));
        assert_eq!(analysis_size(6001, 4001), (3001, 2001));
        // A 100 MP mosaic halves twice, so the cap really is a cap.
        assert_eq!(analysis_size(10000, 10000), (2500, 2500));
    }
}
