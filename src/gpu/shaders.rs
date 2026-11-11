/// WGSL shader code for real-time RAW image processing
///
/// This shader applies non-destructive edits to RAW sensor data in real-time.
/// Full debayering, color science, and advanced adjustments.
///
/// Color pipeline order (scene-referred → display-referred):
///   1.  Debayer          – camera-native linear RGB
///   2.  White Balance    – camera-space per-channel multipliers
///   3.  Color Matrix     – camera → sRGB linear (Bradford D50→D65 adapted)
///   4.  (Noise reduction runs before this pass — see gpu/denoise.rs)
///   5.  Sharpening       – unsharp mask in sRGB linear
///   6.  Temperature/Tint – blue-yellow and green-magenta axes in sRGB space
///   7.  Highlight clamp  – neutralise near-clipping channels before exposure,
///       then Dehaze      – invert the haze model (camera space, scene-linear)
///   8.  Exposure         – linear stop adjustment, × Texture/Clarity gain
///   9.  Highlights/Shadows
///  10.  Contrast         – pivoted at 18% gray (perceptual midtone)
///  11.  Levels           – whites / blacks
///  12.  Saturation
///  13.  Vibrance
///  14.  Filmic tone curve (fallback path only — DCP ProfileToneCurve otherwise)
///  15.  sRGB TRC         – IEC 61966-2-1 piecewise (γ=2.4 + linear toe)
///  16.  Clamp
pub const PASSTHROUGH_SHADER: &str = r#"
// ── Highlights / Shadows tuning ──────────────────────────────────────────────
// Both are a gain in STOPS, chosen by the level of a pixel's SURROUNDINGS in
// stops relative to sensor white (1.0 scene-linear — WB and Step 3 routinely
// reach +1; middle grey sits near −2.5). See apply_highlights_shadows.
// MIRRORED in core::tone_regions (shader_tone_region_constants_match_rust).
//
// Highlights weigh in from HS_HL_LO (just above grey) to full at HS_HL_HI,
// past clipping. At −1 they pull a surround down HS_HL_RECOVER stops — a 2.0
// highlight lands near 0.8, back where the tone curve resolves gradation —
// and at +1 lift it HS_HL_LIFT.
const HS_HL_LO: f32 = -2.0;
const HS_HL_HI: f32 = 0.5;
const HS_HL_RECOVER: f32 = 1.3;
const HS_HL_LIFT: f32 = 0.6;
// Shadows: full below HS_SH_LO, fading out by HS_SH_HI (just above grey),
// ±HS_SH_LIFT stops at ±1.
const HS_SH_LO: f32 = -6.5;
const HS_SH_HI: f32 = -2.0;
const HS_SH_LIFT: f32 = 1.6;

// ========== Vertex Shader ==========
// Full-screen triangle (no vertex buffers needed)

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var output: VertexOutput;

    // Full-screen triangle covering entire viewport
    // Vertex 0: (-1, -1) -> tex (0, 1)
    // Vertex 1: ( 3, -1) -> tex (2, 1)
    // Vertex 2: (-1,  3) -> tex (0, -1)
    let x = f32(i32(vertex_index & 1u) * 4 - 1);
    let y = f32(i32(vertex_index >> 1u) * 4 - 1);

    output.clip_position = vec4<f32>(x, -y, 0.0, 1.0);

    // Map the target across `view_rect` — the sub-region of the displayed
    // image this pass is asked to produce. At (0,0,1,1) this is the identity
    // and the pass covers the whole image, which is what the histogram pass
    // and every un-zoomed display render use.
    //
    // Rendering only the visible window is what keeps cost flat as the user
    // zooms in: the target no longer grows with zoom, it just looks at a
    // smaller slice of the source at higher magnification.
    //
    // (params.zoom / pan_x / pan_y are vestigial and intentionally unread —
    // the viewport shader's projection matrix owns zoom and pan.)
    let t = vec2<f32>((x + 1.0) * 0.5, (y + 1.0) * 0.5);
    let view = params.view_rect.xy + t * params.view_rect.zw;

    // Phase 135: Conditional Crop
    // When is_cropping is active, we render the full image bounds [0, 1]
    // allowing the user to see and adjust the crop handles non-destructively.
    if (params.is_cropping == 0u) {
        output.tex_coords = params.crop.xy + view * params.crop.zw;
    } else {
        output.tex_coords = view;
    }

    return output;
}

// ========== Fragment Shader ==========

struct EditParams {
    exposure: f32,        // -5.0 to +5.0 stops
    contrast: f32,        // -100.0 to +100.0
    highlights: f32,      // -100.0 to +100.0
    shadows: f32,         // -100.0 to +100.0
    whites: f32,          // white point (default 1.0)
    blacks: f32,          // black point (default 0.0)
    vibrance: f32,        // -100.0 to +100.0
    saturation: f32,      // -100.0 to +100.0
    temperature: f32,     // -1.0 to +1.0
    tint: f32,            // -1.0 to +1.0
    // Full-image sensor UV → input-texture UV: uv * src_scale + src_offset.
    // The identity except for the detail loupe, whose input is a crop.
    src_offset: vec2<f32>,
    // Phase 14: Color science metadata
    wb_multipliers: vec4<f32>,  // White balance [R, G, B, G2]
    // Camera RGB → working space, ROWS (rebuilt via transpose() below).
    // has_dcp == 1 → camera → ProPhoto (XYZ D50 step folded in on the CPU)
    // has_dcp == 0 → camera → linear sRGB
    forward_matrix_0: vec3<f32>,  // row 0
    padding3: f32,
    forward_matrix_1: vec3<f32>,  // row 1
    padding4: f32,
    forward_matrix_2: vec3<f32>,  // row 2
    padding5: f32,
    src_scale: vec2<f32>,
    pan_y: f32,           // vestigial; holds later offsets in place
    // Phase 34: CFA Pattern
    cfa_pattern: u32,

    // Sub-region of the displayed image this render covers: (u0, v0, du, dv)
    // in view UV. (0,0,1,1) = the whole image. Occupies four formerly-dead
    // padding floats, so offsets are unchanged. See gpu/params.rs.
    view_rect: vec4<f32>,

    // Phase 36: Per-channel Black Levels (vec4 alignment = 16 bytes)
    black_levels: vec4<u32>,

    white_level: u32,

    // Highlight-reconstruction correction, camera-native (core::highlights).
    // Three former pad slots: no offset moved.
    highlight_k_r: f32,
    highlight_k_g: f32,
    highlight_k_b: f32,

    // Phase 38: Manual Black Level Offsets
    black_offsets: vec4<f32>,

    // Phase 39: Black Level Phase Correction
    black_phase_x: u32,
    black_phase_y: u32,

    // Phase 133: Noise Reduction
    luma_noise: f32,
    color_noise: f32,

    // Phase 50: Sharpening
    sharpening: f32,

    // Phase 51: Sharpening Masking
    sharpen_masking: f32,

    // Phase 52: Rotation
    rotation: f32,

    // Export colour space (core::output_space::OutputSpace::shader_id); 0 =
    // sRGB, which is all the display ever uses. Former pad slot.
    output_space: u32,

    // Phase 66: Crop (vec4 alignment = 16 bytes)
    crop: vec4<f32>,

    // Phase 135: Non-destructive crop visibility
    is_cropping: u32,

    // Padding to reach 256 bytes (16-byte alignment)
    has_dcp: u32,
    dcp_has_curve: u32,
    // EXIF orientation: 1 normal, 3 = 180°, 6 = 90° CW, 8 = 270° CW.
    // Display-space UVs are mapped to sensor space at the top of fs_main.
    orientation: u32,

    // Local adjustment masks (Step 10.5). Only the first mask_count are active.
    mask_count: u32,
    // Selected tone mapping operator. Only Camera (0) vs non-Camera is read;
    // the operator's shape arrives baked into the tone curve LUT. Occupies a
    // formerly-dead pad slot, so no offset moved. See gpu/params.rs.
    tone_mapper: u32,
    // 1 = sample the baked point-curve LUT (binding 6) in display space.
    curve_active: u32,
    // 0 = off, i + 1 = tint mask i's coverage red (see the end of fs_main).
    mask_overlay: u32,
    // Lens correction — see core::lens and Step 0. Mirrors gpu/params.rs.
    lens_dist: vec4<f32>,      // r_d/r_u polynomial p0..p3
    lens_misc: vec4<f32>,      // p4, scale.x, scale.y, active
    lens_tca_r: vec4<f32>,     // red (v, c, b), vignetting radius ratio
    lens_tca_b: vec4<f32>,     // blue (v, c, b), manual vignetting amount
    lens_vig: vec4<f32>,       // PA k1..k3, 1 / output corner radius
    defringe: vec4<f32>,       // purple amount, purple hue°, green amount, green hue°
    defringe_misc: vec4<f32>,  // active, edge radius (texels), -, -
    presence: vec4<f32>,       // texture, clarity, dehaze, -
    haze: vec4<f32>,           // airlight (camera-native rgb), haze map present
}

// One local adjustment mask (112 bytes — uniform array stride must be ×16).
// Mirrors gpu/params.rs GpuMask. Geometry lives in full-image display UV, the
// same space as params.crop, so masks stay glued to image content when the
// crop changes. Like the crop rectangle, masks do not follow the straighten
// rotation.
struct Mask {
    info: vec4<f32>,  // x: type (0 linear / 1 radial / 2 brush / 3 range / 4 AI), y: enabled, z: invert, w: feather
    geom: vec4<f32>,  // linear: ax, ay, bx, by;  radial: cx, cy, rx, ry
    adj0: vec4<f32>,  // exposure (EV), contrast, saturation, warmth
    adj1: vec4<f32>,  // highlights, shadows, tint, rotation (radians)
    adj2: vec4<f32>,  // hue (radians), has brush layer, luma range on, colour range on
    range: vec4<f32>, // luma lo, luma hi, luma smooth, colour tolerance
    color: vec4<f32>, // colour target (ln r/g, ln b/g), -, -
}

struct MaskBlock {
    masks: array<Mask, 8>,
}

// Group 1: local masks and the brush layers they paint into (gpu/brush.rs).
@group(1) @binding(0)
var brush_layers: texture_2d_array<f32>;

@group(1) @binding(1)
var<uniform> mask_block: MaskBlock;

// AI masks' segmentation bitmaps, one layer per mask slot, in the same
// oriented display frame as the brush layers (gpu/ai_mask.rs).
@group(1) @binding(2)
var ai_layers: texture_2d_array<f32>;

// Group 2: Presence, all in sensor UV. The local-contrast bands
// (gpu/local_contrast.rs): y = B0 (fine), z = B1 (texture base), log2 luma;
// the Clarity guided-filter coefficients, B2 = a·B1 + b, on a coarse grid;
// and the dehaze guided-filter coefficients (core/haze.rs), (a, b) per
// coarse cell — Rg32Float, which is not filterable, hence the manual
// bilinear in haze_coeffs.
@group(2) @binding(0)
var local_tex: texture_2d<f32>;

@group(2) @binding(1)
var haze_tex: texture_2d<f32>;

@group(2) @binding(2)
var clarity_tex: texture_2d<f32>;

// Texture / Clarity: stops of gain per stop of band detail at +1 (at −1 the
// band is removed outright, i.e. ×1), and the soft ceiling on the total.
const TEXTURE_GAIN: f32 = 1.0;
const CLARITY_GAIN: f32 = 1.5;
const LOCAL_LIMIT_EV: f32 = 1.5;
// Dehaze: share of the estimated haze removed at +1 (He et al.'s ω — a
// little is kept so distance still reads), the transmission floor (caps the
// 1/t gain, which also multiplies noise), and how much haze −1 adds.
const DEHAZE_OMEGA: f32 = 0.95;
const DEHAZE_T0: f32 = 0.15;
const HAZE_ADD: f32 = 0.6;
// Sky tolerance (Wang et al.'s K, relative to the airlight): a pixel within
// this of the airlight is mostly haze-coloured scene — sky — and its 1/t
// gain is cut in proportion, or any error in A (and the sky's noise) comes
// back multiplied by up to 1/T0 as a colour cast.
const DEHAZE_SKY_K: f32 = 0.15;
// No pixel may lose more than this share of its darkest (airlight-relative)
// channel: removing more haze than the pixel itself contains drives it
// negative, which clamps — crushed, hue-shifted shadows.
const DEHAZE_KEEP: f32 = 0.9;

// Bilinear (a, b) of the dehaze guided filter at sensor UV.
fn haze_coeffs(uv: vec2<f32>) -> vec2<f32> {
    let dims = vec2<i32>(textureDimensions(haze_tex));
    let p = uv * vec2<f32>(dims) - vec2<f32>(0.5);
    let i0 = vec2<i32>(floor(p));
    let f = p - floor(p);
    let hi = dims - vec2<i32>(1);
    let c00 = textureLoad(haze_tex, clamp(i0, vec2<i32>(0), hi), 0).xy;
    let c10 = textureLoad(haze_tex, clamp(i0 + vec2<i32>(1, 0), vec2<i32>(0), hi), 0).xy;
    let c01 = textureLoad(haze_tex, clamp(i0 + vec2<i32>(0, 1), vec2<i32>(0), hi), 0).xy;
    let c11 = textureLoad(haze_tex, clamp(i0 + vec2<i32>(1, 1), vec2<i32>(0), hi), 0).xy;
    return mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
}

// Highlight reconstruction (Step 3). MIRRORED from core::highlights —
// `shader_highlight_constants_match_rust` checks. A channel starts being
// rebuilt at HL_CLIP_LO of its ceiling; rebuilt channels go white as the
// least-clipped channel climbs from HL_FADE_LO of its ceiling to clipping;
// HL_ITERATIONS rounds let channels clipped together lift each other.
const HL_CLIP_LO: f32 = 0.9;
const HL_FADE_LO: f32 = 0.7;
const HL_ITERATIONS: i32 = 4;

// Rebuild clipped channels (core::highlights, whose `reconstruct` is the CPU
// mirror of this). In white-balanced space, each clipped channel is
// estimated as the geometric mean of the other two — their current
// estimates, clipped or not, since a clipped value is a close lower bound —
// times `k`, the learned correction; it only ever raises a channel, blended
// in by `clip_w`. Log space throughout: the mean is a sum there.
fn reconstruct_highlight(measured: vec3<f32>, ceiling: vec3<f32>, clip_w: vec3<f32>, k: vec3<f32>) -> vec3<f32> {
    let lm = log(max(measured, vec3<f32>(1e-6)));
    var l = lm;
    for (var i = 0; i < HL_ITERATIONS; i++) {
        l.g = mix(lm.g, max(lm.g, 0.5 * (l.b + l.r) + k.g), clip_w.g);
        l.b = mix(lm.b, max(lm.b, 0.5 * (l.r + l.g) + k.b), clip_w.b);
        l.r = mix(lm.r, max(lm.r, 0.5 * (l.g + l.b) + k.r), clip_w.r);
    }
    let ratio = measured / ceiling;
    let white = smoothstep(HL_FADE_LO, 1.0, min(ratio.r, min(ratio.g, ratio.b)));
    let top = max(l.r, max(l.g, l.b));
    return exp(l + (vec3<f32>(top) - l) * white * clip_w);
}

// Log-chromaticity epsilon for colour-range masks. MIRRORED from
// core::types::MASK_CHROMA_EPS — `shader_mask_constants_match_rust` checks.
const MASK_CHROMA_EPS: f32 = 0.004;

// Phase 128: Pass 2 now reads the debayered Rgba16Float intermediate texture.
@group(0) @binding(0)
var input_texture: texture_2d<f32>;  // Debayered camera-native linear RGB

@group(0) @binding(1)
var texture_sampler: sampler;  // Not used for integer textures, kept for compatibility

@group(0) @binding(2)
var<uniform> params: EditParams;

// Phase 140: DCP Textures
@group(0) @binding(3)
var hsv_lut: texture_3d<f32>;

@group(0) @binding(4)
var tone_curve: texture_1d<f32>;

@group(0) @binding(5)
var lut_sampler: sampler;

// Point curves, baked on the CPU (core::curves): rgb = R∘M, G∘M, B∘M.
@group(0) @binding(6)
var curve_lut: texture_1d<f32>;

fn sample_curve(x: f32) -> vec4<f32> {
    let n = f32(textureDimensions(curve_lut));
    let c = (clamp(x, 0.0, 1.0) * (n - 1.0) + 0.5) / n;
    return textureSampleLevel(curve_lut, lut_sampler, c, 0.0);
}

// Phase 128: Debayer functions removed — Pass 2 reads pre-debayered float texture.
// Neighbour helper for NR / sharpening.  One texture read per neighbour
// instead of the 9 that debayer() required.
fn get_neighbor(coords: vec2<i32>) -> vec3<f32> {
    let dimensions = textureDimensions(input_texture);
    let clamped = clamp(coords, vec2<i32>(0, 0), vec2<i32>(i32(dimensions.x) - 1, i32(dimensions.y) - 1));
    return textureLoad(input_texture, clamped, 0).rgb;
}

fn get_srgb_neighbor(coords: vec2<i32>, dimensions: vec2<u32>, cm: mat3x3<f32>) -> vec3<f32> {
    let clamped = vec2<i32>(
        clamp(coords.x, 0, i32(dimensions.x) - 1),
        clamp(coords.y, 0, i32(dimensions.y) - 1)
    );
    let cam = textureLoad(input_texture, clamped, 0).rgb;
    return max(cm * (cam * params.wb_multipliers.rgb), vec3<f32>(0.0));
}

// Fast per-pixel luma from the debayered camera texture.
// Applies WB but skips the full color matrix — green-dominant weighting
// approximates luminance in Bayer-sensor space and is ~5× cheaper per tap
// than get_srgb_neighbor (no mat3×3 multiply).
fn get_cam_luma(coords: vec2<i32>, dimensions: vec2<u32>) -> f32 {
    let clamped = clamp(coords, vec2<i32>(0), vec2<i32>(dimensions) - vec2<i32>(1));
    let cam = textureLoad(input_texture, clamped, 0).rgb * params.wb_multipliers.rgb;
    return cam.r * 0.25 + cam.g * 0.50 + cam.b * 0.25;
}

// Weight of a local adjustment mask at point p (full-image display UV,
// the same space as params.crop). Closed-form — no mask texture needed.
// `aspect` is the display (post-orientation) width/height ratio: UV space
// isn't square-pixel, so the radial branch's rotation must un-distort to an
// isotropic frame before rotating, or a rotated ellipse would come out
// sheared instead of rotated on any non-square image.
fn mask_shape(m: Mask, p: vec2<f32>, aspect: f32) -> f32 {
    var w: f32;
    if (m.info.x > 3.5) {
        // AI mask: its bitmap (sampled in mask_weight, which knows the slot).
        w = 0.0;
    } else if (m.info.x > 2.5) {
        // Range mask: the whole frame, narrowed only by its refinements.
        w = 1.0;
    } else if (m.info.x > 1.5) {
        // Brush mask: nothing but what is painted.
        w = 0.0;
    } else if (m.info.x < 0.5) {
        // Linear gradient: weight 1 at A falling smoothly to 0 at B.
        // Iso-lines are perpendicular to B−A in UV space.
        let d = m.geom.zw - m.geom.xy;
        let t = dot(p - m.geom.xy, d) / max(dot(d, d), 1e-6);
        w = 1.0 - smoothstep(0.0, 1.0, t);
    } else {
        // Radial ellipse with per-axis radii; feather sets where the
        // falloff starts (1 = from center, 0 = hard edge).
        let d = p - m.geom.xy;
        let cos_r = cos(-m.adj1.w);
        let sin_r = sin(-m.adj1.w);
        // Un-distort to isotropic space, apply the inverse rotation (maps
        // the sample point into the ellipse's unrotated local frame), then
        // redistort back before comparing against the (raw UV) radii. At
        // rotation=0 this reduces exactly to the un-rotated formula.
        let iso = vec2<f32>(d.x * aspect, d.y);
        let rotated_iso = vec2<f32>(
            iso.x * cos_r - iso.y * sin_r,
            iso.x * sin_r + iso.y * cos_r,
        );
        let rotated = vec2<f32>(rotated_iso.x / aspect, rotated_iso.y);
        let q = rotated / max(m.geom.zw, vec2<f32>(1e-4));
        let r = length(q); // 1.0 at the ellipse edge
        let inner = 1.0 - clamp(m.info.w, 0.0, 0.999);
        w = 1.0 - smoothstep(inner, 1.0, r);
    }
    return w;
}

// `q` is where the pixel-anchored layers (AI bitmap, brush) are read: the
// display-frame UV of the SOURCE position this pixel shows, so they follow the
// picture through Straighten and lens correction. It is `p` itself when
// neither is active. Shapes stay in `p`, the frame the user places them in.
//
// Full weight of mask `idx` at `p`: shape, plus its brush layer (signed:
// painting adds, erasing subtracts), times its range refinements, then
// inverted if asked. `color0` is the pixel as it entered the mask stage
// (linear sRGB) and `cam0` its camera-native value, for the luminance and
// colour ranges respectively.
fn mask_weight(m: Mask, idx: u32, p: vec2<f32>, q: vec2<f32>, aspect: f32, color0: vec3<f32>, cam0: vec3<f32>) -> f32 {
    var w = mask_shape(m, p, aspect);
    if (m.info.x > 3.5) {
        w = textureSampleLevel(ai_layers, lut_sampler, q, i32(idx), 0.0).r;
    }
    if (m.adj2.y > 0.5) {
        w = w + textureSampleLevel(brush_layers, lut_sampler, q, i32(idx), 0.0).r;
    }
    w = clamp(w, 0.0, 1.0);
    if (m.adj2.z > 0.5) {
        // Luminance band in display terms, where a photographer reads it.
        let lum = linear_to_srgb(max(dot(color0, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0));
        let s = m.range.z;
        w = w * smoothstep(m.range.x - s, m.range.x, lum) * (1.0 - smoothstep(m.range.y, m.range.y + s, lum));
    }
    if (m.adj2.w > 0.5) {
        // Exposure-invariant chromaticity of the camera-native colour.
        let c = max(cam0, vec3<f32>(0.0)) + vec3<f32>(MASK_CHROMA_EPS);
        let lc = vec2<f32>(log(c.r / c.g), log(c.b / c.g));
        let d = distance(lc, m.color.xy);
        let tol = 0.04 + 0.5 * m.range.w;
        w = w * (1.0 - smoothstep(tol, tol * 1.5 + 0.03, d));
    }
    if (m.info.z > 0.5) {
        w = 1.0 - w;
    }
    return w;
}

// ── Perceptual colour controls (Oklab) ───────────────────────────────────────
// MIRRORED from core::perceptual — that module explains the choices and its
// unit tests check them; `shader_perceptual_constants_match_rust` asserts
// the constants agree. Contrast, Whites/Blacks, Saturation, Vibrance and the
// mask Hue run here; everything that models light stays linear RGB.
const CONTRAST_PIVOT_L: f32 = 0.5646216;
const SAT_KNEE: f32 = 0.8;
const HK_STRENGTH: f32 = 3.0;
const SKIN_DIR: vec2<f32> = vec2<f32>(0.5, 0.8660254);
const SKIN_COS_LO: f32 = 0.7660444;
const SKIN_COS_HI: f32 = 0.9659258;
const SKIN_PROTECT: f32 = 0.6;

// Linear sRGB → cube-rooted LMS (Ottosson's M1, then a signed cube root).
fn ok_lms_(c: vec3<f32>) -> vec3<f32> {
    let lms = vec3<f32>(
        dot(c, vec3<f32>(0.41222146, 0.53633255, 0.051445995)),
        dot(c, vec3<f32>(0.2119035, 0.6806995, 0.10739696)),
        dot(c, vec3<f32>(0.08830246, 0.28171885, 0.6299787)),
    );
    return sign(lms) * pow(max(abs(lms), vec3<f32>(1e-20)), vec3<f32>(1.0 / 3.0));
}

fn ok_from_rgb(c: vec3<f32>) -> vec3<f32> {
    let q = ok_lms_(c);
    return vec3<f32>(
        dot(q, vec3<f32>(0.21045426, 0.7936178, -0.004072047)),
        dot(q, vec3<f32>(1.9779985, -2.4285922, 0.4505937)),
        dot(q, vec3<f32>(0.025904037, 0.78277177, -0.80867577)),
    );
}

fn ok_to_rgb(lab: vec3<f32>) -> vec3<f32> {
    let q = vec3<f32>(
        dot(lab, vec3<f32>(1.0, 0.39633778, 0.21580376)),
        dot(lab, vec3<f32>(1.0, -0.105561346, -0.06385417)),
        dot(lab, vec3<f32>(1.0, -0.08948418, -1.2914855)),
    );
    let lms = q * q * q;
    return vec3<f32>(
        dot(lms, vec3<f32>(4.0767417, -3.3077116, 0.23096994)),
        dot(lms, vec3<f32>(-1.268438, 2.6097574, -0.34131938)),
        dot(lms, vec3<f32>(-0.0041960863, -0.7034186, 1.7076147)),
    );
}

fn ok_l(c: vec3<f32>) -> f32 {
    return dot(ok_lms_(c), vec3<f32>(0.21045426, 0.7936178, -0.004072047));
}

// Largest Oklab saturation C/L that linear sRGB holds along the unit hue
// direction (a, b) — Ottosson's compute_max_saturation (polynomial per
// limiting channel + one Halley step). `rgb >= 0` is a cone through black,
// so this holds at every L.
fn ok_max_saturation(a: f32, b: f32) -> f32 {
    var k0: f32; var k1: f32; var k2: f32; var k3: f32; var k4: f32;
    var wl: f32; var wm: f32; var ws: f32;
    if (-1.8817033 * a - 0.8093649 * b > 1.0) {
        k0 = 1.1908628; k1 = 1.7657673; k2 = 0.5966264; k3 = 0.755152; k4 = 0.5677124;
        wl = 4.0767417; wm = -3.3077116; ws = 0.23096994;
    } else if (1.8144411 * a - 1.1944528 * b > 1.0) {
        k0 = 0.73956515; k1 = -0.45954404; k2 = 0.08285427; k3 = 0.1254107; k4 = 0.14503204;
        wl = -1.268438; wm = 2.6097574; ws = -0.34131938;
    } else {
        k0 = 1.3573365; k1 = -0.00915799; k2 = -1.1513021; k3 = -0.50559606; k4 = 0.00692167;
        wl = -0.0041960863; wm = -0.7034186; ws = 1.7076147;
    }
    var s = k0 + k1 * a + k2 * b + k3 * a * a + k4 * a * b;
    let k_l = 0.39633778 * a + 0.21580376 * b;
    let k_m = -0.105561346 * a - 0.06385417 * b;
    let k_s = -0.08948418 * a - 1.2914855 * b;
    let l_ = 1.0 + s * k_l;
    let m_ = 1.0 + s * k_m;
    let s_ = 1.0 + s * k_s;
    let f = wl * l_ * l_ * l_ + wm * m_ * m_ * m_ + ws * s_ * s_ * s_;
    let f1 = 3.0 * (wl * k_l * l_ * l_ + wm * k_m * m_ * m_ + ws * k_s * s_ * s_);
    let f2 = 6.0 * (wl * k_l * k_l * l_ + wm * k_m * k_m * m_ + ws * k_s * k_s * s_);
    s = s - f * f1 / (f1 * f1 - 0.5 * f * f2);
    return s;
}

// A boosted saturation rolled off into the gamut limit instead of across
// it; never below `s`, the saturation before the boost.
fn ok_soft_limit(boosted: f32, s: f32, s_max: f32) -> f32 {
    let knee = SAT_KNEE * s_max;
    if (boosted <= knee) {
        return boosted;
    }
    let room = s_max - knee;
    return max(knee + room * (1.0 - exp(-(boosted - knee) / room)), s);
}

// Saturation (chroma factor `sat`, 1 = unchanged) and Vibrance (`vib`, 0 =
// unchanged): hue held exactly, boosts kept inside the gamut cone, lightness
// compensated for the Helmholtz–Kohlrausch effect, skin spared some of a
// vibrance boost.
fn perceptual_saturation(c: vec3<f32>, sat: f32, vib: f32) -> vec3<f32> {
    if (sat == 1.0 && vib == 0.0) {
        return c;
    }
    let lab = ok_from_rgb(c);
    let l = lab.x;
    let ch = length(lab.yz);
    if (l <= 1e-6 || ch <= 1e-7) {
        return c;
    }
    let d = lab.yz / ch;
    let s = ch / l;
    let s_max = max(ok_max_saturation(d.x, d.y), 1e-4);
    var s1 = s * sat;
    if (sat > 1.0) {
        s1 = ok_soft_limit(s1, s, s_max);
    }
    var s2 = s1;
    if (vib != 0.0) {
        var w = 1.0 - clamp(s1 / s_max, 0.0, 1.0);
        if (vib > 0.0) {
            w = w * (1.0 - SKIN_PROTECT * smoothstep(SKIN_COS_LO, SKIN_COS_HI, dot(d, SKIN_DIR)));
        }
        let f = 1.0 + vib * w;
        s2 = s1 * f;
        if (f > 1.0) {
            s2 = ok_soft_limit(s2, s1, s_max);
        }
    }
    let c2 = s2 * l;
    let hk = HK_STRENGTH * (0.116 * sqrt(max((1.0 - d.y) * 0.5, 0.0)) + 0.085) * clamp(1.0 - l, 0.0, 1.0);
    // Scaling all of (L, a, b) keeps saturation and hue: still in the cone.
    let r = clamp((l - hk * (c2 - ch)) / l, 0.5, 1.5);
    return ok_to_rgb(vec3<f32>(l, d * c2) * r);
}

// Rotate the Oklab hue at constant L and chroma; a colour the new hue
// cannot hold comes in to its gamut limit along that hue.
fn perceptual_hue_shift(c: vec3<f32>, angle: f32) -> vec3<f32> {
    if (angle == 0.0) {
        return c;
    }
    let lab = ok_from_rgb(c);
    let ch = length(lab.yz);
    if (lab.x <= 1e-6 || ch <= 1e-7) {
        return c;
    }
    let ca = cos(angle);
    let sa = sin(angle);
    let d = vec2<f32>(lab.y * ca - lab.z * sa, lab.y * sa + lab.z * ca) / ch;
    let c2 = min(ch, ok_max_saturation(d.x, d.y) * lab.x);
    return ok_to_rgb(vec3<f32>(lab.x, d * c2));
}

// S-curve on Oklab L with slope k at the pivot; black and white held,
// overbrights (profile-less path, still scene-linear) carried through.
fn contrast_curve(l: f32, k: f32) -> f32 {
    let p = CONTRAST_PIVOT_L;
    if (l <= p) {
        return p * pow(max(l, 0.0) / p, k);
    } else if (l < 1.0) {
        return 1.0 - (1.0 - p) * pow((1.0 - l) / (1.0 - p), k);
    }
    return l;
}

// Contrast on L, carried to the colour as a scale by (L'/L)³ — Oklab is
// homogeneous, so hue and saturation are untouched.
fn perceptual_contrast(c: vec3<f32>, k: f32) -> vec3<f32> {
    if (k == 1.0) {
        return c;
    }
    let l = ok_l(c);
    if (l <= 1e-6) {
        return c;
    }
    let r = contrast_curve(l, max(k, 0.05)) / l;
    return c * (r * r * r);
}

// Whites/Blacks: linear levels on Y = L³ (exactly the value of a grey, so
// neutrals map as they always did), carried to colours by scaling.
fn perceptual_levels(c: vec3<f32>, blacks: f32, whites: f32) -> vec3<f32> {
    if (blacks == 0.0 && whites == 1.0) {
        return c;
    }
    let l = ok_l(c);
    let y = l * l * l;
    if (y <= 1e-9) {
        return vec3<f32>(0.0);
    }
    return c * (max((y - blacks) / (whites - blacks + 0.0001), 0.0) / y);
}

// ── Gamut mapping (Oklab, constant hue) ─────────────────────────────────────
// MIRRORED from core::perceptual::gamut_map, which explains it.
const GAMUT_THRESHOLD: f32 = 0.9;
const GAMUT_LIMIT: f32 = 1.5;
const GAMUT_POWER: f32 = 1.2;
const GAMUT_L0_ALPHA: f32 = 0.05;
// core::perceptual::gamut_compress_scale(), precomputed.
const GAMUT_SCALE: f32 = 0.11087054;

// The ACES RGC power curve: identity to the threshold, 1 at the limit.
fn gamut_compress(d: f32) -> f32 {
    if (d <= GAMUT_THRESHOLD) {
        return d;
    }
    let x = (d - GAMUT_THRESHOLD) / GAMUT_SCALE;
    return GAMUT_THRESHOLD + GAMUT_SCALE * x / pow(1.0 + pow(x, GAMUT_POWER), 1.0 / GAMUT_POWER);
}

// Cube-root-LMS hue derivative: along hue (a, b) at L = 1 the LMS cube
// roots are 1 + s·k for saturation s.
fn ok_hue_k(a: f32, b: f32) -> vec3<f32> {
    return vec3<f32>(
        0.39633778 * a + 0.21580376 * b,
        -0.105561346 * a - 0.06385417 * b,
        -0.08948418 * a - 1.2914855 * b,
    );
}

// Output-space RGB at L = 1, saturation s, along k.
fn gamut_rgb_along(lms_to: mat3x3<f32>, k: vec3<f32>, s: f32) -> vec3<f32> {
    let q = vec3<f32>(1.0) + s * k;
    return lms_to * (q * q * q);
}

// MIRRORED from core::perceptual (`shader_perceptual_constants_match_rust`).
const GAMUT_MARCH: f32 = 1.2;
const GAMUT_MARCH_STEPS: u32 = 18u;
const GAMUT_BISECT_STEPS: u32 = 20u;

fn gamut_inside(lms_to: mat3x3<f32>, k: vec3<f32>, s: f32) -> bool {
    let rgb = gamut_rgb_along(lms_to, k, s);
    return min(rgb.r, min(rgb.g, rgb.b)) >= 0.0;
}

// ok_max_saturation for any output gamut containing sRGB — mirrors
// core::perceptual::max_saturation_in, which explains the march: sRGB is
// Ottosson's fit; a wider space marches out from it ×1.2 until a channel
// goes negative, then bisects.
fn gamut_max_saturation(a: f32, b: f32, lms_to: mat3x3<f32>, wide: bool) -> f32 {
    let s_srgb = ok_max_saturation(a, b);
    if (!wide) {
        return s_srgb;
    }
    let k = ok_hue_k(a, b);
    var lo = 0.9 * s_srgb;
    var hi = lo * GAMUT_MARCH;
    for (var i = 0u; i < GAMUT_MARCH_STEPS; i = i + 1u) {
        if (!gamut_inside(lms_to, k, hi)) {
            break;
        }
        lo = hi;
        hi = hi * GAMUT_MARCH;
    }
    for (var i = 0u; i < GAMUT_BISECT_STEPS; i = i + 1u) {
        let mid = 0.5 * (lo + hi);
        if (gamut_inside(lms_to, k, mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    return lo;
}

// Lightness and chroma of the output gamut's cusp at hue (a, b) —
// Ottosson's find_cusp.
fn ok_find_cusp(a: f32, b: f32, lms_to: mat3x3<f32>, wide: bool) -> vec2<f32> {
    let s_cusp = gamut_max_saturation(a, b, lms_to, wide);
    let rgb = gamut_rgb_along(lms_to, ok_hue_k(a, b), s_cusp);
    let l_cusp = pow(1.0 / max(rgb.r, max(rgb.g, rgb.b)), 1.0 / 3.0);
    return vec2<f32>(l_cusp, l_cusp * s_cusp);
}

// Boundary crossing of the line (l0, 0) → (l1, c1) at hue (a, b), as the
// line parameter t (< 1: the point is outside). Ottosson's
// find_gamut_intersection: exact below the cusp, a triangle estimate plus
// one Halley step above it.
fn ok_gamut_intersection(a: f32, b: f32, l1: f32, c1: f32, l0: f32, lms_to: mat3x3<f32>, wide: bool) -> f32 {
    let cusp = ok_find_cusp(a, b, lms_to, wide);
    if ((l1 - l0) * cusp.y - (cusp.x - l0) * c1 <= 0.0) {
        return cusp.y * l0 / (c1 * cusp.x + cusp.y * (l0 - l1));
    }
    let t = cusp.y * (l0 - 1.0) / (c1 * (cusp.x - 1.0) + cusp.y * (l0 - l1));

    let k = ok_hue_k(a, b);
    let dt = vec3<f32>(l1 - l0) + c1 * k;
    let q = vec3<f32>(l0 * (1.0 - t) + t * l1) + (t * c1) * k;
    let lms = q * q * q;
    let d1 = 3.0 * dt * q * q;
    let d2 = 6.0 * dt * dt * q;
    let rows = transpose(lms_to);
    let f = vec3<f32>(dot(rows[0], lms), dot(rows[1], lms), dot(rows[2], lms)) - vec3<f32>(1.0);
    let f1 = vec3<f32>(dot(rows[0], d1), dot(rows[1], d1), dot(rows[2], d1));
    let f2 = vec3<f32>(dot(rows[0], d2), dot(rows[1], d2), dot(rows[2], d2));
    let u = f1 / (f1 * f1 - 0.5 * f * f2);
    let steps = select(vec3<f32>(3.0e38), -f * u, u >= vec3<f32>(0.0));
    let step = min(steps.x, min(steps.y, steps.z));
    return select(t, t + step, step < 3.0e38);
}

// Fit extended-range linear sRGB into the output space's [0,1]³ at constant
// Oklab hue: the pipeline's one gamut stage. `space` is the output space
// (OutputSpace::shader_id; 0 = sRGB); the result is its linear RGB.
// Near-neutral colours pass untouched; the rest are compressed towards
// Ottosson's adaptive L0 — the colour's own L for near-greys, white for
// overbrights (the path to white).
fn gamut_map(c: vec3<f32>, space: u32) -> vec3<f32> {
    let out_m = output_matrix(space);
    let out = out_m * c;
    let lab = ok_from_rgb(c);
    let l = lab.x;
    if (l <= 0.0) {
        return vec3<f32>(0.0);
    }
    let ch = length(lab.yz);
    if (ch <= 1e-6) {
        return select(vec3<f32>(1.0), clamp(out, vec3<f32>(0.0), vec3<f32>(1.0)), l <= 1.0);
    }
    let lms_to = out_m * ok_lms_to_srgb();
    let dir = lab.yz / ch;
    let ld = l - 0.5;
    let e1 = 0.5 + abs(ld) + GAMUT_L0_ALPHA * ch;
    let l0 = 0.5 * (1.0 + sign(ld) * (e1 - sqrt(max(e1 * e1 - 2.0 * abs(ld), 0.0))));
    let t = ok_gamut_intersection(dir.x, dir.y, l, ch, l0, lms_to, space != 0u);
    let d = 1.0 / max(t, 1e-6);
    if (d <= GAMUT_THRESHOLD) {
        return out;
    }
    let f = min(gamut_compress(d), 1.0) / d;
    let lab2 = vec3<f32>(l0 + f * (l - l0), dir * (f * ch));
    let q = vec3<f32>(
        dot(lab2, vec3<f32>(1.0, 0.39633778, 0.21580376)),
        dot(lab2, vec3<f32>(1.0, -0.105561346, -0.06385417)),
        dot(lab2, vec3<f32>(1.0, -0.08948418, -1.2914855)),
    );
    return clamp(lms_to * (q * q * q), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Ottosson's LMS → linear sRGB (M1⁻¹) as a matrix.
fn ok_lms_to_srgb() -> mat3x3<f32> {
    return transpose(mat3x3<f32>(
        vec3<f32>(4.0767417, -3.3077116, 0.23096994),
        vec3<f32>(-1.268438, 2.6097574, -0.34131938),
        vec3<f32>(-0.0041960863, -0.7034186, 1.7076147),
    ));
}

// ── Output colour space (core::output_space) ────────────────────────────────
// MIRRORED from core::output_space — `shader_output_space_constants_match_rust`.
// The display is always sRGB (id 0); only exports choose another.
const ADOBE_RGB_GAMMA: f32 = 2.1992188;
const PROPHOTO_GAMMA: f32 = 1.8;
const PROPHOTO_LINEAR_EDGE: f32 = 0.001953125;
// Linear sRGB → each space, as ROWS (OutputSpace::srgb_to_space_matrix).
const OUT_P3_0: vec3<f32> = vec3<f32>(0.82246196, 0.17753804, 0.0);
const OUT_P3_1: vec3<f32> = vec3<f32>(0.0331942, 0.9668058, 0.0);
const OUT_P3_2: vec3<f32> = vec3<f32>(0.017082632, 0.07239744, 0.91051996);
const OUT_ADOBE_0: vec3<f32> = vec3<f32>(0.7151256, 0.28487438, 0.0);
const OUT_ADOBE_1: vec3<f32> = vec3<f32>(0.0, 1.0, 0.0);
const OUT_ADOBE_2: vec3<f32> = vec3<f32>(0.0, 0.041161947, 0.95883805);
const OUT_PROPHOTO_0: vec3<f32> = vec3<f32>(0.52931243, 0.3301362, 0.14055134);
const OUT_PROPHOTO_1: vec3<f32> = vec3<f32>(0.098361745, 0.87347007, 0.028168162);
const OUT_PROPHOTO_2: vec3<f32> = vec3<f32>(0.016875, 0.11767152, 0.8654535);

fn output_matrix(id: u32) -> mat3x3<f32> {
    switch (id) {
        case 1u: { return transpose(mat3x3<f32>(OUT_P3_0, OUT_P3_1, OUT_P3_2)); }
        case 2u: { return transpose(mat3x3<f32>(OUT_ADOBE_0, OUT_ADOBE_1, OUT_ADOBE_2)); }
        case 3u: { return transpose(mat3x3<f32>(OUT_PROPHOTO_0, OUT_PROPHOTO_1, OUT_PROPHOTO_2)); }
        default: {
            return mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
        }
    }
}

// Display-linear → encoded with the space's transfer curve (sRGB's for
// sRGB and Display P3).
fn encode_output(c: vec3<f32>, id: u32) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    if (id == 2u) {
        return pow(x, vec3<f32>(1.0 / ADOBE_RGB_GAMMA));
    }
    if (id == 3u) {
        return select(pow(x, vec3<f32>(1.0 / PROPHOTO_GAMMA)), 16.0 * x, x < vec3<f32>(PROPHOTO_LINEAR_EDGE));
    }
    return vec3<f32>(linear_to_srgb(x.r), linear_to_srgb(x.g), linear_to_srgb(x.b));
}

// The inverse of encode_output.
fn decode_output(e: vec3<f32>, id: u32) -> vec3<f32> {
    let x = max(e, vec3<f32>(0.0));
    if (id == 2u) {
        return pow(x, vec3<f32>(ADOBE_RGB_GAMMA));
    }
    if (id == 3u) {
        return select(pow(x, vec3<f32>(PROPHOTO_GAMMA)), x / 16.0, x < vec3<f32>(16.0 * PROPHOTO_LINEAR_EDGE));
    }
    return select(pow((x + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045));
}

// Interleaved gradient noise (Jimenez 2014): ALU-only, high-frequency.
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// TPDF dither before 8-bit quantisation. Smooth gradients (skies) otherwise
// band: 256 levels in display encoding is fewer than the eye resolves over
// a wide, slow ramp. Triangular noise of ±1 LSB decorrelates the rounding
// error from the signal — its mean is zero, so brightness does not move,
// and its power does not depend on the value, so it adds no visible
// modulation. Uniform → triangular by Gjøl's remap. It fades out within
// one LSB of either end, so pure black and white (clipping, letterboxing)
// stay pure. One noise for all channels: luminance grain, not colour speckle.
fn dither_8bit(c: vec3<f32>, p: vec2<f32>) -> vec3<f32> {
    let u = ign(p) * 2.0 - 1.0;
    let t = sign(u) * (1.0 - sqrt(1.0 - abs(u)));
    let room = clamp(min(c, vec3<f32>(1.0) - c) * 255.0, vec3<f32>(0.0), vec3<f32>(1.0));
    return c + t * room / 255.0;
}

// Top of the scene-linear domain the tone LUT covers.
// MIRRORED from core::tonemap::TONE_LUT_MAX — the Rust test
// `shader_tone_lut_constants_match_rust` asserts these agree.
const TONE_LUT_MAX: f32 = 16.0;

// Tone mapper ids. Only Camera is distinguished here: every other operator's
// SHAPE arrives baked into the curve LUT, so the shader needs no per-operator
// branch and no per-operator cost. See core::tonemap.
const TONE_CAMERA: u32 = 0u;

// Scene-linear → tone LUT coordinate. Log-encoded so one 4096-entry table can
// span [0, TONE_LUT_MAX] while still spending ~1000 entries below 1.0, where
// tone curves are steepest. Must match core::tonemap::tone_lut_encode exactly,
// or the baked table is read at the wrong positions.
fn tone_lut_encode(x: f32) -> f32 {
    return log2(1.0 + max(x, 0.0)) / log2(1.0 + TONE_LUT_MAX);
}

// Sample the tone curve at texel centers.
// The curve texture stores N samples of f(decode(i/(N-1))); naive normalized-
// coordinate sampling is skewed by half a texel at both ends.
fn sample_tone_curve(x: f32) -> f32 {
    let n = f32(textureDimensions(tone_curve));
    let t = clamp(tone_lut_encode(x), 0.0, 1.0);
    let c = (t * (n - 1.0) + 0.5) / n;
    return textureSampleLevel(tone_curve, lut_sampler, c, 0.0).r;
}

// Highlights & Shadows: a local exposure change.
//
// `level` is the scene-linear level of the pixel's SURROUNDINGS: the caller
// carries the pixel there with `to_surround` (Step 4 — the Clarity base B2
// over the pixel's own L, gpu/local_contrast.rs). The gain is chosen from it
// and multiplies the pixel, so:
//   • detail survives — a region gets one gain, so each pixel's ratio to its
//     neighbours (texture in a recovered sky, folds in a lifted shadow) is
//     kept. Weighting each pixel by its own level compressed exactly those
//     ratios: at −1 a sky's clouds went flat grey.
//   • hue and saturation survive — a scene-linear multiply moves along the
//     ray through black. (The old additive Shadows lift was a grey wash that
//     lifted black too.)
//   • tonal order survives — the gain changes by less than a stop per stop of
//     level (slope ≤ 1.5·amount/width < 1), so a brighter surround never ends
//     darker than a dimmer one.
// B2 is edge-preserving, so across a strong edge the surround follows the
// edge and the gain does not bleed over it (no halo).
//
// What `level` measures differs by space and is the caller's choice:
//   • linear sRGB (no-DCP fallback) → luma, `dot(c, (0.2126, 0.7152, 0.0722))`
//   • ProPhoto (DCP path)           → the PEAK CHANNEL
// ProPhoto's Y coefficients are (0.2880, 0.7119, 0.0001) — blue contributes
// essentially nothing, so a blown blue sky would score near-zero luma and the
// highlight weight would never engage. Proximity to clipping is also what
// recovery is about.
//
// MUST run in scene-linear space, before any shoulder. Applied afterwards it is
// just a uniform multiply on an already-compressed band: the highlight goes
// grey and no texture comes back.
fn highlights_shadows_ev(highlights: f32, shadows: f32, level: f32) -> f32 {
    let ev = log2(max(level, 1e-6));
    var gain = 0.0;
    if (highlights != 0.0) {
        // Recovery gets more authority than lift: pulling a blown highlight
        // back has further to travel than brightening one.
        let amount = select(HS_HL_LIFT, HS_HL_RECOVER, highlights < 0.0);
        gain += highlights * amount * smoothstep(HS_HL_LO, HS_HL_HI, ev);
    }
    if (shadows != 0.0) {
        gain += shadows * HS_SH_LIFT * (1.0 - smoothstep(HS_SH_LO, HS_SH_HI, ev));
    }
    return gain;
}

fn apply_highlights_shadows(c: vec3<f32>, highlights: f32, shadows: f32, level: f32) -> vec3<f32> {
    if (highlights == 0.0 && shadows == 0.0) {
        return c;
    }
    return c * exp2(highlights_shadows_ev(highlights, shadows, level));
}

// Adobe DNG SDK "RGBTone": hue-preserving tone curve application.
// Applying a steep s-curve independently per channel widens the spread between
// channels and badly oversaturates colours (neon reds/cyans/pinks).  Adobe
// instead curves the max and min channels and places the middle channel by
// linear interpolation, preserving the hue ratio (g-b)/(r-b) etc.
fn dcp_rgb_tone(c: vec3<f32>) -> vec3<f32> {
    let r = c.r; let g = c.g; let b = c.b;
    var rr: f32; var gg: f32; var bb: f32;
    if (r >= g) {
        if (g > b) {
            // r >= g > b
            rr = sample_tone_curve(r);
            bb = sample_tone_curve(b);
            gg = bb + (rr - bb) * (g - b) / (r - b);
        } else if (b > r) {
            // b > r >= g
            bb = sample_tone_curve(b);
            gg = sample_tone_curve(g);
            rr = gg + (bb - gg) * (r - g) / (b - g);
        } else if (b > g) {
            // r >= b > g
            rr = sample_tone_curve(r);
            gg = sample_tone_curve(g);
            bb = gg + (rr - gg) * (b - g) / (r - g);
        } else {
            // r >= g == b (neutral-ish)
            rr = sample_tone_curve(r);
            gg = sample_tone_curve(g);
            bb = gg;
        }
    } else {
        if (r >= b) {
            // g > r >= b
            gg = sample_tone_curve(g);
            bb = sample_tone_curve(b);
            rr = bb + (gg - bb) * (r - b) / (g - b);
        } else if (b > g) {
            // b > g > r
            bb = sample_tone_curve(b);
            rr = sample_tone_curve(r);
            gg = rr + (bb - rr) * (g - r) / (b - r);
        } else {
            // g >= b > r
            gg = sample_tone_curve(g);
            rr = sample_tone_curve(r);
            bb = rr + (gg - rr) * (b - r) / (g - r);
        }
    }
    return vec3<f32>(rr, gg, bb);
}

// RGBTone for extended-range input (the profile-less path, whose colours
// outside sRGB carry a negative channel). The curve is only defined from
// 0, so the non-negative part is toned and a negative channel follows the
// brightest one's gain — continuous at 0, and the colour stays outside the
// gamut for Step 15b to map instead of being clipped onto it here.
fn rgb_tone_signed(c: vec3<f32>) -> vec3<f32> {
    let toned = dcp_rgb_tone(max(c, vec3<f32>(0.0)));
    let hi = max(c.r, max(c.g, c.b));
    if (hi <= 0.0) {
        return toned;
    }
    let gain = max(toned.r, max(toned.g, toned.b)) / hi;
    return select(toned, c * gain, c < vec3<f32>(0.0));
}

// IEC 61966-2-1 sRGB transfer function.
// Uses γ = 2.4 with a linear toe segment, NOT the 1/2.2 approximation.
// The difference is ~5% in the shadows and midtones.
fn linear_to_srgb(x: f32) -> f32 {
    if x <= 0.0031308 {
        return 12.92 * x;
    }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

// The two colour-pass entry points: one per output format (a wgpu pipeline
// bakes its target format in, see SharedContext). Only the 8-bit one
// dithers — the 16-bit export has precision to spare.
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let c = color_pass(input);
    return vec4<f32>(dither_8bit(c.rgb, input.clip_position.xy), c.a);
}

@fragment
fn fs_main_16(input: VertexOutput) -> @location(0) vec4<f32> {
    return color_pass(input);
}

fn color_pass(input: VertexOutput) -> vec4<f32> {
    // Discard fragments outside texture bounds (letterboxing when zoomed out)
    if input.tex_coords.x < 0.0 || input.tex_coords.x > 1.0 ||
       input.tex_coords.y < 0.0 || input.tex_coords.y > 1.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    // Phase 52: Apply straightening rotation in DISPLAY space.
    // The aspect used for the angle correction must be the DISPLAY aspect —
    // for 90°/270° EXIF orientations that's the sensor aspect inverted.
    var tex_coords = input.tex_coords;

    if (abs(params.rotation) > 0.01) {
        // The whole image's aspect, also when the input is a loupe crop.
        let dimensions_r = vec2<f32>(textureDimensions(input_texture)) * params.src_scale;
        var aspect = dimensions_r.x / dimensions_r.y;
        if (params.orientation == 6u || params.orientation == 8u) {
            aspect = 1.0 / aspect;
        }
        let angle_rad = params.rotation * 3.14159265359 / 180.0;
        let cos_a = cos(angle_rad);
        let sin_a = sin(angle_rad);

        var centered = tex_coords - vec2<f32>(0.5, 0.5);
        centered.x *= aspect;

        let rotated = vec2<f32>(
            centered.x * cos_a - centered.y * sin_a,
            centered.x * sin_a + centered.y * cos_a
        );

        var unscaled = rotated;
        unscaled.x /= aspect;
        tex_coords = unscaled + vec2<f32>(0.5, 0.5);

        if (tex_coords.x < 0.0 || tex_coords.x > 1.0 ||
            tex_coords.y < 0.0 || tex_coords.y > 1.0) {
            return vec4<f32>(0.0, 0.0, 0.0, 1.0);
        }
    }

    // EXIF orientation: map display-space UV → sensor-space UV. The debayered
    // texture stays in native (landscape) sensor layout; the render target is
    // sized to the ORIENTED aspect by the caller, so sampling through this
    // mapping produces an upright image.
    //   6 = 90° CW display rotation:  sensor = (v, 1-u)
    //   8 = 270° CW (90° CCW):        sensor = (1-v, u)
    //   3 = 180°:                     sensor = (1-u, 1-v)
    if (params.orientation == 6u) {
        tex_coords = vec2<f32>(tex_coords.y, 1.0 - tex_coords.x);
    } else if (params.orientation == 8u) {
        tex_coords = vec2<f32>(1.0 - tex_coords.y, tex_coords.x);
    } else if (params.orientation == 3u) {
        tex_coords = vec2<f32>(1.0 - tex_coords.x, 1.0 - tex_coords.y);
    }

    let dimensions = textureDimensions(input_texture);

    // ── Step 0: Lens correction ──────────────────────────────────────────────
    // `tex_coords` is now this OUTPUT pixel's position in sensor UV. Map it to
    // the SOURCE position it comes from (core::lens): geometry first, then
    // per-channel lateral CA on the distorted coordinate, exactly as lensfun's
    // models are defined. `lens_misc.yz` takes a UV offset from centre into
    // the profile's calibration units; the Constrain zoom is folded into the
    // polynomial on the CPU (core::lens::gpu_params), since a zoom in this
    // scale would cancel when converting back.
    var uv_r = tex_coords;
    var uv_b = tex_coords;
    var lens_gain = 1.0;
    let lens_on = params.lens_misc.w > 0.5;
    if (lens_on) {
        let lscale = params.lens_misc.yz;
        let p = (tex_coords - vec2<f32>(0.5)) * lscale;
        let ru = length(p);
        let dc = params.lens_dist;
        let f = dc.x + ru * (dc.y + ru * (dc.z + ru * (dc.w + ru * params.lens_misc.x)));
        let pd = p * f;
        let rd = length(pd);
        let fr = params.lens_tca_r.x + rd * (params.lens_tca_r.y + rd * params.lens_tca_r.z);
        let fb = params.lens_tca_b.x + rd * (params.lens_tca_b.y + rd * params.lens_tca_b.z);
        tex_coords = pd / lscale + vec2<f32>(0.5);
        uv_r = pd * fr / lscale + vec2<f32>(0.5);
        uv_b = pd * fb / lscale + vec2<f32>(0.5);

        // Profile vignetting (PA model) at the SOURCE radius, which lensfun
        // normalises to the half-diagonal.
        let rv = rd * params.lens_tca_r.w;
        let rv2 = rv * rv;
        let k = params.lens_vig.xyz;
        lens_gain = 1.0 / max(1.0 + rv2 * (k.x + rv2 * (k.y + rv2 * k.z)), 0.05);
        // Manual vignetting on the OUTPUT frame: ±1 = ±1 stop at the corner.
        let rn = ru * params.lens_vig.w;
        let rn2 = rn * rn;
        lens_gain *= exp2(params.lens_tca_b.w * (0.5 * rn2 + 0.5 * rn2 * rn2));

        if (tex_coords.x < 0.0 || tex_coords.x > 1.0 || tex_coords.y < 0.0 || tex_coords.y > 1.0) {
            return vec4<f32>(0.0, 0.0, 0.0, 1.0);
        }
    }

    // Where the pixel-anchored mask layers are read (see mask_weight): back
    // to the display frame from the source position. Exactly the plain
    // display UV whenever neither Straighten nor the lens moved it.
    var layer_uv = input.tex_coords;
    if (abs(params.rotation) > 0.01 || lens_on) {
        if (params.orientation == 6u) {
            layer_uv = vec2<f32>(1.0 - tex_coords.y, tex_coords.x);
        } else if (params.orientation == 8u) {
            layer_uv = vec2<f32>(tex_coords.y, 1.0 - tex_coords.x);
        } else if (params.orientation == 3u) {
            layer_uv = vec2<f32>(1.0 - tex_coords.x, 1.0 - tex_coords.y);
        } else {
            layer_uv = tex_coords;
        }
    }

    // `tex_coords`, `uv_r` and `uv_b` stay in full-image sensor UV (the
    // local-contrast bands and haze map are whole-image textures); only reads
    // of the input go through the source mapping. Exact identity outside the
    // loupe: x * 1.0 + 0.0 == x.
    let src_uv = tex_coords * params.src_scale + params.src_offset;
    let src_uv_r = uv_r * params.src_scale + params.src_offset;
    let src_uv_b = uv_b * params.src_scale + params.src_offset;
    let pixel_coords = vec2<i32>(
        i32(src_uv.x * f32(dimensions.x)),
        i32(src_uv.y * f32(dimensions.y))
    );

    // ── Step 1: Read debayered camera-native linear RGB from intermediate ────
    // Phase 135: textureSample with linear filtering for smooth downscaling.
    // With lens correction on, red and blue come from their own CA-shifted
    // positions (explicit LOD: these coordinates are not a smooth function of
    // the screen, so implicit derivatives would be meaningless).
    var color: vec3<f32>;
    if (lens_on) {
        color = vec3<f32>(
            textureSampleLevel(input_texture, texture_sampler, src_uv_r, 0.0).r,
            textureSampleLevel(input_texture, texture_sampler, src_uv, 0.0).g,
            textureSampleLevel(input_texture, texture_sampler, src_uv_b, 0.0).b,
        );
    } else {
        color = textureSample(input_texture, texture_sampler, src_uv).rgb;
    }
    // Camera-native colour, for colour-range masks (exposure- and
    // WB-independent by construction).
    let cam0 = color;

    // Phase 135: Non-destructive dimming
    // If we are in crop mode, dim the area outside the current crop rectangle.
    // Note: We use input.tex_coords (pre-rotation) to check against the crop bounds.
    if (params.is_cropping == 1u) {
        let in_x = input.tex_coords.x >= params.crop.x && input.tex_coords.x <= (params.crop.x + params.crop.z);
        let in_y = input.tex_coords.y >= params.crop.y && input.tex_coords.y <= (params.crop.y + params.crop.w);
        if (!(in_x && in_y)) {
            color = color * 0.3; // 70% dimming for discarded areas
        }
    }

    // (STAGE 1 clip detection removed — any per-Bayer-site approach creates a
    // 2-pixel-period checkerboard on bright fabric/sky because adjacent R/G/B Bayer
    // sites have structurally different raw values for any non-neutral scene.
    // Step 15b's gamut map handles overbrights post-matrix without artifacts.)

    // ── Step 2: White Balance ─────────────────────────────────────────────────
    color = color * params.wb_multipliers.rgb;

    // Pre-fetch the 8 nearest camera-space neighbours (WB-scaled) for the
    // sharpening ring. The branch is uniform across the warp (params are
    // constants), so no divergence.
    var nb_n  = vec3<f32>(0.0); var nb_s  = vec3<f32>(0.0);
    var nb_e  = vec3<f32>(0.0); var nb_w  = vec3<f32>(0.0);
    var nb_ne = vec3<f32>(0.0); var nb_nw = vec3<f32>(0.0);
    var nb_se = vec3<f32>(0.0); var nb_sw = vec3<f32>(0.0);
    if (params.sharpening > 0.0) {
        let wb3  = params.wb_multipliers.rgb;
        let dmax = vec2<i32>(dimensions) - vec2<i32>(1);
        nb_n  = textureLoad(input_texture, clamp(pixel_coords + vec2<i32>( 0, -1), vec2<i32>(0), dmax), 0).rgb * wb3;
        nb_s  = textureLoad(input_texture, clamp(pixel_coords + vec2<i32>( 0,  1), vec2<i32>(0), dmax), 0).rgb * wb3;
        nb_e  = textureLoad(input_texture, clamp(pixel_coords + vec2<i32>( 1,  0), vec2<i32>(0), dmax), 0).rgb * wb3;
        nb_w  = textureLoad(input_texture, clamp(pixel_coords + vec2<i32>(-1,  0), vec2<i32>(0), dmax), 0).rgb * wb3;
        nb_ne = textureLoad(input_texture, clamp(pixel_coords + vec2<i32>( 1, -1), vec2<i32>(0), dmax), 0).rgb * wb3;
        nb_nw = textureLoad(input_texture, clamp(pixel_coords + vec2<i32>(-1, -1), vec2<i32>(0), dmax), 0).rgb * wb3;
        nb_se = textureLoad(input_texture, clamp(pixel_coords + vec2<i32>( 1,  1), vec2<i32>(0), dmax), 0).rgb * wb3;
        nb_sw = textureLoad(input_texture, clamp(pixel_coords + vec2<i32>(-1,  1), vec2<i32>(0), dmax), 0).rgb * wb3;
    }

    // (Noise reduction is no longer done here: it runs once per change in the
    // cached denoise pre-pass — gpu/denoise.rs — and this pass samples its
    // output. luma_noise / color_noise are vestigial in this struct.)

    // ── Step 3: Highlight reconstruction (white-balanced camera space) ───────
    // A channel that saturates reads its ceiling instead of its true value.
    // Green clips first, so a bright sky reads magenta once WB lifts red and
    // blue past it — and exposure pulled down showed that as purple. Each
    // clipped channel is rebuilt from the others (reconstruct_highlight;
    // core::highlights explains the estimate), with a per-image correction
    // learned from the unclipped rims of clipped areas, so a sky is rebuilt
    // in the colour it has just before it clips.
    //
    // Clip weights compare each DEBAYERED value against its own WB-scaled
    // ceiling (the demosaic caps every sample there): spatially smooth, no
    // Bayer-period checkerboard. Before exposure, so pulling exposure down
    // reveals the rebuilt values. The correction is stored camera-native and
    // converted to this white balance here: k_c − (−1.5·ln w_c + ½Σ ln w).
    let wb3 = params.wb_multipliers.rgb;
    let hl_clip_w = smoothstep(wb3 * HL_CLIP_LO, wb3, color);
    if (max(hl_clip_w.r, max(hl_clip_w.g, hl_clip_w.b)) > 0.0) {
        let k_native = vec3<f32>(params.highlight_k_r, params.highlight_k_g, params.highlight_k_b);
        let lw = log(wb3);
        let k = k_native + 1.5 * lw - vec3<f32>(0.5 * (lw.r + lw.g + lw.b));
        color = reconstruct_highlight(color, wb3, hl_clip_w, k);
    }

    // ── Step 3b: Dehaze ───────────────────────────────────────────────────────
    // Haze is additive light: I = J·t + A·(1 − t), A the airlight. Inverted
    // here, on scene-linear camera values where that model is exact — after
    // highlight reconstruction (which needs the true clip ceilings) and
    // before exposure, which it commutes with. The dark channel of I/A reads
    // 1 − t (core::haze); evaluating the guided filter with this pixel's own
    // guide value is what keeps the skyline sharp. Normalising by A also
    // makes all of it independent of the WB sliders.
    let dehaze = params.presence.z;
    if (dehaze != 0.0 && params.haze.w > 0.5) {
        let air = params.haze.rgb * wb3;
        let nrm = color / max(air, vec3<f32>(1e-6));
        let ab = haze_coeffs(tex_coords);
        let dark = clamp(ab.x * (nrm.r + nrm.g + nrm.b) / 3.0 + ab.y, 0.0, 1.0);
        if (dehaze > 0.0) {
            var t = 1.0 - DEHAZE_OMEGA * dehaze * dark;
            let own_dark = min(nrm.r, min(nrm.g, nrm.b));
            t = max(t, 1.0 - DEHAZE_KEEP * own_dark);
            let from_air = max(abs(nrm.r - 1.0), max(abs(nrm.g - 1.0), abs(nrm.b - 1.0)));
            t = min(t * max(DEHAZE_SKY_K / max(from_air, 1e-4), 1.0), 1.0);
            t = max(t, DEHAZE_T0);
            color = max((color - air) / t + air, vec3<f32>(0.0));
        } else {
            // Adding haze: more where there already is some (distance),
            // a little everywhere.
            let t = 1.0 + HAZE_ADD * dehaze * (0.35 + 0.65 * dark);
            color = color * t + air * (1.0 - t);
        }
    }

    // ── Step 4: Exposure ──────────────────────────────────────────────────────
    // The lens vignetting gain rides here rather than at sampling: applied
    // before Step 3 it would push brightened corners past the clip thresholds
    // and trigger false highlight reconstruction.
    //
    // So do Texture and Clarity: each scales one band of log luminance
    // (gpu/local_contrast.rs), which is a per-pixel exposure change — hue
    // and saturation untouched, and scene-linear, so the shoulder below rolls
    // off whatever it lifts past white. Both fade out in deep shadows, where
    // log-domain detail is mostly noise; Clarity also leaves the highlights
    // alone, keeping it a midtone control.
    //
    // Highlights/Shadows (global or in any mask, `presence.w`) read the same
    // bands: B2 − L is the step from this pixel to its surroundings.
    var local_ev = 0.0;
    var surround_ev = 0.0;
    let texture_amt = params.presence.x;
    let clarity_amt = params.presence.y;
    let wants_surround = params.presence.w != 0.0;
    if (texture_amt != 0.0 || clarity_amt != 0.0 || wants_surround) {
        let bands = textureSampleLevel(local_tex, texture_sampler, tex_coords, 0.0);
        let gf = textureSampleLevel(clarity_tex, texture_sampler, tex_coords, 0.0).xy;
        let b2 = gf.x * bands.z + gf.y;
        surround_ev = b2 - bands.w;
        if (texture_amt != 0.0 || clarity_amt != 0.0) {
            // Surrounding level after exposure, in stops below sensor white.
            let base_ev = b2 + params.exposure;
            let shadow_w = smoothstep(-11.0, -7.0, base_ev);
            let mid_w = shadow_w * (1.0 - smoothstep(-1.5, 1.0, base_ev));
            let k_tex = select(TEXTURE_GAIN, 1.0, texture_amt < 0.0);
            let k_clar = select(CLARITY_GAIN, 1.0, clarity_amt < 0.0);
            local_ev = texture_amt * k_tex * (bands.y - bands.z) * shadow_w
                     + clarity_amt * k_clar * (bands.z - b2) * mid_w;
            local_ev = local_ev / (1.0 + abs(local_ev) / LOCAL_LIMIT_EV);
        }
    }
    let exposure_multiplier = pow(2.0, params.exposure) * lens_gain * exp2(local_ev);
    color = color * exposure_multiplier;
    // Any level homogeneous in the colour (peak channel, luma) times this is
    // that level of the pixel's surroundings — what apply_highlights_shadows
    // keys off. Texture/Clarity's gain is detail too, so it comes back out.
    // WB, exposure, dehaze and Step 3 all act on the pixel itself, so they
    // carry through.
    let to_surround = select(1.0, exp2(surround_ev - local_ev), wants_surround);

    // ── Step 5: Camera RGB → sRGB (DCP Pipeline vs Fallback) ────────────────
    // NOTE: WB was already applied in Step 2 above. Do NOT re-apply here.

    let matrix_from_params = transpose(mat3x3<f32>(
        params.forward_matrix_0,
        params.forward_matrix_1,
        params.forward_matrix_2
    ));

    if (params.has_dcp == 1u) {
        // 1. Camera RGB → ProPhoto RGB, in one multiply.
        // The DCP's ForwardMatrix is camera → XYZ D50, and XYZ D50 → ProPhoto is
        // a fixed matrix; both are constant across the frame, so their product
        // is composed once on the CPU (raw/dcp.rs::interpolate_at_temperature)
        // instead of traversing XYZ per pixel.
        //
        // No pre-clamp here — negative ProPhoto values (wide-gamut colors) must reach
        // the HueSatMap with correct hue/saturation. We clamp after reconstruction.
        var pp = matrix_from_params * color;

        // ── Highlights & Shadows (SCENE-LINEAR, before the shoulder) ────────
        // Deliberately here and not at Step 10. Everything below — the knee,
        // the HueSatMap value axis, the tone-curve LUT — works in [0,1], so by
        // Step 10 a full stop of highlight (1.0 → 2.0 scene-linear) has already
        // been squeezed by the knee into 0.850 → 0.944, about 9% of range.
        // Recovery applied to that is a uniform multiply on a flattened band:
        // the highlight goes grey and no texture returns. Running it here, on
        // values that still exceed 1.0, redistributes them back down into the
        // part of the curve that resolves detail.
        //
        // Peak channel rather than luma, of the surroundings — see
        // apply_highlights_shadows.
        let hs_level = max(pp.r, max(pp.g, pp.b)) * to_surround;
        pp = apply_highlights_shadows(pp, params.highlights, params.shadows, hs_level);

        // Highlight shoulder — CAMERA OPERATOR ONLY.
        //
        // WB amplification pushes bright pixels above 1.0 linear (a bright blue
        // puck: raw 0.75 × wb_b 1.45 ≈ 1.09). The DCP ProfileToneCurve is
        // DEFINED only on [0,1], so without this every value > 1 would collapse
        // to one flat output: blown highlights with zero gradation. Compress
        // [0.7, ∞) smoothly into [0.7, 1.0) per channel instead (C1-continuous
        // Reinhard knee). Being per-channel is deliberate, not incidental: it
        // also lets bright pixels sample the profile's top value slices, where
        // camera-matching profiles desaturate/darken (this DCP scales saturated
        // cyan sat ×0.78 and red value ×0.65–0.85 up there).
        //
        // Every OTHER operator is itself the scene-linear → display mapping,
        // and its LUT spans [0, TONE_LUT_MAX]. Kneeing first would hand it an
        // already-compressed signal and it would tone-map a tone-mapped image —
        // the exact failure that made Highlights recovery return grey instead
        // of texture. So the knee is skipped and the operator sees the real
        // scene-linear range. (The HueSatMap's value axis still clamps at 1.0
        // for its LOOKUP, which is correct: it only selects a value slice.)
        if (params.tone_mapper == TONE_CAMERA) {
            let sh_knee: f32 = 0.7;
            let sh_head: f32 = 1.0 - sh_knee;
            if (pp.r > sh_knee) { let o = pp.r - sh_knee; pp.r = sh_knee + sh_head * o / (o + sh_head); }
            if (pp.g > sh_knee) { let o = pp.g - sh_knee; pp.g = sh_knee + sh_head * o / (o + sh_head); }
            if (pp.b > sh_knee) { let o = pp.b - sh_knee; pp.b = sh_knee + sh_head * o / (o + sh_head); }
        }

        // 3. ProPhoto HSV for HueSatMap lookup
        let v    = max(pp.r, max(pp.g, pp.b));
        let cmin = min(pp.r, min(pp.g, pp.b));
        let delta = v - cmin;
        var h = 0.0;
        var s = 0.0;
        if (v > 0.001) { s = delta / v; }
        if (delta > 0.001) {
            if (v == pp.r) {
                h = (pp.g - pp.b) / delta;
                if (h < 0.0) { h += 6.0; }
            } else if (v == pp.g) {
                h = (pp.b - pp.r) / delta + 2.0;
            } else {
                h = (pp.r - pp.g) / delta + 4.0;
            }
            h /= 6.0;
        }

        // Texel-center mapping for the HueSatMap LUT.
        // X axis holds hueDivs+1 texels (last duplicates hue 0 so the clamp
        // sampler interpolates across the 0°/360° red seam); sat and val axes
        // span their grid inclusively, so grid point i sits at texel center i.
        let lut_dims = vec3<f32>(textureDimensions(hsv_lut));
        let lut_uvw = vec3<f32>(
            (h * max(lut_dims.x - 1.0, 1.0) + 0.5) / lut_dims.x,
            (s * max(lut_dims.y - 1.0, 1.0) + 0.5) / lut_dims.y,
            (clamp(v, 0.0, 1.0) * max(lut_dims.z - 1.0, 1.0) + 0.5) / lut_dims.z
        );
        let lut_val = textureSampleLevel(hsv_lut, lut_sampler, lut_uvw, 0.0);
        h = fract(h + lut_val.r / 360.0);
        s = clamp(s * lut_val.g, 0.0, 1.0);
        let new_v = max(v * lut_val.b, 0.0);

        // HSV → ProPhoto RGB (s==0 degenerates correctly: p=q=t=new_v for all cases)
        var r_pp = 0.0; var g_pp = 0.0; var b_pp = 0.0;
        let h6  = h * 6.0;
        let hi  = floor(h6);
        let f   = h6 - hi;
        let p   = new_v * (1.0 - s);
        let q   = new_v * (1.0 - f * s);
        let t   = new_v * (1.0 - (1.0 - f) * s);
        let idx = i32(hi) % 6;
        if      (idx == 0) { r_pp = new_v; g_pp = t;     b_pp = p;     }
        else if (idx == 1) { r_pp = q;     g_pp = new_v; b_pp = p;     }
        else if (idx == 2) { r_pp = p;     g_pp = new_v; b_pp = t;     }
        else if (idx == 3) { r_pp = p;     g_pp = q;     b_pp = new_v; }
        else if (idx == 4) { r_pp = t;     g_pp = p;     b_pp = new_v; }
        else               { r_pp = new_v; g_pp = p;     b_pp = q;     }
        // Clamp after reconstruction — out-of-gamut negatives are clipped here, not before LUT
        pp = max(vec3<f32>(r_pp, g_pp, b_pp), vec3<f32>(0.0));

        // 4. ProfileToneCurve (linear 1:1 if no curve in this DCP)
        // Applied hue-preservingly (Adobe RGBTone) — see dcp_rgb_tone above.
        pp = dcp_rgb_tone(pp);

        // 5. ProPhoto → XYZ D50 → Bradford D50→D65 → linear sRGB
        // Combined matrix: xyz_to_srgb_D65 @ bradford_D50→D65 @ prophoto_to_xyz_D50
        // Column-major for WGSL (each vec3 is a column):
        let prophoto_to_srgb = mat3x3<f32>(
            2.0340758, -0.2288131, -0.0085698,
            -0.7273341,  1.2317301, -0.1532866,
            -0.3067418, -0.0029168,  1.1618564
        );
        color = prophoto_to_srgb * pp;

    } else {
        color = matrix_from_params * color;
    }

    // From here to Step 15b `color` is EXTENDED-RANGE linear sRGB: colours
    // outside sRGB (deep cyans, magentas, flowers, LEDs) keep their negative
    // channel, so every edit below sees the real colour and Saturation −50
    // can bring one back into gamut instead of desaturating a clipped one.
    // Nothing here may clamp to [0, 1]; Step 15b is the one gamut stage.

    // ── Step 6b: Defringe (axial CA) ─────────────────────────────────────────
    // Purple/green fringes sit beside high-contrast edges and are a property of
    // the lens's focus, not its geometry, so Step 0 cannot remove them.
    // Desaturate pixels whose hue falls in a fringe band AND that sit near a
    // strong local luma edge; flat coloured areas are left alone.
    if (params.defringe_misc.x > 0.5) {
        let rad = i32(params.defringe_misc.y);
        var lmax = get_cam_luma(pixel_coords, dimensions);
        var lmin = lmax;
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                if (dx == 0 && dy == 0) { continue; }
                let l = get_cam_luma(pixel_coords + vec2<i32>(dx, dy) * rad, dimensions);
                lmax = max(lmax, l);
                lmin = min(lmin, l);
            }
        }
        let edge = smoothstep(0.15, 0.5, (lmax - lmin) / (lmax + 0.02));
        let cmax = max(color.r, max(color.g, color.b));
        let cmin = min(color.r, min(color.g, color.b));
        let chroma = cmax - cmin;
        if (edge > 0.0 && chroma > 1e-5) {
            var hue: f32;
            if (cmax == color.r) {
                hue = (color.g - color.b) / chroma;
            } else if (cmax == color.g) {
                hue = (color.b - color.r) / chroma + 2.0;
            } else {
                hue = (color.r - color.g) / chroma + 4.0;
            }
            hue = fract(hue / 6.0 + 1.0) * 360.0;
            let sat_w = smoothstep(0.08, 0.25, chroma / max(cmax, 1e-5));
            let dp = abs(fract((hue - params.defringe.y) / 360.0 + 0.5) - 0.5) * 360.0;
            let dg = abs(fract((hue - params.defringe.w) / 360.0 + 0.5) - 0.5) * 360.0;
            // Purple fringes run from blue-violet to magenta, hence the wider
            // band; green fringes are narrower in hue.
            let band = max(
                params.defringe.x * (1.0 - smoothstep(30.0, 60.0, dp)),
                params.defringe.z * (1.0 - smoothstep(25.0, 50.0, dg)),
            );
            let w = clamp(band * edge * sat_w, 0.0, 1.0);
            let lum = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
            color = mix(color, vec3<f32>(lum), w);
        }
    }

    // ── Phase 132: 5×5 Perceptual USM with Edge-Aware Masking ──────────────
    // All reads use get_cam_luma (WB + dot, no color matrix), which is ~5×
    // cheaper per tap than get_srgb_neighbor.  Using camera luma for both the
    // center and neighbours keeps the high-pass signal in a consistent space —
    // the old 3×3 code mixed post-exposure sRGB (center) with pre-exposure
    // camera (neighbours), biasing the signal at non-zero exposure.
    //
    // params.sharpen_masking: 0 = sharpen everything, 1 = edges only.
    // The Laplacian is free since n/s/e/w are already fetched for the blur.
    if (params.sharpening > 0.0) {
        // Center luma in camera space (pre-exposure, consistent with neighbours)
        let center_cam = textureLoad(input_texture, pixel_coords, 0).rgb * params.wb_multipliers.rgb;
        let c_luma_linear = max(0.0001, center_cam.r * 0.25 + center_cam.g * 0.50 + center_cam.b * 0.25);
        let c_luma = sqrt(c_luma_linear);

        // Radius-1 samples: derived from the pre-fetched nb_* ring (no extra reads)
        let clw = vec3<f32>(0.25, 0.50, 0.25);
        let n1  = sqrt(max(0.0001, dot(nb_n,  clw)));
        let s1  = sqrt(max(0.0001, dot(nb_s,  clw)));
        let e1  = sqrt(max(0.0001, dot(nb_e,  clw)));
        let w1  = sqrt(max(0.0001, dot(nb_w,  clw)));
        let ne1 = sqrt(max(0.0001, dot(nb_ne, clw)));
        let nw1 = sqrt(max(0.0001, dot(nb_nw, clw)));
        let se1 = sqrt(max(0.0001, dot(nb_se, clw)));
        let sw1 = sqrt(max(0.0001, dot(nb_sw, clw)));

        // Radius-2 axis samples
        let n2 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>( 0, -2), dimensions)));
        let s2 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>( 0,  2), dimensions)));
        let e2 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>( 2,  0), dimensions)));
        let w2 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>(-2,  0), dimensions)));

        // Radius-2 mixed samples (axis×2, diag×1)
        let n2e1 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>( 1, -2), dimensions)));
        let n2w1 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>(-1, -2), dimensions)));
        let s2e1 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>( 1,  2), dimensions)));
        let s2w1 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>(-1,  2), dimensions)));
        let e2n1 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>( 2, -1), dimensions)));
        let e2s1 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>( 2,  1), dimensions)));
        let w2n1 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>(-2, -1), dimensions)));
        let w2s1 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>(-2,  1), dimensions)));

        // Radius-2 corner samples
        let ne2 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>( 2, -2), dimensions)));
        let nw2 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>(-2, -2), dimensions)));
        let se2 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>( 2,  2), dimensions)));
        let sw2 = sqrt(max(0.0001, get_cam_luma(pixel_coords + vec2<i32>(-2,  2), dimensions)));

        // 5×5 Gaussian blur (σ≈1.0) in perceptual camera-luma space
        //  kernel weights: 41 26 7 / 273 centre→edge on each axis
        //  ┌ 1  4  7  4  1 ┐
        //  │ 4 16 26 16  4 │
        //  │ 7 26 41 26  7 │ / 273
        //  │ 4 16 26 16  4 │
        //  └ 1  4  7  4  1 ┘
        let blur_luma = (
              c_luma * 41.0
            + (n1 + s1 + e1 + w1) * 26.0
            + (ne1 + nw1 + se1 + sw1) * 16.0
            + (n2 + s2 + e2 + w2) * 7.0
            + (n2e1 + n2w1 + s2e1 + s2w1 + e2n1 + e2s1 + w2n1 + w2s1) * 4.0
            + (ne2 + nw2 + se2 + sw2) * 1.0
        ) / 273.0;

        let high_pass = c_luma - blur_luma;

        // Laplacian edge strength (free — cardinal neighbours already fetched).
        // High value = hard edge; low value = smooth/noisy flat area.
        let laplacian = abs(4.0 * c_luma - n1 - s1 - e1 - w1);

        // sharpen_masking=0 → mask=1 everywhere (no protection).
        // sharpen_masking=1 → smoothly ramps up: flat areas (laplacian<0.04)
        // get little or no sharpening, protecting noise from amplification.
        let mask = mix(1.0, smoothstep(0.0, 0.08, laplacian), params.sharpen_masking);

        let sharpened_perc = max(0.0, c_luma + high_pass * params.sharpening * 2.0 * mask);
        let new_linear_luma = sharpened_perc * sharpened_perc;

        // Ratio-preserving scale applied to the fully-processed sRGB colour.
        // The exposure factor cancels in new/old ratio, so camera-space luma
        // in the denominator is correct even though `color` has exposure baked in.
        let sharpen_scale = new_linear_luma / c_luma_linear;
        color = color * sharpen_scale;
    }

    // ── Step 10: Highlights & Shadows — FALLBACK PATH ONLY ───────────────────
    // On the DCP path this already ran in ProPhoto scene-linear, before the
    // shoulder (see the block above); running it again here would double-apply.
    //
    // The fallback path has no shoulder until the filmic curve at Step 15, and
    // `color` here is linear sRGB that still exceeds 1.0, so this position is
    // already scene-linear for that path — it needs no move.
    //
    // Gated on has_dcp, NOT dcp_has_curve: the knee runs for any DCP, curve or
    // not, so any DCP means the work was already done upstream.
    if (params.has_dcp == 0u) {
        let luma_hl = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722)) * to_surround;
        color = apply_highlights_shadows(color, params.highlights, params.shadows, luma_hl);
    }

    // (STAGE 2, a mix towards the peak channel for overbrights, is gone: Step 3
    // handles sensor-clipping casts, and Step 15b's gamut map is the path to
    // white for display overbrights.)

    // (Step 9 removed — Temperature & Tint are now REAL white balance: the CPU
    // solves the slider Kelvin/tint to camera WB multipliers via the colour
    // matrix (Robertson CCT, see src/color.rs) and updates the wb_multipliers
    // uniform. Post-matrix sRGB channel scaling twisted hues at extremes.)

    // ── Step 10.5: Local Adjustment Masks ────────────────────────────────────
    // Applied in (extended-range) sRGB-linear space, after global
    // highlights/shadows and before global contrast/saturation, so local adjustments
    // compose consistently with their global counterparts. Mask geometry is
    // evaluated against input.tex_coords (full-image display UV, pre-rotation)
    // — the same convention as the crop rectangle.
    //
    // UV→isotropic scale factor for mask_weight's rotation math. Starts from
    // the display (post-orientation) aspect — mirroring the straightening-
    // rotation block's orientation swap above, reusing the `dimensions`
    // variable already computed for Step 1.
    // This is the FULL frame's display aspect, with no crop term, and that is
    // correct for both crop mode and normal viewing. The render target is now
    // sized to the CROPPED aspect (see RawEditor::image_display_dims), so the
    // crop sub-rect is scaled UNIFORMLY onto it: one unit of u spans
    // target_w/crop.z pixels and one unit of v spans target_h/crop.w, and
    // those two ratios reduce to exactly the full frame's aspect.
    //
    // A crop term used to live here to cancel the non-uniform stretch the
    // old full-aspect target produced. Re-adding one would double-apply the
    // crop and shear rotated ellipses. Keep in sync with
    // core::viewport::mask_uv_aspect, the CPU side of this. Scaled by
    // `src_scale` so a loupe crop still uses the whole frame's aspect.
    let mask_dims = vec2<f32>(dimensions) * params.src_scale;
    var mask_aspect = mask_dims.x / mask_dims.y;
    if (params.orientation == 6u || params.orientation == 8u) {
        mask_aspect = 1.0 / mask_aspect;
    }
    // Every mask's luminance range keys off the pixel as it ENTERED this
    // stage, so masks don't shift each other's selections.
    let color0 = color;
    var overlay_w = 0.0;
    for (var mi = 0u; mi < min(params.mask_count, 8u); mi = mi + 1u) {
        let m = mask_block.masks[mi];
        if (m.info.y < 0.5) {
            continue;
        }
        let w = mask_weight(m, mi, input.tex_coords, layer_uv, mask_aspect, color0, cam0);
        if (params.mask_overlay == mi + 1u) {
            overlay_w = w;
        }
        if (w < 0.001) {
            continue;
        }

        // Hue: rotate the Oklab hue (±30° at ±1), scaled by coverage —
        // turning by less, rather than mixing two hues, which desaturates.
        if (m.adj2.x != 0.0) {
            color = perceptual_hue_shift(color, m.adj2.x * w);
        }

        // Adjustment ranges: exposure ±4 EV; all others −1..1 (same units as
        // the global highlights/shadows sliders).

        // Exposure: weighted stops
        color = color * exp2(m.adj0.x * w);

        // Warmth: gentle sRGB-linear channel tilt (±10% at ±1). Real WB is
        // uniform-wide (can't vary per pixel), so this local tilt is kept
        // deliberately modest — large post-matrix channel scaling twists hues.
        let warm = m.adj0.w * w;
        if (warm != 0.0) {
            color = color * vec3<f32>(1.0 + 0.10 * warm, 1.0, 1.0 - 0.10 * warm);
        }

        // Tint: same gentle-tilt technique as warmth, on the green/magenta
        // axis instead of blue/amber. Positive = MAGENTA, matching the ACR
        // slider and this codebase's global tint (see TINT_SCALE in
        // src/color.rs) — the two tint sliders sit in the same sidebar and
        // must not run in opposite directions.
        let tint = m.adj1.z * w;
        if (tint != 0.0) {
            color = color * vec3<f32>(1.0 + 0.10 * tint, 1.0 - 0.10 * tint, 1.0 + 0.10 * tint);
        }

        // Highlights / Shadows: the global sliders' function, keyed off the
        // same surroundings. This runs after the tone curve, where levels are
        // display-linear, but white is still 1 and grey near 0.18, so the
        // knees land on much the same tones.
        let m_hl = m.adj1.x * w;
        let m_sh = m.adj1.y * w;
        if (m_hl != 0.0 || m_sh != 0.0) {
            let luma_m = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722)) * to_surround;
            color = apply_highlights_shadows(color, m_hl, m_sh, luma_m);
        }

        // Contrast and Saturation: the global controls' functions (Steps
        // 11 and 13), so a mask behaves exactly like the slider it mirrors.
        color = perceptual_contrast(color, max(1.0 + m.adj0.y * w, 0.0));
        color = perceptual_saturation(color, max(1.0 + m.adj0.z * w, 0.0), 0.0);
    }

    // ── Step 11: Contrast ─────────────────────────────────────────────────────
    // An S-curve on Oklab L about middle grey, applied as a scale of the
    // colour: hue and saturation stay put, black and white stay pinned. (A
    // per-channel linear stretch here shifted saturation with contrast and
    // cut everything below ~1.6% linear to black at +10.)
    color = perceptual_contrast(color, 1.0 + (params.contrast / 100.0));

    // ── Step 12: Levels (Whites & Blacks) ────────────────────────────────────
    color = perceptual_levels(color, params.blacks, params.whites);

    // ── Steps 13–14: Saturation & Vibrance (Oklab chroma) ────────────────────
    // A Rec.709-luma mix in linear RGB drifted blues to purple and drove
    // channels negative, which the clamp below turned into neon.
    color = perceptual_saturation(color, 1.0 + (params.saturation / 100.0), params.vibrance);

    // ── Step 15: Filmic Tone Curve (S-curve with highlight protection) ──────────
    // Approximates the camera picture-style curve visible in the embedded JPEG:
    //
    //  1. Midtone lift — f(x) = x·(1 + t − t·x), t = 0.08.
    //     Lifts 18 % grey by ≈ 6 %, fades to 0 at black and white.
    //     Gives the slight "pop" that camera Standard picture styles apply.
    //
    //  2. Highlight shoulder — Reinhard rolloff starting at 0.65 linear (≈ 85 % sRGB).
    //     Old threshold was 0.80 (≈ 91 % sRGB), leaving only 20 % headroom and
    //     blowing out cloud / sky detail.  0.65 gives 35 % headroom and matches
    //     the typical camera highlight-protection curve.
    // When the active DCP embeds a ProfileToneCurve (applied in the DCP block
    // above), it already provides the rendering S-curve — running the filmic
    // lift + shoulder on top double-tone-maps: lifted mids on lifted mids and a
    // second shoulder crushing highlights the profile curve already rolled off.
    // "Adobe Standard" DCPs carry NO embedded curve (they expect the host's
    // default curve), so the filmic curve stays on for them and for the
    // no-DCP fallback path.
    //
    // Tone rendering is ONE selected stage (params.tone_mapper). Reaching it
    // splits by path, because the two paths tone-map at different points:
    //   • DCP path — already handled above, at the ProfileToneCurve slot,
    //     which is where a tone curve belongs relative to the HueSatMap.
    //   • Fallback path (no profile) — nothing has curved it yet, so the
    //     selected operator is applied here.
    // Getting this gate wrong double-tone-maps, which is what the old
    // dcp_has_curve condition existed to prevent.
    if (params.tone_mapper != TONE_CAMERA) {
        if (params.has_dcp == 0u) {
            // Same baked LUT and same hue-preserving application the DCP path
            // uses — the operator does not care which colour space fed it, and
            // RGBTone beats the naive per-channel application these operators
            // normally get.
            color = rgb_tone_signed(color);
        }
    } else if (params.has_dcp == 0u || params.dcp_has_curve == 0u) {
        // Camera operator with no curve to speak of: the built-in filmic
        // rendering. Mirrored on the CPU as core::tonemap::filmic, so
        // selecting "Filmic" explicitly produces identical numbers.
        let tone_lift: f32 = 0.08;
        color = color * (vec3<f32>(1.0 + tone_lift) - tone_lift * color);

        let pre_tone_max = max(color.r, max(color.g, color.b));
        if (pre_tone_max > 0.0) {
            let threshold: f32 = 0.65;
            let headroom:  f32 = 1.0 - threshold; // 0.35
            var post_tone_max: f32;
            if (pre_tone_max <= threshold) {
                post_tone_max = pre_tone_max;
            } else {
                let over_t = pre_tone_max - threshold;
                post_tone_max = threshold + (headroom * over_t) / (over_t + headroom);
            }
            color = color * (post_tone_max / pre_tone_max);
        }
    }

    // ── Step 15b: Gamut map — the pipeline's ONE gamut stage ───────────────
    // Display-linear, extended-range sRGB in; [0,1]³ out, at constant Oklab
    // hue. Colours near the boundary roll off (ACES RGC curve) instead of
    // piling up on it as flat neon; overbrights head to white. This replaced
    // three stages with three different rules (a luma desaturation of
    // negatives straight after the matrix — which threw wide-gamut colour
    // away before any edit — a peak-channel mix, and a luma/scale-by-max
    // fit here), none of which held perceived hue. Runs after the tone
    // curve, so the fallback path's shoulder still sees > 1.0 input.
    color = gamut_map(color, params.output_space);

    // ── Step 15: Transfer function (sRGB's IEC 61966-2-1 on screen) ─────────
    color = encode_output(color, params.output_space);

    // ── Step 16: Clamp to display range ──────────────────────────────────────
    color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));

    // ── Step 17: Point curves (display-encoded, like every curve tool) ──────
    // Per channel: the LUT already composes each channel's curve after the
    // master, so this is three fetches, not six.
    // Curves are drawn against the sRGB-encoded display, so an export in
    // another space applies them in sRGB encoding too and re-encodes: greys
    // then match the screen exactly.
    if (params.curve_active == 1u) {
        var e = color;
        if (params.output_space != 0u) {
            e = encode_output(decode_output(color, params.output_space), 0u);
        }
        e = vec3<f32>(sample_curve(e.r).r, sample_curve(e.g).g, sample_curve(e.b).b);
        if (params.output_space != 0u) {
            e = encode_output(decode_output(e, 0u), params.output_space);
        }
        color = e;
    }

    // Mask overlay: show the selected mask's coverage while editing it.
    if (params.mask_overlay > 0u) {
        color = mix(color, vec3<f32>(1.0, 0.18, 0.12), overlay_w * 0.55);
    }

    return vec4<f32>(color, 1.0);
}
"#;

/// Get the color science shader (Pass 2 — reads debayered Rgba16Float)
pub fn get_color_shader() -> &'static str {
    PASSTHROUGH_SHADER
}


#[cfg(test)]
mod tests {
    use super::*;

    fn validate_wgsl(source: &str, name: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name} failed to parse: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name} failed validation: {e:?}"));
    }

    #[test]
    fn color_shader_is_valid_wgsl() {
        validate_wgsl(PASSTHROUGH_SHADER, "PASSTHROUGH_SHADER");
    }

    #[test]
    fn shader_mask_constants_match_rust() {
        let expected = format!(
            "const MASK_CHROMA_EPS: f32 = {};",
            crate::core::types::MASK_CHROMA_EPS
        );
        assert!(PASSTHROUGH_SHADER.contains(&expected), "missing `{expected}`");
    }

    /// The tone LUT's domain is encoded on the CPU (at bake time) and decoded
    /// in the shader (at sample time). If the two constants drift, every
    /// lookup lands at the wrong scene-linear value and the whole image
    /// shifts — silently, with no crash and no validation error.
    #[test]
    fn shader_tone_lut_constants_match_rust() {
        let expected = format!(
            "const TONE_LUT_MAX: f32 = {:.1};",
            crate::core::tonemap::TONE_LUT_MAX
        );
        assert!(
            PASSTHROUGH_SHADER.contains(&expected),
            "shader is missing `{expected}` — CPU/GPU tone LUT domains disagree"
        );
    }

    /// `core::perceptual` is what the unit tests check; the shader is what
    /// renders. They only agree while these do.
    #[test]
    fn shader_perceptual_constants_match_rust() {
        use crate::core::perceptual::*;
        for expected in [
            format!("const CONTRAST_PIVOT_L: f32 = {CONTRAST_PIVOT_L:?};"),
            format!("const SAT_KNEE: f32 = {SAT_KNEE:?};"),
            format!("const HK_STRENGTH: f32 = {HK_STRENGTH:?};"),
            format!(
                "const SKIN_DIR: vec2<f32> = vec2<f32>({:?}, {:?});",
                SKIN_DIR[0], SKIN_DIR[1]
            ),
            format!("const SKIN_COS_LO: f32 = {SKIN_COS_LO:?};"),
            format!("const SKIN_COS_HI: f32 = {SKIN_COS_HI:?};"),
            format!("const SKIN_PROTECT: f32 = {SKIN_PROTECT:?};"),
            format!("const GAMUT_THRESHOLD: f32 = {GAMUT_THRESHOLD:?};"),
            format!("const GAMUT_LIMIT: f32 = {GAMUT_LIMIT:?};"),
            format!("const GAMUT_POWER: f32 = {GAMUT_POWER:?};"),
            format!("const GAMUT_L0_ALPHA: f32 = {GAMUT_L0_ALPHA:?};"),
            format!("const GAMUT_SCALE: f32 = {:?};", gamut_compress_scale()),
            format!("const GAMUT_MARCH: f32 = {GAMUT_MARCH:?};"),
            format!("const GAMUT_MARCH_STEPS: u32 = {GAMUT_MARCH_STEPS}u;"),
            format!("const GAMUT_BISECT_STEPS: u32 = {GAMUT_BISECT_STEPS}u;"),
        ] {
            assert!(PASSTHROUGH_SHADER.contains(&expected), "missing `{expected}`");
        }
    }

    /// Highlights/Shadows: `core::tone_regions` is where the curve's
    /// properties (identity at 0, tonal order kept) are tested.
    #[test]
    fn shader_tone_region_constants_match_rust() {
        use crate::core::tone_regions::*;
        for expected in [
            format!("const HS_HL_LO: f32 = {HS_HL_LO:?};"),
            format!("const HS_HL_HI: f32 = {HS_HL_HI:?};"),
            format!("const HS_HL_RECOVER: f32 = {HS_HL_RECOVER:?};"),
            format!("const HS_HL_LIFT: f32 = {HS_HL_LIFT:?};"),
            format!("const HS_SH_LO: f32 = {HS_SH_LO:?};"),
            format!("const HS_SH_HI: f32 = {HS_SH_HI:?};"),
            format!("const HS_SH_LIFT: f32 = {HS_SH_LIFT:?};"),
        ] {
            assert!(PASSTHROUGH_SHADER.contains(&expected), "missing `{expected}`");
        }
    }

    /// Export colour spaces: the shader's matrices and curves are what
    /// renders, `core::output_space` is what the ICC profile describes.
    #[test]
    fn shader_output_space_constants_match_rust() {
        use crate::core::output_space::*;
        let mut expected = vec![
            format!("const ADOBE_RGB_GAMMA: f32 = {ADOBE_RGB_GAMMA:?};"),
            format!("const PROPHOTO_GAMMA: f32 = {PROPHOTO_GAMMA:?};"),
            format!("const PROPHOTO_LINEAR_EDGE: f32 = {PROPHOTO_LINEAR_EDGE:?};"),
        ];
        for (name, space) in
            [("P3", OutputSpace::DisplayP3), ("ADOBE", OutputSpace::AdobeRgb), ("PROPHOTO", OutputSpace::ProPhoto)]
        {
            let m = space.srgb_to_space_matrix();
            for row in 0..3 {
                expected.push(format!(
                    "const OUT_{name}_{row}: vec3<f32> = vec3<f32>({:?}, {:?}, {:?});",
                    m[row * 3],
                    m[row * 3 + 1],
                    m[row * 3 + 2]
                ));
            }
            assert_eq!(space.shader_id(), [0, 1, 2, 3][OutputSpace::ALL.iter().position(|&s| s == space).unwrap()]);
        }
        let missing: Vec<_> = expected.iter().filter(|e| !PASSTHROUGH_SHADER.contains(e.as_str())).collect();
        assert!(missing.is_empty(), "missing:\n{}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"));
    }

    /// Highlight analysis samples only what Step 3 trusts (below its clip
    /// band), and `core::highlights::reconstruct` — what the unit tests
    /// check — mirrors Step 3. Both only hold while the constants agree.
    #[test]
    fn shader_highlight_constants_match_rust() {
        use crate::core::highlights::{HL_CLIP_LO, HL_FADE_LO, HL_ITERATIONS};
        for expected in [
            format!("const HL_CLIP_LO: f32 = {HL_CLIP_LO};"),
            format!("const HL_FADE_LO: f32 = {HL_FADE_LO};"),
            format!("const HL_ITERATIONS: i32 = {HL_ITERATIONS};"),
        ] {
            assert!(PASSTHROUGH_SHADER.contains(&expected), "missing `{expected}`");
        }
    }
}
