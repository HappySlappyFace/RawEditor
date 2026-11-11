//! Pass 1: RCD demosaicing (Ratio Corrected Demosaicing, Luis Sanz
//! Rodríguez — the algorithm darktable and RawTherapee default to).
//!
//! Replaces the old single-pass gradient-bilinear debayer, whose 3×3 window
//! could only choose between a handful of axis-aligned averages: it zippered
//! on fine diagonal detail and put false colour on every high-contrast edge.
//! RCD estimates green from *ratio-corrected* neighbours (a low-pass of the
//! local mosaic says how fast the colour ratio changes, so the estimate tracks
//! texture instead of smearing it), picks the interpolation direction from a
//! 9-tap high-pass discrimination rather than a single gradient, and fills the
//! red/blue planes from colour DIFFERENCES weighted by diagonal and cardinal
//! gradients. The result is markedly sharper with far less false colour.
//!
//! RCD's steps each read neighbourhoods of the previous step's output, so it
//! runs as four fragment passes into transient textures:
//!
//! | pass        | reads              | writes                                   |
//! |-------------|--------------------|------------------------------------------|
//! | `fs_dirs`   | mosaic             | `dirs`: (VH_Dir, PQ_Dir, LPF) Rgba16F    |
//! | `fs_green`  | mosaic, dirs       | `green`: G everywhere, R32F              |
//! | `fs_opp`    | mosaic, dirs, green| `opp`: B at R sites / R at B sites, R32F |
//! | `fs_final`  | all of the above   | camera-native RGB, Rgba16F (the output)  |
//!
//! Only the final texture outlives the call. This still runs exactly once per
//! image load (it has no edit-dependent inputs), so its cost never reaches a
//! slider drag.
//!
//! **White balance.** Colour-difference and colour-ratio methods assume a
//! neutral surface has equal channels — darktable and RawTherapee both
//! demosaic after white balance for that reason. The mosaic is therefore
//! scaled by the as-shot multipliers on the way in and divided back out on
//! the way out: the output stays camera-native, so the Temp/Tint sliders keep
//! working without ever re-running this pass.
//!
//! **Borders** are handled by mirror addressing that preserves CFA parity
//! (`-x` and `2(n-1)-x` keep the parity of `x`), so the full algorithm runs
//! right up to the edge instead of needing a separate border interpolator.

use iced_wgpu::wgpu;
use wgpu::util::DeviceExt;

/// Demosaic uniforms. Everything per-site is pre-resolved on the CPU into
/// "tile position" form, `k = (y & 1) * 2 + (x & 1)`, so the shader never has
/// to know which CFA pattern it is looking at beyond one packed lookup.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuDemosaicParams {
    /// Black level (metadata + user offset) of each tile position, raw units.
    pub black: [f32; 4],
    /// `wb[colour(k)] / (white - black[k])`: one multiply takes a raw value
    /// to a white-balanced, unit-range sample.
    pub scale: [f32; 4],
    /// White-balanced ceiling of each colour (r, g, b, _): the largest value
    /// the sensor can record in that channel. Interpolated values are capped
    /// here so ratio estimates cannot overshoot past clipping.
    pub ceil: [f32; 4],
    /// Reciprocal WB per colour (r, g, b, _), applied on output so the result
    /// is camera-native again.
    pub inv_wb: [f32; 4],
    /// Colour of each tile position, 2 bits each (0 = R, 1 = G, 2 = B).
    pub colors: u32,
    pub _pad: [u32; 3],
}

/// CFA colour at each tile position for the loader's pattern ids
/// (0 RGGB, 1 GRBG, 2 GBRG, 3 BGGR).
pub fn tile_colors(cfa_pattern: u32) -> [u32; 4] {
    match cfa_pattern {
        1 => [1, 0, 2, 1],
        2 => [1, 2, 0, 1],
        3 => [2, 1, 1, 0],
        _ => [0, 1, 1, 2],
    }
}

/// Which `[R, G1, G2, B]` black-level slot each tile position reads. This is
/// the mapping the old single-pass debayer used, kept verbatim so per-channel
/// black levels (and the sensor-correction offsets) land on the same sites.
pub fn tile_black_slots(cfa_pattern: u32) -> [usize; 4] {
    match cfa_pattern {
        1 => [1, 0, 3, 2],
        2 => [1, 3, 0, 2],
        3 => [3, 1, 2, 0],
        _ => [0, 1, 2, 3],
    }
}

impl GpuDemosaicParams {
    pub fn new(
        cfa_pattern: u32,
        black_levels: [u32; 4],
        black_offsets: [f32; 4],
        white_level: u32,
        wb_multipliers: [f32; 4],
    ) -> Self {
        // Normalise to green = 1 so the green ceiling is exactly the sensor's.
        let g = if wb_multipliers[1] > 0.0 { wb_multipliers[1] } else { 1.0 };
        let wb = [
            (wb_multipliers[0] / g).clamp(0.05, 20.0),
            1.0,
            (wb_multipliers[2] / g).clamp(0.05, 20.0),
        ];
        let colors = tile_colors(cfa_pattern);
        let slots = tile_black_slots(cfa_pattern);
        let mut black = [0.0f32; 4];
        let mut scale = [0.0f32; 4];
        for k in 0..4 {
            let b = black_levels[slots[k]] as f32 + black_offsets[slots[k]];
            black[k] = b;
            let range = (white_level as f32 - b).max(1.0);
            scale[k] = wb[colors[k] as usize] / range;
        }
        let packed = colors
            .iter()
            .enumerate()
            .fold(0u32, |acc, (k, &c)| acc | (c << (2 * k)));
        Self {
            black,
            scale,
            ceil: [wb[0], wb[1], wb[2], 0.0],
            inv_wb: [1.0 / wb[0], 1.0 / wb[1], 1.0 / wb[2], 0.0],
            colors: packed,
            _pad: [0; 3],
        }
    }
}

pub const DEMOSAIC_SHADER: &str = r#"
struct DemosaicParams {
    black: vec4<f32>,
    scale: vec4<f32>,
    ceil: vec4<f32>,
    inv_wb: vec4<f32>,
    colors: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

@group(0) @binding(0) var raw_tex: texture_2d<u32>;
@group(0) @binding(1) var dirs_tex: texture_2d<f32>;
@group(0) @binding(2) var green_tex: texture_2d<f32>;
@group(0) @binding(3) var opp_tex: texture_2d<f32>;
@group(0) @binding(4) var<uniform> dp: DemosaicParams;

// RCD's own constants, for data normalised to [0, 1].
const EPS: f32 = 1e-5;
const EPSSQ: f32 = 1e-10;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(vi & 1u) * 4 - 1);
    let y = f32(i32(vi >> 1u) * 4 - 1);
    return vec4<f32>(x, -y, 0.0, 1.0);
}

// Reflect out-of-range coordinates about the edge pixel. Both reflections
// preserve parity, so the CFA colour of a mirrored site is still correct.
fn mirror1(x: i32, n: i32) -> i32 {
    var v = x;
    if (v < 0) { v = -v; }
    if (v >= n) { v = 2 * (n - 1) - v; }
    return clamp(v, 0, n - 1);
}

fn mirror(p: vec2<i32>) -> vec2<i32> {
    let d = vec2<i32>(textureDimensions(raw_tex));
    return vec2<i32>(mirror1(p.x, d.x), mirror1(p.y, d.y));
}

fn tile(p: vec2<i32>) -> u32 {
    return u32(p.y & 1) * 2u + u32(p.x & 1);
}

// CFA colour at p: 0 = R, 1 = G, 2 = B.
fn fc(p: vec2<i32>) -> u32 {
    return (dp.colors >> (2u * tile(p))) & 3u;
}

// White-balanced, unit-range mosaic sample, capped at its colour's ceiling.
// Sensors can read past the metadata white level (a Nikon D3300 piles its
// clipped photosites up at 4095 against a white of 3880): left uncapped,
// clipped native sites sat ~6% above the interpolated ones, which ARE
// capped, so a blown area held a mosaic pattern and its clipped channels
// read brighter than their ceiling — biasing highlight reconstruction
// towards whichever channel overshot.
fn cfa(p: vec2<i32>) -> f32 {
    let q = mirror(p);
    let raw = f32(textureLoad(raw_tex, q, 0).r);
    let k = tile(q);
    let colour = (dp.colors >> (2u * k)) & 3u;
    return min(max(raw - dp.black[k], 0.0) * dp.scale[k], dp.ceil[colour]);
}

fn dirs(p: vec2<i32>) -> vec4<f32> {
    return textureLoad(dirs_tex, mirror(p), 0);
}

fn green(p: vec2<i32>) -> f32 {
    return textureLoad(green_tex, mirror(p), 0).r;
}

// Colour `c` (0 = R or 2 = B) at a red or blue site: the mosaic itself where
// that is the site's own colour, otherwise the fs_opp estimate.
fn rb(p: vec2<i32>, c: u32) -> f32 {
    if (fc(p) == c) {
        return cfa(p);
    }
    return textureLoad(opp_tex, mirror(p), 0).r;
}

// Square of RCD's colour-difference high-pass along one line of 7 samples.
fn hpf(m3: f32, m2: f32, m1: f32, c: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    let t = (m3 - m1 - p1 + p3) - 3.0 * (m2 + p2) + 6.0 * c;
    return t * t;
}

// Prefer the neighbourhood's direction when it is more decisive than the
// centre's own (RCD's "refined" discrimination).
fn refine(central: f32, neighbourhood: f32) -> f32 {
    if (abs(0.5 - central) < abs(0.5 - neighbourhood)) {
        return neighbourhood;
    }
    return central;
}

// ── Step 1: direction discrimination + low-pass ─────────────────────────────
// r: VH_Dir (0 = interpolate vertically, 1 = horizontally) at every site.
// g: PQ_Dir (0 = NW-SE diagonal, 1 = NE-SW) at red/blue sites.
// b: RCD's 3×3 low-pass of the mosaic at red/blue sites.
@fragment
fn fs_dirs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    var v: array<f32, 9>;
    var h: array<f32, 9>;
    for (var i = 0; i < 9; i++) {
        v[i] = cfa(p + vec2<i32>(0, i - 4));
        h[i] = cfa(p + vec2<i32>(i - 4, 0));
    }
    let v_stat = max(EPSSQ,
        hpf(v[0], v[1], v[2], v[3], v[4], v[5], v[6]) +
        hpf(v[1], v[2], v[3], v[4], v[5], v[6], v[7]) +
        hpf(v[2], v[3], v[4], v[5], v[6], v[7], v[8]));
    let h_stat = max(EPSSQ,
        hpf(h[0], h[1], h[2], h[3], h[4], h[5], h[6]) +
        hpf(h[1], h[2], h[3], h[4], h[5], h[6], h[7]) +
        hpf(h[2], h[3], h[4], h[5], h[6], h[7], h[8]));
    let vh = v_stat / (v_stat + h_stat);

    if (fc(p) == 1u) {
        return vec4<f32>(vh, 0.5, 0.0, 0.0);
    }

    // P runs NW→SE, Q runs NE→SW.
    var d: array<f32, 9>;
    var q: array<f32, 9>;
    for (var i = 0; i < 9; i++) {
        d[i] = cfa(p + vec2<i32>(i - 4, i - 4));
        q[i] = cfa(p + vec2<i32>(4 - i, i - 4));
    }
    let p_stat = max(EPSSQ,
        hpf(d[0], d[1], d[2], d[3], d[4], d[5], d[6]) +
        hpf(d[1], d[2], d[3], d[4], d[5], d[6], d[7]) +
        hpf(d[2], d[3], d[4], d[5], d[6], d[7], d[8]));
    let q_stat = max(EPSSQ,
        hpf(q[0], q[1], q[2], q[3], q[4], q[5], q[6]) +
        hpf(q[1], q[2], q[3], q[4], q[5], q[6], q[7]) +
        hpf(q[2], q[3], q[4], q[5], q[6], q[7], q[8]));
    let pq = p_stat / (p_stat + q_stat);

    let lpf = v[4]
        + 0.5 * (v[3] + v[5] + h[3] + h[5])
        + 0.25 * (cfa(p + vec2<i32>(-1, -1)) + cfa(p + vec2<i32>(1, -1))
                + cfa(p + vec2<i32>(-1, 1)) + cfa(p + vec2<i32>(1, 1)));
    return vec4<f32>(vh, pq, lpf, 0.0);
}

// ── Step 2: green at red/blue sites ─────────────────────────────────────────
@fragment
fn fs_green(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let c0 = cfa(p);
    if (fc(p) == 1u) {
        return vec4<f32>(c0, 0.0, 0.0, 0.0);
    }

    let vh_disc = refine(
        dirs(p).r,
        0.25 * (dirs(p + vec2<i32>(-1, -1)).r + dirs(p + vec2<i32>(1, -1)).r
              + dirs(p + vec2<i32>(-1, 1)).r + dirs(p + vec2<i32>(1, 1)).r));

    var n: array<f32, 5>;
    var s: array<f32, 5>;
    var w: array<f32, 5>;
    var e: array<f32, 5>;
    n[0] = c0; s[0] = c0; w[0] = c0; e[0] = c0;
    for (var i = 1; i < 5; i++) {
        n[i] = cfa(p + vec2<i32>(0, -i));
        s[i] = cfa(p + vec2<i32>(0, i));
        w[i] = cfa(p + vec2<i32>(-i, 0));
        e[i] = cfa(p + vec2<i32>(i, 0));
    }

    let n_grad = EPS + abs(n[1] - s[1]) + abs(n[0] - n[2]) + abs(n[1] - n[3]) + abs(n[2] - n[4]);
    let s_grad = EPS + abs(n[1] - s[1]) + abs(s[0] - s[2]) + abs(s[1] - s[3]) + abs(s[2] - s[4]);
    let w_grad = EPS + abs(w[1] - e[1]) + abs(w[0] - w[2]) + abs(w[1] - w[3]) + abs(w[2] - w[4]);
    let e_grad = EPS + abs(w[1] - e[1]) + abs(e[0] - e[2]) + abs(e[1] - e[3]) + abs(e[2] - e[4]);

    // Ratio-corrected cardinal estimates: the neighbour's green scaled by how
    // the local low-pass changes between it and the centre.
    let lpf0 = dirs(p).b;
    let n_est = n[1] * (2.0 * lpf0) / (EPS + lpf0 + dirs(p + vec2<i32>(0, -2)).b);
    let s_est = s[1] * (2.0 * lpf0) / (EPS + lpf0 + dirs(p + vec2<i32>(0, 2)).b);
    let w_est = w[1] * (2.0 * lpf0) / (EPS + lpf0 + dirs(p + vec2<i32>(-2, 0)).b);
    let e_est = e[1] * (2.0 * lpf0) / (EPS + lpf0 + dirs(p + vec2<i32>(2, 0)).b);

    let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
    let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);
    let g = mix(v_est, h_est, vh_disc);
    return vec4<f32>(clamp(g, 0.0, dp.ceil.g), 0.0, 0.0, 0.0);
}

// ── Step 3: blue at red sites, red at blue sites ────────────────────────────
@fragment
fn fs_opp(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let own = fc(p);
    if (own == 1u) {
        return vec4<f32>(0.0);
    }
    let c = 2u - own; // the colour to estimate

    let pq_disc = refine(
        dirs(p).g,
        0.25 * (dirs(p + vec2<i32>(-1, -1)).g + dirs(p + vec2<i32>(1, -1)).g
              + dirs(p + vec2<i32>(-1, 1)).g + dirs(p + vec2<i32>(1, 1)).g));

    // Diagonal neighbours at distance 1 and 3 are native `c` sites.
    let nw1 = cfa(p + vec2<i32>(-1, -1));
    let ne1 = cfa(p + vec2<i32>(1, -1));
    let sw1 = cfa(p + vec2<i32>(-1, 1));
    let se1 = cfa(p + vec2<i32>(1, 1));
    let g0 = green(p);

    let nw_grad = EPS + abs(nw1 - se1) + abs(nw1 - cfa(p + vec2<i32>(-3, -3))) + abs(g0 - green(p + vec2<i32>(-2, -2)));
    let ne_grad = EPS + abs(ne1 - sw1) + abs(ne1 - cfa(p + vec2<i32>(3, -3))) + abs(g0 - green(p + vec2<i32>(2, -2)));
    let sw_grad = EPS + abs(ne1 - sw1) + abs(sw1 - cfa(p + vec2<i32>(-3, 3))) + abs(g0 - green(p + vec2<i32>(-2, 2)));
    let se_grad = EPS + abs(nw1 - se1) + abs(se1 - cfa(p + vec2<i32>(3, 3))) + abs(g0 - green(p + vec2<i32>(2, 2)));

    let nw_est = nw1 - green(p + vec2<i32>(-1, -1));
    let ne_est = ne1 - green(p + vec2<i32>(1, -1));
    let sw_est = sw1 - green(p + vec2<i32>(-1, 1));
    let se_est = se1 - green(p + vec2<i32>(1, 1));

    let p_est = (nw_grad * se_est + se_grad * nw_est) / (nw_grad + se_grad);
    let q_est = (ne_grad * sw_est + sw_grad * ne_est) / (ne_grad + sw_grad);
    let v = g0 + mix(p_est, q_est, pq_disc);
    return vec4<f32>(clamp(v, 0.0, dp.ceil[c]), 0.0, 0.0, 0.0);
}

// ── Step 4: red and blue at green sites; assemble the output ────────────────
@fragment
fn fs_final(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let own = fc(p);
    let g0 = green(p);
    var rgb: vec3<f32>;

    if (own == 0u) {
        rgb = vec3<f32>(cfa(p), g0, rb(p, 2u));
    } else if (own == 2u) {
        rgb = vec3<f32>(rb(p, 0u), g0, cfa(p));
    } else {
        let vh_disc = refine(
            dirs(p).r,
            0.25 * (dirs(p + vec2<i32>(-1, -1)).r + dirs(p + vec2<i32>(1, -1)).r
                  + dirs(p + vec2<i32>(-1, 1)).r + dirs(p + vec2<i32>(1, 1)).r));

        let gn = green(p + vec2<i32>(0, -1));
        let gs = green(p + vec2<i32>(0, 1));
        let gw = green(p + vec2<i32>(-1, 0));
        let ge = green(p + vec2<i32>(1, 0));
        let n1 = EPS + abs(g0 - green(p + vec2<i32>(0, -2)));
        let s1 = EPS + abs(g0 - green(p + vec2<i32>(0, 2)));
        let w1 = EPS + abs(g0 - green(p + vec2<i32>(-2, 0)));
        let e1 = EPS + abs(g0 - green(p + vec2<i32>(2, 0)));

        var out = array<f32, 2>(0.0, 0.0);
        for (var i = 0u; i < 2u; i++) {
            let c = i * 2u; // 0 = R, 2 = B
            let cn = rb(p + vec2<i32>(0, -1), c);
            let cs = rb(p + vec2<i32>(0, 1), c);
            let cw = rb(p + vec2<i32>(-1, 0), c);
            let ce = rb(p + vec2<i32>(1, 0), c);
            let sn_abs = abs(cn - cs);
            let ew_abs = abs(cw - ce);
            let n_grad = n1 + sn_abs + abs(cn - rb(p + vec2<i32>(0, -3), c));
            let s_grad = s1 + sn_abs + abs(cs - rb(p + vec2<i32>(0, 3), c));
            let w_grad = w1 + ew_abs + abs(cw - rb(p + vec2<i32>(-3, 0), c));
            let e_grad = e1 + ew_abs + abs(ce - rb(p + vec2<i32>(3, 0), c));

            let v_est = (n_grad * (cs - gs) + s_grad * (cn - gn)) / (n_grad + s_grad);
            let h_est = (e_grad * (cw - gw) + w_grad * (ce - ge)) / (e_grad + w_grad);
            out[i] = clamp(g0 + mix(v_est, h_est, vh_disc), 0.0, dp.ceil[c]);
        }
        rgb = vec3<f32>(out[0], g0, out[1]);
    }

    // Back to camera-native: the colour pass applies the user's WB itself.
    return vec4<f32>(rgb * dp.inv_wb.rgb, 1.0);
}
"#;

/// Pipelines + layouts for the four RCD passes. Created once with the
/// `SharedContext`.
#[derive(Debug)]
pub struct Demosaic {
    dirs: Pass,
    green: Pass,
    opp: Pass,
    fin: Pass,
}

#[derive(Debug)]
struct Pass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

/// Formats of the transient textures, in pass order.
const DIRS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const PLANE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// Output format — what the colour pass samples.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

const UNFILTERED: wgpu::TextureSampleType = wgpu::TextureSampleType::Float { filterable: false };

impl Demosaic {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RCD Demosaic"),
            source: wgpu::ShaderSource::Wgsl(DEMOSAIC_SHADER.into()),
        });

        let uniform = wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let raw = texture_entry(0, wgpu::TextureSampleType::Uint);
        let dirs = texture_entry(1, UNFILTERED);
        let green = texture_entry(2, UNFILTERED);
        let opp = texture_entry(3, UNFILTERED);

        let make = |label: &str, entry: &str, entries: &[wgpu::BindGroupLayoutEntry], format| {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &module, entry_point: "vs_main", buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: entry,
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
            });
            Pass { pipeline, layout }
        };

        Self {
            dirs: make("RCD dirs", "fs_dirs", &[raw, uniform], DIRS_FORMAT),
            green: make("RCD green", "fs_green", &[raw, dirs, uniform], PLANE_FORMAT),
            opp: make("RCD opp", "fs_opp", &[raw, dirs, green, uniform], PLANE_FORMAT),
            fin: make("RCD final", "fs_final", &[raw, dirs, green, opp, uniform], OUTPUT_FORMAT),
        }
    }

    /// Demosaic `raw` (R16Uint, `width`×`height`) into `output` (an
    /// `OUTPUT_FORMAT` render target of the same size), and submit.
    ///
    /// The three intermediate textures are dropped on return; wgpu reclaims
    /// them at the next `poll`, which every render path performs.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        raw: &wgpu::TextureView,
        width: u32,
        height: u32,
        params: &GpuDemosaicParams,
        output: &wgpu::TextureView,
    ) {
        let make_target = |label: &str, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let dirs = make_target("RCD dirs", DIRS_FORMAT);
        let green = make_target("RCD green", PLANE_FORMAT);
        let opp = make_target("RCD opp", PLANE_FORMAT);

        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("RCD params"),
            contents: bytemuck::bytes_of(params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind = |pass: &Pass, views: &[(u32, &wgpu::TextureView)]| {
            let mut entries: Vec<wgpu::BindGroupEntry> = views
                .iter()
                .map(|(binding, view)| wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: wgpu::BindingResource::TextureView(view),
                })
                .collect();
            entries.push(wgpu::BindGroupEntry { binding: 4, resource: uniform.as_entire_binding() });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RCD bind group"),
                layout: &pass.layout,
                entries: &entries,
            })
        };
        let bg_dirs = bind(&self.dirs, &[(0, raw)]);
        let bg_green = bind(&self.green, &[(0, raw), (1, &dirs)]);
        let bg_opp = bind(&self.opp, &[(0, raw), (1, &dirs), (2, &green)]);
        let bg_fin = bind(&self.fin, &[(0, raw), (1, &dirs), (2, &green), (3, &opp)]);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("RCD encoder"),
        });
        for (pass, bind_group, target) in [
            (&self.dirs, &bg_dirs, &dirs),
            (&self.green, &bg_green, &green),
            (&self.opp, &bg_opp, &opp),
            (&self.fin, &bg_fin, output),
        ] {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("RCD pass"),
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
            rpass.set_pipeline(&pass.pipeline);
            rpass.set_bind_group(0, bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
        queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demosaic_shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(DEMOSAIC_SHADER)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(DEMOSAIC_SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("demosaic shader failed validation: {e:?}"));
    }

    #[test]
    fn uniform_is_16_byte_aligned() {
        assert_eq!(std::mem::size_of::<GpuDemosaicParams>() % 16, 0);
        assert_eq!(std::mem::size_of::<GpuDemosaicParams>(), 80);
    }

    /// The packed colour code must agree with the CFA pattern names, site by
    /// site — a slip here demosaics every image with swapped channels.
    #[test]
    fn packed_colours_match_the_pattern_names() {
        for (pattern, name) in [(0, "RGGB"), (1, "GRBG"), (2, "GBRG"), (3, "BGGR")] {
            let p = GpuDemosaicParams::new(pattern, [0; 4], [0.0; 4], 1000, [1.0; 4]);
            for (k, ch) in name.chars().enumerate() {
                let colour = (p.colors >> (2 * k)) & 3;
                let expect = match ch { 'R' => 0, 'G' => 1, _ => 2 };
                assert_eq!(colour, expect, "{name} site {k}");
            }
        }
    }

    /// Black levels are stored `[R, G1, G2, B]`; each tile position must read
    /// the slot of its own colour.
    #[test]
    fn black_levels_follow_the_site_colour() {
        let black = [100, 200, 300, 400]; // R, G1, G2, B
        for pattern in 0..4 {
            let p = GpuDemosaicParams::new(pattern, black, [0.0; 4], 4000, [1.0; 4]);
            let colours = tile_colors(pattern);
            for (k, colour) in colours.iter().enumerate() {
                match colour {
                    0 => assert_eq!(p.black[k], 100.0, "pattern {pattern} R"),
                    2 => assert_eq!(p.black[k], 400.0, "pattern {pattern} B"),
                    _ => assert!(p.black[k] == 200.0 || p.black[k] == 300.0, "pattern {pattern} G"),
                }
            }
        }
    }

    #[test]
    fn white_balance_scales_in_and_out() {
        let p = GpuDemosaicParams::new(0, [0; 4], [0.0; 4], 1000, [2.0, 1.0, 1.5, 1.0]);
        // Site 0 is red: raw 1000 → 1.0 × wb_r.
        assert!((1000.0 * p.scale[0] - 2.0).abs() < 1e-6);
        assert_eq!(p.ceil, [2.0, 1.0, 1.5, 0.0]);
        for c in 0..3 {
            assert!((p.ceil[c] * p.inv_wb[c] - 1.0).abs() < 1e-6);
        }
    }
    /// Synthetic scene with a known answer: gratings at several frequencies
    /// and orientations, a saturated slanted edge and smooth colour ramps.
    fn scene(w: usize, h: usize) -> Vec<[f32; 3]> {
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32, y as f32);
                let base = [0.25 + 0.5 * fx / w as f32, 0.4, 0.25 + 0.5 * fy / h as f32];
                let region = (x * 4 / w, y * 2 / h);
                let v = match region {
                    // Gratings: horizontal, vertical, diagonal, fine diagonal.
                    (0, 0) => 0.5 + 0.4 * (fx * 0.6).sin(),
                    (1, 0) => 0.5 + 0.4 * (fy * 0.6).sin(),
                    (2, 0) => 0.5 + 0.4 * ((fx + fy) * 0.45).sin(),
                    (3, 0) => 0.5 + 0.4 * ((fx - fy) * 0.9).sin(),
                    // Slanted colour edge (red on cyan).
                    (0, 1) | (1, 1) => {
                        if fx * 0.97 + fy * 0.24 > w as f32 * 0.25 + h as f32 * 0.6 { 1.0 } else { 0.0 }
                    }
                    _ => 0.5,
                };
                let px = match region {
                    (0, 1) | (1, 1) => {
                        if v > 0.5 { [0.8, 0.15, 0.1] } else { [0.1, 0.6, 0.7] }
                    }
                    (_, 0) => [base[0] * v * 1.4, base[1] * v * 1.4, base[2] * v * 1.4],
                    _ => base,
                };
                out.push(px);
            }
        }
        out
    }

    fn psnr(a: &[[f32; 3]], b: &[[f32; 3]], w: usize, h: usize) -> f64 {
        // Skip a border: neither method is judged on its edge handling here.
        let mut se = 0.0f64;
        let mut n = 0usize;
        for y in 6..h - 6 {
            for x in 6..w - 6 {
                for c in 0..3 {
                    let d = (a[y * w + x][c] - b[y * w + x][c]) as f64;
                    se += d * d;
                    n += 1;
                }
            }
        }
        10.0 * (1.0 / (se / n as f64)).log10()
    }

    /// Bilinear demosaic on the CPU — the floor any real algorithm must beat.
    fn bilinear(mosaic: &[f32], colours: [u32; 4], w: usize, h: usize) -> Vec<[f32; 3]> {
        let at = |x: i64, y: i64| -> (f32, u32) {
            let x = x.clamp(0, w as i64 - 1) as usize;
            let y = y.clamp(0, h as i64 - 1) as usize;
            (mosaic[y * w + x], colours[(y & 1) * 2 + (x & 1)])
        };
        let mut out = vec![[0.0; 3]; w * h];
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let mut sum = [0.0f32; 3];
                let mut cnt = [0.0f32; 3];
                let (v0, c0) = at(x, y);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (v, c) = at(x + dx, y + dy);
                        sum[c as usize] += v;
                        cnt[c as usize] += 1.0;
                    }
                }
                let mut px = [0.0; 3];
                for c in 0..3 {
                    px[c] = if c as u32 == c0 { v0 } else { sum[c] / cnt[c].max(1.0) };
                }
                out[y as usize * w + x as usize] = px;
            }
        }
        out
    }

    /// RCD must reconstruct a known scene correctly on every CFA layout, and
    /// clearly better than bilinear. A parity or channel slip shows up as a
    /// collapse in PSNR on the affected pattern(s).
    ///
    /// Needs a GPU: `cargo test --release rcd_reconstructs -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn rcd_reconstructs_a_synthetic_scene() {
        let (w, h) = (256usize, 128usize);
        let truth = scene(w, h);
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        let ctx = rt.block_on(crate::gpu::shared::SharedContext::new()).expect("gpu");
        let (device, queue) = (&ctx.device, &ctx.queue);

        for pattern in 0..4u32 {
            let colours = tile_colors(pattern);
            let mosaic: Vec<f32> = (0..w * h)
                .map(|i| truth[i][colours[((i / w) & 1) * 2 + ((i % w) & 1)] as usize])
                .collect();
            let raw: Vec<u16> = mosaic.iter().map(|v| (v * 4000.0).round() as u16).collect();
            let quantised: Vec<f32> = raw.iter().map(|&v| v as f32 / 4000.0).collect();

            let size = wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 };
            let raw_tex = device.create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: None,
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R16Uint,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                bytemuck::cast_slice(&raw),
            );
            let out_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OUTPUT_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            // Non-unity WB, to prove it scales in and back out exactly.
            let wb = [1.9, 1.0, 1.4, 1.0];
            let params = GpuDemosaicParams::new(pattern, [0; 4], [0.0; 4], 4000, wb);
            ctx.demosaic.run(
                device,
                queue,
                &raw_tex.create_view(&Default::default()),
                w as u32,
                h as u32,
                &params,
                &out_tex.create_view(&Default::default()),
            );

            let bpr = (w as u32 * 8 + 255) & !255;
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (bpr * h as u32) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut enc = device.create_command_encoder(&Default::default());
            enc.copy_texture_to_buffer(
                out_tex.as_image_copy(),
                wgpu::ImageCopyBuffer {
                    buffer: &buf,
                    layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: None },
                },
                size,
            );
            queue.submit(Some(enc.finish()));
            buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::Maintain::Wait);
            let data = buf.slice(..).get_mapped_range();
            let mut rcd = vec![[0.0f32; 3]; w * h];
            for y in 0..h {
                let row: &[half::f16] = bytemuck::cast_slice(&data[y * bpr as usize..][..w * 8]);
                for x in 0..w {
                    rcd[y * w + x] = [row[x * 4].to_f32(), row[x * 4 + 1].to_f32(), row[x * 4 + 2].to_f32()];
                }
            }
            drop(data);

            let p_rcd = psnr(&rcd, &truth, w, h);
            let p_bil = psnr(&bilinear(&quantised, colours, w, h), &truth, w, h);
            eprintln!("pattern {pattern}: RCD {p_rcd:.2} dB, bilinear {p_bil:.2} dB");
            assert!(p_rcd > 30.0, "pattern {pattern}: RCD PSNR {p_rcd:.2} dB — reconstruction is broken");
            assert!(p_rcd > p_bil + 3.0, "pattern {pattern}: RCD ({p_rcd:.2}) not clearly better than bilinear ({p_bil:.2})");
        }
    }
}
