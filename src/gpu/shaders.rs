/// WGSL shader code for real-time RAW image processing
///
/// This shader applies non-destructive edits to RAW sensor data in real-time.
/// Full debayering, color science, and advanced adjustments.
///
/// Color pipeline order (scene-referred → display-referred):
///   1.  Debayer          – camera-native linear RGB
///   2.  White Balance    – camera-space per-channel multipliers
///   3.  Color Matrix     – camera → sRGB linear (Bradford D50→D65 adapted)
///   4.  Noise Reduction  – chroma denoising with Rec.709 luma (valid in sRGB)
///   5.  Sharpening       – unsharp mask in sRGB linear
///   6.  Temperature/Tint – blue-yellow and green-magenta axes in sRGB space
///   7.  Highlight clamp  – neutralise near-clipping channels before exposure
///   8.  Exposure         – linear stop adjustment
///   9.  Highlights/Shadows
///  10.  Contrast         – pivoted at 18% gray (perceptual midtone)
///  11.  Levels           – whites / blacks
///  12.  Saturation
///  13.  Vibrance
///  14.  Filmic tone curve (fallback path only — DCP ProfileToneCurve otherwise)
///  15.  sRGB TRC         – IEC 61966-2-1 piecewise (γ=2.4 + linear toe)
///  16.  Clamp
pub const PASSTHROUGH_SHADER: &str = r#"
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

    // Phase 25: Apply zoom and pan transformations
    var tex_x = (x + 1.0) * 0.5;
    var tex_y = (y + 1.0) * 0.5;

    tex_x -= 0.5;
    tex_y -= 0.5;

    tex_x /= params.zoom;
    tex_y /= params.zoom;

    tex_x -= params.pan_x;
    tex_y -= params.pan_y;

    tex_x += 0.5;
    tex_y += 0.5;

    // Phase 135: Conditional Crop
    // When is_cropping is active, we render the full image bounds [0, 1]
    // allowing the user to see and adjust the crop handles non-destructively.
    if (params.is_cropping == 0u) {
        tex_x = params.crop.x + (tex_x * params.crop.z);
        tex_y = params.crop.y + (tex_y * params.crop.w);
    }

    output.tex_coords = vec2<f32>(tex_x, tex_y);

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
    padding1: f32,
    padding2: f32,
    // Phase 14: Color science metadata
    wb_multipliers: vec4<f32>,  // White balance [R, G, B, G2]
    forward_matrix_0: vec3<f32>,  // Forward matrix row 0
    padding3: f32,
    forward_matrix_1: vec3<f32>,  // Forward matrix row 1
    padding4: f32,
    forward_matrix_2: vec3<f32>,  // Forward matrix row 2
    padding5: f32,
    // Phase 25: Zoom & Pan
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
    // Phase 34: CFA Pattern
    cfa_pattern: u32,

    // Padding to align black_levels to 16 bytes
    pad_cfa_1: f32,
    pad_cfa_2: f32,
    pad_cfa_3: f32,
    pad_cfa_4: f32,

    // Phase 36: Per-channel Black Levels (vec4 alignment = 16 bytes)
    black_levels: vec4<u32>,

    white_level: u32,

    // Padding to reach 176 bytes
    pad_end_1: f32,
    pad_end_2: f32,
    pad_end_3: f32,

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

    // Padding to reach 224 bytes
    pad_phase_1: f32,

    // Phase 66: Crop (vec4 alignment = 16 bytes)
    crop: vec4<f32>,

    // Phase 135: Non-destructive crop visibility
    is_cropping: u32,

    // Padding to reach 256 bytes (16-byte alignment)
    has_dcp: u32,
    dcp_has_curve: u32,
    pad_crop_3: u32,
}

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

// Sample the DCP ProfileToneCurve at texel centers.
// The curve texture stores N samples of f(i/(N-1)); naive normalized-coordinate
// sampling is skewed by half a texel at both ends.
fn sample_tone_curve(x: f32) -> f32 {
    let n = f32(textureDimensions(tone_curve));
    let c = (clamp(x, 0.0, 1.0) * (n - 1.0) + 0.5) / n;
    return textureSampleLevel(tone_curve, lut_sampler, c, 0.0).r;
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

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Discard fragments outside texture bounds (letterboxing when zoomed out)
    if input.tex_coords.x < 0.0 || input.tex_coords.x > 1.0 ||
       input.tex_coords.y < 0.0 || input.tex_coords.y > 1.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    // Phase 52: Apply rotation to texture coordinates
    var tex_coords = input.tex_coords;

    if (abs(params.rotation) > 0.01) {
        let dimensions_r = textureDimensions(input_texture);
        let aspect = f32(dimensions_r.x) / f32(dimensions_r.y);
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

    let dimensions = textureDimensions(input_texture);

    let pixel_coords = vec2<i32>(
        i32(tex_coords.x * f32(dimensions.x)),
        i32(tex_coords.y * f32(dimensions.y))
    );

    // ── Step 1: Read debayered camera-native linear RGB from intermediate ────
    // var color = textureLoad(input_texture, pixel_coords, 0).rgb;
    // Phase 135: Use textureSample with Linear Filtering for smooth downscaling
    var color = textureSample(input_texture, texture_sampler, tex_coords).rgb;

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
    // STAGE 2 gamut compression handles overbrights post-matrix without artifacts.)

    // ── Step 2: White Balance ─────────────────────────────────────────────────
    color = color * params.wb_multipliers.rgb;

    // Pre-fetch the 8 nearest camera-space neighbours (WB-scaled).
    // Both Phase 133 (NR) and Phase 132 (sharpening) need this ring — fetching
    // it once here saves 8 textureLoad calls when both controls are active.
    // The branch is uniform across the warp (params are constants), so no divergence.
    var nb_n  = vec3<f32>(0.0); var nb_s  = vec3<f32>(0.0);
    var nb_e  = vec3<f32>(0.0); var nb_w  = vec3<f32>(0.0);
    var nb_ne = vec3<f32>(0.0); var nb_nw = vec3<f32>(0.0);
    var nb_se = vec3<f32>(0.0); var nb_sw = vec3<f32>(0.0);
    if (params.luma_noise > 0.0 || params.color_noise > 0.0 || params.sharpening > 0.0) {
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

    // ── Phase 133: Split Luma & Chroma Noise Reduction ───────────────────────
    if (params.luma_noise > 0.0 || params.color_noise > 0.0) {
        // Green-dominant weights: green pixels are ~2× denser in a Bayer pattern
        // and carry the most spatial detail. Equal weights (0.333) under-weight green,
        // causing the bilateral to treat green/red (or green/blue) brightness
        // differences as edges rather than noise.
        let cam_luma_weights = vec3<f32>(0.25, 0.60, 0.15);

        let c_Y = dot(color, cam_luma_weights);
        let c_C = color - vec3<f32>(c_Y);

        var sum_Y = c_Y;
        var weight_Y = 1.0;
        var sum_C = c_C;
        var weight_C = 1.0;

        // Bilateral sigma: range sensitivity derived from noise strength.
        // sigma_r ≈ 0.05 at luma_noise=1 — blends neighbours within ~5% brightness.
        let sigma_r_sq = (0.05 * params.luma_noise) * (0.05 * params.luma_noise);

        // 3×3 bilateral luma NR + first pass of chroma accumulation.
        // Uses the pre-fetched nb_* ring above — no extra texture reads here.
        // Must be `var` (function address space) so dynamic indexing via `i` is valid.
        var ring3 = array<vec3<f32>, 8>(nb_n, nb_s, nb_e, nb_w, nb_ne, nb_nw, nb_se, nb_sw);
        for (var i: u32 = 0u; i < 8u; i++) {
            let n_color = ring3[i];
            let n_Y = dot(n_color, cam_luma_weights);
            let n_C = n_color - vec3<f32>(n_Y);

            if (params.luma_noise > 0.0) {
                let diff_sq = (c_Y - n_Y) * (c_Y - n_Y);
                let w_Y = exp(-diff_sq / (sigma_r_sq + 0.0001));
                sum_Y += n_Y * w_Y;
                weight_Y += w_Y;
            }

            if (params.color_noise > 0.0) {
                sum_C += n_C * params.color_noise;
                weight_C += params.color_noise;
            }
        }

        // Extend chroma blur to 5×5: human vision has low chroma acuity so a larger
        // kernel removes colour mottling without visible hue smearing.
        // (The 3×3 ring is already accumulated above; here we add the outer ring only.)
        if (params.color_noise > 0.0) {
            for (var x: i32 = -2; x <= 2; x++) {
                for (var y: i32 = -2; y <= 2; y++) {
                    if (abs(x) <= 1 && abs(y) <= 1) { continue; }
                    let clamped_coords = clamp(pixel_coords + vec2<i32>(x, y), vec2<i32>(0), vec2<i32>(dimensions) - vec2<i32>(1));
                    let n_color = textureLoad(input_texture, clamped_coords, 0).rgb * params.wb_multipliers.rgb;
                    let n_Y = dot(n_color, cam_luma_weights);
                    let n_C = n_color - vec3<f32>(n_Y);
                    sum_C += n_C * params.color_noise;
                    weight_C += params.color_noise;
                }
            }
        }

        let final_Y = sum_Y / weight_Y;
        let final_C = sum_C / weight_C;
        color = vec3<f32>(final_Y) + final_C;
    }

    // ── Step 3: Highlight neutralisation (camera space, per-channel) ──────────
    // When a raw sensor channel clips (hits digital saturation at raw = 1.0), its
    // WB-amplified value reaches wb_multipliers.channel × 1.0 = wb_multipliers.channel.
    // Adjacent Bayer-opposite pixels (e.g. a G site next to a clipped B sky pixel) have
    // the clipped channel interpolated from their neighbours by the debayer pass, so
    // color.b at the G site also approaches wb_multipliers.b.  Comparing each channel's
    // debayered value against its own WB multiplier therefore produces a spatially-smooth
    // blend weight — free of the 2-pixel Bayer-period checkerboard that appeared when
    // the old code used the per-Bayer-site raw alpha instead.
    //
    // This runs before the exposure step so the correction is visible when exposure is
    // pulled down (which brings clipped areas into the visible range as magenta).
    let wb3 = params.wb_multipliers.rgb;
    let hl_clip_r = smoothstep(wb3.r * 0.90, wb3.r, color.r);
    let hl_clip_g = smoothstep(wb3.g * 0.90, wb3.g, color.g);
    let hl_clip_b = smoothstep(wb3.b * 0.90, wb3.b, color.b);
    let hl_clip_blend = max(hl_clip_r, max(hl_clip_g, hl_clip_b));
    if (hl_clip_blend > 0.0) {
        let hl_max = max(color.r, max(color.g, color.b));
        color = mix(color, vec3<f32>(hl_max), hl_clip_blend);
    }

    // ── Step 4: Exposure ──────────────────────────────────────────────────────
    let exposure_multiplier = pow(2.0, params.exposure);
    color = color * exposure_multiplier;

    // ── Step 5: Camera RGB → sRGB (DCP Pipeline vs Fallback) ────────────────
    // NOTE: WB was already applied in Step 2 above. Do NOT re-apply here.

    let matrix_from_params = transpose(mat3x3<f32>(
        params.forward_matrix_0,
        params.forward_matrix_1,
        params.forward_matrix_2
    ));

    if (params.has_dcp == 1u) {
        // 1. Camera RGB → XYZ D50 via ForwardMatrix
        var xyz = matrix_from_params * color;

        // 2. XYZ D50 → ProPhoto RGB (ProPhoto uses D50, no Bradford needed)
        let xyz_to_prophoto = mat3x3<f32>(
            1.3459433, -0.5445989,  0.0000000,
        -0.2556075,  1.5081673,  0.0000000,
        -0.0511118,  0.0205351,  1.2118128
        );
        // No pre-clamp here — negative ProPhoto values (wide-gamut colors) must reach
        // the HueSatMap with correct hue/saturation. We clamp after reconstruction.
        var pp = xyz_to_prophoto * xyz;

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
        pp.r = sample_tone_curve(pp.r);
        pp.g = sample_tone_curve(pp.g);
        pp.b = sample_tone_curve(pp.b);

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

    // ── Step 6: Clamp Negatives ───────────────────────────────────────────────
    // The camera→sRGB matrix contains negative cross-talk values. Any surviving
    // negative channel would poison luma coefficients and invert colours.
    // Must clamp BEFORE luma calculations or gamut compression.
    color = max(color, vec3<f32>(0.0));

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

    // ── Step 10: Highlights & Shadows (before gamut compression) ─────────────
    // Highlights runs here — before STAGE 2 — so overexposed areas that are still
    // above 1.0 in linear space can be pulled back below the gamut-compression
    // threshold.  Running it after would only compress values already flattened
    // into the narrow 0.8–1.0 band, wiping out cloud/fabric texture.
    //
    // Formula: multiplicative scale weighted by luma (ratio-preserving).
    // Scaling the full pixel keeps colour ratios intact so partially-overexposed
    // areas (e.g. warm-white clouds at R>G>B) stay warm rather than shifting to
    // neutral grey.  Truly clipped pixels (all channels equal ≥ 1) have no colour
    // info to preserve and will still go neutral — that's a fundamental sensor limit.
    if (params.highlights != 0.0) {
        let luma_hl = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
        // Smooth weight: 0 at luma≤0.5 (midtones untouched), 1 at luma≥1.5+
        let hl_weight = smoothstep(0.5, 1.5, luma_hl);
        if (params.highlights < 0.0) {
            // Recovery: compress the full pixel.  At -1 + luma≥1.5: factor≈0.4.
            let hl_factor = max(1.0 + params.highlights * 0.6 * hl_weight, 0.2);
            color = color * hl_factor;
        } else {
            // Lift: expand upper tones for a bright/airy look.
            let hl_factor = 1.0 + params.highlights * 0.4 * hl_weight;
            color = color * hl_factor;
        }
    }
    if (params.shadows != 0.0) {
        let sh_norm   = params.shadows * 0.15;
        let sh_luma   = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
        let sh_weight = clamp(1.0 - sh_luma / 0.5, 0.0, 1.0);
        color = color + vec3<f32>(sh_weight * sh_norm);
    }

    // ── STAGE 2: Gamut Compression (True Path-to-White) ──────────────────────
    // Mix toward the peak channel value (neutral at that brightness).  This fixes
    // the magenta/colour cast that appears in partially-clipped sensor highlights:
    // when the blue raw channel clips before red and green, the wrong channel ratios
    // pass through the forward matrix and appear pink/magenta in sRGB.  Correcting
    // here (post-matrix) means the blend operates on spatially-smooth sRGB values
    // rather than per-Bayer-site camera values, so there is no checkerboard.
    //
    // Step 3 (camera space, pre-exposure) now handles sensor-clipping colour casts.
    // STAGE 2 remains as a gentle safety net for any post-matrix overbrights that
    // survive (e.g. extreme exposure boosts) — range (0, 2.0) is intentionally light
    // so legitimate saturated colours that are slightly over 1.0 are not desaturated.
    let max_c_gc = max(color.r, max(color.g, color.b));
    let overbright = max(0.0, max_c_gc - 1.0);
    let path_to_white = smoothstep(0.0, 2.0, overbright);
    color = mix(color, vec3<f32>(max_c_gc), path_to_white);

    // ── Step 9: Temperature & Tint (in sRGB space) ───────────────────────────
    color.r *= (1.0 + params.temperature * 0.3);
    color.b *= (1.0 - params.temperature * 0.3);
    color.g *= (1.0 + params.tint * 0.3);

    // ── Step 11: Contrast ─────────────────────────────────────────────────────
    let contrast_factor = 1.0 + (params.contrast / 100.0);
    color = (color - 0.18) * contrast_factor + 0.18;

    // ── Step 12: Levels (Whites & Blacks) ────────────────────────────────────
    color = (color - vec3<f32>(params.blacks)) / vec3<f32>(params.whites - params.blacks + 0.0001);

    // ── Step 13: Saturation ───────────────────────────────────────────────────
    var luma = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let sat_factor = 1.0 + (params.saturation / 100.0);
    color = mix(vec3<f32>(luma), color, sat_factor);

    // ── Step 14: Vibrance ─────────────────────────────────────────────────────
    let chroma = max(color.r, max(color.g, color.b)) - min(color.r, min(color.g, color.b));
    let vibrance_amount = params.vibrance * (1.0 - chroma);
    luma = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    color = mix(vec3<f32>(luma), color, 1.0 + vibrance_amount);

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
    color = max(color, vec3<f32>(0.0));
    if (params.has_dcp == 0u || params.dcp_has_curve == 0u) {
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

    // ── Step 15: sRGB Transfer Function (IEC 61966-2-1) ──────────────────────
    color = vec3<f32>(
        linear_to_srgb(color.r),
        linear_to_srgb(color.g),
        linear_to_srgb(color.b)
    );

    // ── Step 16: Clamp to display range ──────────────────────────────────────
    color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));

    return vec4<f32>(color, 1.0);
}
"#;

// ═══════════════════════════════════════════════════════════════════════════
// Phase 128: DEBAYER SHADER (Pass 1)
// Reads the R16Uint raw sensor texture and writes debayered camera-native
// linear RGB to an Rgba16Float intermediate texture.  No color science,
// no WB, no exposure — just geometry.
// ═══════════════════════════════════════════════════════════════════════════

pub const DEBAYER_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

// Simple full-screen triangle — no zoom, pan, or crop.  1:1 mapping to the
// intermediate texture at full sensor resolution.
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var output: VertexOutput;
    let x = f32(i32(vertex_index & 1u) * 4 - 1);
    let y = f32(i32(vertex_index >> 1u) * 4 - 1);
    output.clip_position = vec4<f32>(x, -y, 0.0, 1.0);
    output.tex_coords = vec2<f32>((x + 1.0) * 0.5, (y + 1.0) * 0.5);
    return output;
}

// ── EditParams struct (must match Rust GpuEditParams layout exactly) ─────
struct EditParams {
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
    padding1: f32,
    padding2: f32,
    wb_multipliers: vec4<f32>,
    forward_matrix_0: vec3<f32>,
    padding3: f32,
    forward_matrix_1: vec3<f32>,
    padding4: f32,
    forward_matrix_2: vec3<f32>,
    padding5: f32,
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
    cfa_pattern: u32,
    pad_cfa_1: f32,
    pad_cfa_2: f32,
    pad_cfa_3: f32,
    pad_cfa_4: f32,
    black_levels: vec4<u32>,
    white_level: u32,
    pad_end_1: f32,
    pad_end_2: f32,
    pad_end_3: f32,
    black_offsets: vec4<f32>,
    black_phase_x: u32,
    black_phase_y: u32,
    luma_noise: f32,
    color_noise: f32,
    sharpening: f32,
    sharpen_masking: f32,
    rotation: f32,
    pad_phase_1: f32,
    // Phase 66: Crop (vec4 alignment = 16 bytes)
    crop: vec4<f32>,

    // Phase 135: Non-destructive crop visibility
    is_cropping: u32,

    // Padding to reach 256 bytes
    has_dcp: u32,
    dcp_has_curve: u32,
    pad_crop_3: u32,
}

@group(0) @binding(0)
var input_texture: texture_2d<u32>;

@group(0) @binding(1)
var texture_sampler: sampler;

@group(0) @binding(2)
var<uniform> params: EditParams;

// ── Debayer helpers (identical to the ones in the old single-pass shader) ─

fn get_neighbor(coords: vec2<i32>, dimensions: vec2<u32>) -> f32 {
    let clamped = vec2<i32>(
        clamp(coords.x, 0, i32(dimensions.x) - 1),
        clamp(coords.y, 0, i32(dimensions.y) - 1)
    );
    let raw_value = textureLoad(input_texture, clamped, 0).r;
    let x = u32(clamped.x);
    let y = u32(clamped.y);
    let is_even_row = (y & 1u) == 0u;
    let is_even_col = (x & 1u) == 0u;
    var bl_index: u32;
    if (params.cfa_pattern == 0u) {
        if (is_even_row && is_even_col) { bl_index = 0u; }
        else if (is_even_row && !is_even_col) { bl_index = 1u; }
        else if (!is_even_row && is_even_col) { bl_index = 2u; }
        else { bl_index = 3u; }
    } else if (params.cfa_pattern == 1u) {
        if (is_even_row && is_even_col) { bl_index = 1u; }
        else if (is_even_row && !is_even_col) { bl_index = 0u; }
        else if (!is_even_row && is_even_col) { bl_index = 3u; }
        else { bl_index = 2u; }
    } else if (params.cfa_pattern == 2u) {
        if (is_even_row && is_even_col) { bl_index = 1u; }
        else if (is_even_row && !is_even_col) { bl_index = 3u; }
        else if (!is_even_row && is_even_col) { bl_index = 0u; }
        else { bl_index = 2u; }
    } else {
        if (is_even_row && is_even_col) { bl_index = 3u; }
        else if (is_even_row && !is_even_col) { bl_index = 1u; }
        else if (!is_even_row && is_even_col) { bl_index = 2u; }
        else { bl_index = 0u; }
    }
    let black = f32(params.black_levels[bl_index]) + params.black_offsets[bl_index];
    let white = f32(params.white_level);
    let corrected = max(0.0, f32(raw_value) - black);
    let range = max(1.0, white - black);
    return corrected / range;
}

fn debayer(coords: vec2<i32>, dimensions: vec2<u32>) -> vec3<f32> {
    let raw_value = textureLoad(input_texture, coords, 0).r;
    let x = u32(coords.x);
    let y = u32(coords.y);
    let is_even_row = (y & 1u) == 0u;
    let is_even_col = (x & 1u) == 0u;
    var bl_index: u32;
    if (params.cfa_pattern == 0u) {
        if (is_even_row && is_even_col) { bl_index = 0u; }
        else if (is_even_row && !is_even_col) { bl_index = 1u; }
        else if (!is_even_row && is_even_col) { bl_index = 2u; }
        else { bl_index = 3u; }
    } else if (params.cfa_pattern == 1u) {
        if (is_even_row && is_even_col) { bl_index = 1u; }
        else if (is_even_row && !is_even_col) { bl_index = 0u; }
        else if (!is_even_row && is_even_col) { bl_index = 3u; }
        else { bl_index = 2u; }
    } else if (params.cfa_pattern == 2u) {
        if (is_even_row && is_even_col) { bl_index = 1u; }
        else if (is_even_row && !is_even_col) { bl_index = 3u; }
        else if (!is_even_row && is_even_col) { bl_index = 0u; }
        else { bl_index = 2u; }
    } else {
        if (is_even_row && is_even_col) { bl_index = 3u; }
        else if (is_even_row && !is_even_col) { bl_index = 1u; }
        else if (!is_even_row && is_even_col) { bl_index = 2u; }
        else { bl_index = 0u; }
    }
    let black = f32(params.black_levels[bl_index]) + params.black_offsets[bl_index];
    let white = f32(params.white_level);
    let corrected = max(0.0, f32(raw_value) - black);
    let range = max(1.0, white - black);
    let normalized = corrected / range;
    var is_red = false;
    var is_green = false;
    var is_blue = false;
    if (params.cfa_pattern == 0u) {
        if (is_even_row && is_even_col) { is_red = true; }
        else if (is_even_row && !is_even_col) { is_green = true; }
        else if (!is_even_row && is_even_col) { is_green = true; }
        else { is_blue = true; }
    } else if (params.cfa_pattern == 1u) {
        if (is_even_row && is_even_col) { is_green = true; }
        else if (is_even_row && !is_even_col) { is_red = true; }
        else if (!is_even_row && is_even_col) { is_blue = true; }
        else { is_green = true; }
    } else if (params.cfa_pattern == 2u) {
        if (is_even_row && is_even_col) { is_green = true; }
        else if (is_even_row && !is_even_col) { is_blue = true; }
        else if (!is_even_row && is_even_col) { is_red = true; }
        else { is_green = true; }
    } else {
        if (is_even_row && is_even_col) { is_blue = true; }
        else if (is_even_row && !is_even_col) { is_green = true; }
        else if (!is_even_row && is_even_col) { is_green = true; }
        else { is_red = true; }
    }
    let n  = get_neighbor(coords + vec2<i32>( 0, -1), dimensions);
    let s  = get_neighbor(coords + vec2<i32>( 0,  1), dimensions);
    let w  = get_neighbor(coords + vec2<i32>(-1,  0), dimensions);
    let e  = get_neighbor(coords + vec2<i32>( 1,  0), dimensions);
    let nw = get_neighbor(coords + vec2<i32>(-1, -1), dimensions);
    let ne = get_neighbor(coords + vec2<i32>( 1, -1), dimensions);
    let sw = get_neighbor(coords + vec2<i32>(-1,  1), dimensions);
    let se = get_neighbor(coords + vec2<i32>( 1,  1), dimensions);
    var rgb: vec3<f32>;
    if (is_red) {
        let r = normalized;
        let grad_v = abs(n - s); let grad_h = abs(e - w);
        var g: f32;
        if (grad_v < grad_h) { g = (n + s) * 0.5; }
        else if (grad_h < grad_v) { g = (e + w) * 0.5; }
        else { g = (n + s + e + w) * 0.25; }
        let grad_nesw = abs(ne - sw); let grad_nwse = abs(nw - se);
        var b: f32;
        if (grad_nesw < grad_nwse) { b = (ne + sw) * 0.5; }
        else if (grad_nwse < grad_nesw) { b = (nw + se) * 0.5; }
        else { b = (ne + sw + nw + se) * 0.25; }
        rgb = vec3<f32>(r, g, b);
    } else if (is_even_row && !is_even_col) {
        // Gr pixel: R from axis neighbors, B from cross-axis neighbors.
        // All 4 diagonals (nw/ne/sw/se) are Gb sites → raw values are Green.
        // Color-correlation correction: assume the local R/G (and B/G) ratio
        // changes slowly, so correct interpolated R/B by the local green gradient.
        // This removes the 0.5-pixel green bias that causes color fringing.
        let g = normalized;
        let g_diag = (nw + ne + sw + se) * 0.25;
        let g_corr = (g - g_diag) * 0.5;
        let r = (w + e) * 0.5 + g_corr;
        let b = (n + s) * 0.5 + g_corr;
        rgb = vec3<f32>(r, g, b);
    } else if (!is_even_row && is_even_col) {
        // Gb pixel: R from cross-axis, B from axis neighbors.
        // All 4 diagonals are Gr sites → raw values are Green. Same correction.
        let g = normalized;
        let g_diag = (nw + ne + sw + se) * 0.25;
        let g_corr = (g - g_diag) * 0.5;
        let r = (n + s) * 0.5 + g_corr;
        let b = (w + e) * 0.5 + g_corr;
        rgb = vec3<f32>(r, g, b);
    } else {
        let b = normalized;
        let grad_v = abs(n - s); let grad_h = abs(e - w);
        var g: f32;
        if (grad_v < grad_h) { g = (n + s) * 0.5; }
        else if (grad_h < grad_v) { g = (e + w) * 0.5; }
        else { g = (n + s + e + w) * 0.25; }
        let grad_nesw = abs(ne - sw); let grad_nwse = abs(nw - se);
        var r: f32;
        if (grad_nesw < grad_nwse) { r = (ne + sw) * 0.5; }
        else if (grad_nwse < grad_nesw) { r = (nw + se) * 0.5; }
        else { r = (ne + sw + nw + se) * 0.25; }
        rgb = vec3<f32>(r, g, b);
    }
    return rgb;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let dimensions = textureDimensions(input_texture);
    let pixel_coords = vec2<i32>(
        i32(input.tex_coords.x * f32(dimensions.x)),
        i32(input.tex_coords.y * f32(dimensions.y))
    );
    let color = debayer(pixel_coords, dimensions);
    // Alpha stores the per-Bayer-site normalised raw value for smooth clip detection
    // in the passthrough shader.  get_neighbor returns the same black/white corrected
    // value that debayer() uses for `normalized`, so the two are consistent.
    return vec4<f32>(color, get_neighbor(pixel_coords, dimensions));
}
"#;

/// Get the color science shader (Pass 2 — reads debayered Rgba16Float)
pub fn get_color_shader() -> &'static str {
    PASSTHROUGH_SHADER
}

/// Get the debayer shader (Pass 1 — reads R16Uint raw data)
pub fn get_debayer_shader() -> &'static str {
    DEBAYER_SHADER
}

/// Legacy alias — returns the color shader for backward compatibility
pub fn get_shader() -> &'static str {
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
    fn debayer_shader_is_valid_wgsl() {
        validate_wgsl(DEBAYER_SHADER, "DEBAYER_SHADER");
    }
}
