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
///  14.  ACES filmic tone curve
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
    pad_crop_2: u32,
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

    // ── STAGE 1: Sensor Clipping Detection (BEFORE WB / Exposure) ────────────
    // Measure destruction on the raw debayered values so WB gains and exposure
    // cannot amplify a clipped channel to look brighter than its neighbours.
    let pre_wb_max = max(color.r, max(color.g, color.b));
    let clip_blend = smoothstep(0.94, 0.98, pre_wb_max);

    // ── Step 2: White Balance ─────────────────────────────────────────────────
    color = color * params.wb_multipliers.rgb;

    // ── Phase 133: Split Luma & Chroma Noise Reduction ───────────────────────
    if (params.luma_noise > 0.0 || params.color_noise > 0.0) {
        // Simple equal-weight luma for camera RGB space
        let cam_luma_weights = vec3<f32>(0.3333, 0.3333, 0.3333);
        
        // Separate Center Pixel into Brightness (Y) and Color (C)
        let c_Y = dot(color, cam_luma_weights);
        let c_C = color - vec3<f32>(c_Y);
        
        var sum_Y = c_Y;
        var weight_Y = 1.0;
        
        var sum_C = c_C;
        var weight_C = 1.0;
        
        // The strictness of the edge-preservation for Luma
        let edge_stop = 10.0 / (params.luma_noise + 0.001);
        
        // Iterate over a 3x3 grid (skip center 0,0)
        for (var x: i32 = -1; x <= 1; x++) {
            for (var y: i32 = -1; y <= 1; y++) {
                if (x == 0 && y == 0) { continue; }
                
                // Fetch neighbor and apply WB
                let offset = vec2<i32>(x, y);
                let clamped_coords = clamp(pixel_coords + offset, vec2<i32>(0), vec2<i32>(dimensions) - vec2<i32>(1));
                let n_color = textureLoad(input_texture, clamped_coords, 0).rgb * params.wb_multipliers.rgb;
                
                // Separate Neighbor into Y and C
                let n_Y = dot(n_color, cam_luma_weights);
                let n_C = n_color - vec3<f32>(n_Y);
                
                // LUMA DENOISE: Edge-Avoiding Bilateral Filter
                // Only blend brightness if the neighbor is similar in brightness to the center
                let diff_Y = abs(c_Y - n_Y);
                let w_Y = exp(-diff_Y * edge_stop) * params.luma_noise;
                sum_Y = sum_Y + (n_Y * w_Y);
                weight_Y = weight_Y + w_Y;
                
                // COLOR DENOISE: Standard Box/Gaussian Blur
                // We aggressively blur color because human eyes don't see color edges well
                let w_C = params.color_noise; 
                sum_C = sum_C + (n_C * w_C);
                weight_C = weight_C + w_C;
            }
        }
        
        // Recombine Denoised Brightness and Denoised Color
        let final_Y = sum_Y / weight_Y;
        let final_C = sum_C / weight_C;
        color = vec3<f32>(final_Y) + final_C;
    }

    // ── Step 3: Highlight Neutralisation (fixes magenta sky at -5 EV) ────────
    // Blend clipped pixels toward grey proportional to clip_blend.
    // We use max_c (the WB-scaled maximum) so the grey level tracks exposure.
    let max_c = max(color.r, max(color.g, color.b));
    color = mix(color, vec3<f32>(max_c), clip_blend);

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

        let lut_val = textureSampleLevel(hsv_lut, lut_sampler, vec3<f32>(h, s, clamp(v, 0.0, 1.0)), 0.0);
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
        pp.r = textureSampleLevel(tone_curve, lut_sampler, clamp(pp.r, 0.0, 1.0), 0.0).r;
        pp.g = textureSampleLevel(tone_curve, lut_sampler, clamp(pp.g, 0.0, 1.0), 0.0).r;
        pp.b = textureSampleLevel(tone_curve, lut_sampler, clamp(pp.b, 0.0, 1.0), 0.0).r;

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

    // ── Phase 132: Perceptual Unsharp Mask ─────────────────────────────────
    // sqrt() compresses linear light into a human-vision curve so the blur
    // and detail extraction are mathematically uniform across shadows and
    // highlights.  Squaring converts back to linear for ratio-preserving scale.
    if (params.sharpening > 0.0) {
        let luma_weights = vec3<f32>(0.2126, 0.7152, 0.0722);

        // 1. Sample center and neighbours in perceptual (sqrt) space
        let c_luma  = sqrt(max(0.0001, dot(color, luma_weights)));
        let n_luma  = sqrt(max(0.0001, dot(get_srgb_neighbor(pixel_coords + vec2<i32>( 0, -1), dimensions, matrix_from_params), luma_weights)));
        let s_luma  = sqrt(max(0.0001, dot(get_srgb_neighbor(pixel_coords + vec2<i32>( 0,  1), dimensions, matrix_from_params), luma_weights)));
        let e_luma  = sqrt(max(0.0001, dot(get_srgb_neighbor(pixel_coords + vec2<i32>( 1,  0), dimensions, matrix_from_params), luma_weights)));
        let w_luma  = sqrt(max(0.0001, dot(get_srgb_neighbor(pixel_coords + vec2<i32>(-1,  0), dimensions, matrix_from_params), luma_weights)));
        let nw_luma = sqrt(max(0.0001, dot(get_srgb_neighbor(pixel_coords + vec2<i32>(-1, -1), dimensions, matrix_from_params), luma_weights)));
        let ne_luma = sqrt(max(0.0001, dot(get_srgb_neighbor(pixel_coords + vec2<i32>( 1, -1), dimensions, matrix_from_params), luma_weights)));
        let sw_luma = sqrt(max(0.0001, dot(get_srgb_neighbor(pixel_coords + vec2<i32>(-1,  1), dimensions, matrix_from_params), luma_weights)));
        let se_luma = sqrt(max(0.0001, dot(get_srgb_neighbor(pixel_coords + vec2<i32>( 1,  1), dimensions, matrix_from_params), luma_weights)));

        // 2. 3×3 Gaussian blur in perceptual space
        let blur_luma = (c_luma * 0.25)
                      + ((n_luma + s_luma + e_luma + w_luma) * 0.125)
                      + ((nw_luma + ne_luma + sw_luma + se_luma) * 0.0625);

        // 3. Extract perceptual detail
        let high_pass = c_luma - blur_luma;

        // 4. Add detail back and convert to linear (square)
        let sharpened_perc = max(0.0, c_luma + (high_pass * params.sharpening * 2.0));
        let new_linear_luma = sharpened_perc * sharpened_perc;
        let old_linear_luma = c_luma * c_luma;  // == original linear luma (clamped)

        // 5. Ratio-preserving scale
        let sharpen_scale = new_linear_luma / old_linear_luma;
        color = color * sharpen_scale;
    }

    // ── STAGE 2: Gamut Compression (True Path-to-White) ──────────────────────
    // Mix toward the peak channel, not luma. This lifts the weaker channels up
    // to match the brightest one, desaturating to a brilliant pure white without
    // crushing brightness energy. Mixing toward luma would pull everything down.
    let max_c_gc = max(color.r, max(color.g, color.b));
    let overbright = max(0.0, max_c_gc - 1.0);
    let path_to_white = smoothstep(0.0, 2.0, overbright);
    color = mix(color, vec3<f32>(max_c_gc), path_to_white);

    // ── Step 9: Temperature & Tint (in sRGB space) ───────────────────────────
    color.r *= (1.0 + params.temperature * 0.3);
    color.b *= (1.0 - params.temperature * 0.3);
    color.g *= (1.0 + params.tint * 0.3);

    // ── Step 10: Highlights & Shadows ────────────────────────────────────────
    if (params.highlights != 0.0) {
        let hl_scale = max(1.0 + params.highlights / 100.0, 0.0);
        let hl_over  = max(color - vec3<f32>(0.5), vec3<f32>(0.0));
        color = min(color, vec3<f32>(0.5)) + hl_over * hl_scale;
    }

    if (params.shadows != 0.0) {
        let sh_norm   = params.shadows / 100.0;
        let sh_luma   = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
        let sh_weight = clamp(1.0 - sh_luma / 0.5, 0.0, 1.0);
        color = color + vec3<f32>(sh_weight * sh_norm * 0.4);
    }

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

    // ── Step 15: Photographic Highlight Roll-off (Threshold Compression) ─────
    // The bottom 80% of the tonal range is 100% linear — no compression, full punch.
    // Only values above the threshold are smoothly compressed into the remaining
    // 20% of display headroom using a local Reinhard sub-curve.
    color = max(color, vec3<f32>(0.0));
    let pre_tone_max = max(color.r, max(color.g, color.b));
    if (pre_tone_max > 0.0) {
        let threshold: f32 = 0.8;
        var post_tone_max: f32;
        if (pre_tone_max <= threshold) {
            post_tone_max = pre_tone_max; // strictly linear below threshold
        } else {
            let over_t    = pre_tone_max - threshold;
            let headroom  = 1.0 - threshold; // == 0.2
            post_tone_max = threshold + (headroom * over_t) / (over_t + headroom);
        }
        color = color * (post_tone_max / pre_tone_max);
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
    pad_crop_2: u32,
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
        let g = normalized;
        let r = (w + e) * 0.5;
        let b = (n + s) * 0.5;
        rgb = vec3<f32>(r, g, b);
    } else if (!is_even_row && is_even_col) {
        let g = normalized;
        let r = (n + s) * 0.5;
        let b = (w + e) * 0.5;
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
    return vec4<f32>(color, 1.0);
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
