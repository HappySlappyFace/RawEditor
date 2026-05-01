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

    // Phase 66: Apply Crop
    tex_x = params.crop.x + (tex_x * params.crop.z);
    tex_y = params.crop.y + (tex_y * params.crop.w);

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
    color_matrix_0: vec3<f32>,  // Color matrix row 0
    padding3: f32,
    color_matrix_1: vec3<f32>,  // Color matrix row 1
    padding4: f32,
    color_matrix_2: vec3<f32>,  // Color matrix row 2
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

    // Phase 49: Noise Reduction
    noise_reduction: f32,

    // Phase 50: Sharpening
    sharpening: f32,

    // Phase 51: Sharpening Masking
    sharpen_masking: f32,

    // Phase 52: Rotation
    rotation: f32,

    // Padding to reach 224 bytes
    pad_phase_1: f32,
    pad_phase_2: f32,

    // Phase 66: Crop (vec4 alignment = 16 bytes)
    crop: vec4<f32>,
    // Total: 240 bytes
}

@group(0) @binding(0)
var input_texture: texture_2d<u32>;  // RAW u16 data stored as u32

@group(0) @binding(1)
var texture_sampler: sampler;  // Not used for integer textures, kept for compatibility

@group(0) @binding(2)
var<uniform> params: EditParams;

// Simple nearest-neighbor debayering with CFA pattern support
// CFA patterns: 0=RGGB, 1=GRBG, 2=GBRG, 3=BGGR
fn debayer(coords: vec2<i32>, dimensions: vec2<u32>) -> vec3<f32> {
    let raw_value = textureLoad(input_texture, coords, 0).r;

    let x = u32(coords.x);
    let y = u32(coords.y);
    let is_even_row = (y & 1u) == 0u;
    let is_even_col = (x & 1u) == 0u;

    // Determine CFA color index for this pixel based on pattern
    // black_levels[0]=R, [1]=G1 (red row), [2]=G2 (blue row), [3]=B
    var bl_index: u32;

    if (params.cfa_pattern == 0u) { // RGGB
        if (is_even_row && is_even_col) { bl_index = 0u; }
        else if (is_even_row && !is_even_col) { bl_index = 1u; }
        else if (!is_even_row && is_even_col) { bl_index = 2u; }
        else { bl_index = 3u; }
    } else if (params.cfa_pattern == 1u) { // GRBG
        if (is_even_row && is_even_col) { bl_index = 1u; }
        else if (is_even_row && !is_even_col) { bl_index = 0u; }
        else if (!is_even_row && is_even_col) { bl_index = 3u; }
        else { bl_index = 2u; }
    } else if (params.cfa_pattern == 2u) { // GBRG
        if (is_even_row && is_even_col) { bl_index = 1u; }
        else if (is_even_row && !is_even_col) { bl_index = 3u; }
        else if (!is_even_row && is_even_col) { bl_index = 0u; }
        else { bl_index = 2u; }
    } else { // BGGR (3)
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

    // Determine pixel color for demosaicing
    var is_red = false;
    var is_green = false;
    var is_blue = false;

    if (params.cfa_pattern == 0u) { // RGGB
        if (is_even_row && is_even_col) { is_red = true; }
        else if (is_even_row && !is_even_col) { is_green = true; }
        else if (!is_even_row && is_even_col) { is_green = true; }
        else { is_blue = true; }
    } else if (params.cfa_pattern == 1u) { // GRBG
        if (is_even_row && is_even_col) { is_green = true; }
        else if (is_even_row && !is_even_col) { is_red = true; }
        else if (!is_even_row && is_even_col) { is_blue = true; }
        else { is_green = true; }
    } else if (params.cfa_pattern == 2u) { // GBRG
        if (is_even_row && is_even_col) { is_green = true; }
        else if (is_even_row && !is_even_col) { is_blue = true; }
        else if (!is_even_row && is_even_col) { is_red = true; }
        else { is_green = true; }
    } else { // BGGR (3)
        if (is_even_row && is_even_col) { is_blue = true; }
        else if (is_even_row && !is_even_col) { is_green = true; }
        else if (!is_even_row && is_even_col) { is_green = true; }
        else { is_red = true; }
    }

    // Phase 112: Edge-Aware Gradient Demosaicing
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

        let grad_v = abs(n - s);
        let grad_h = abs(e - w);
        var g: f32;
        if (grad_v < grad_h) { g = (n + s) * 0.5; }
        else if (grad_h < grad_v) { g = (e + w) * 0.5; }
        else { g = (n + s + e + w) * 0.25; }

        let grad_nesw = abs(ne - sw);
        let grad_nwse = abs(nw - se);
        var b: f32;
        if (grad_nesw < grad_nwse) { b = (ne + sw) * 0.5; }
        else if (grad_nwse < grad_nesw) { b = (nw + se) * 0.5; }
        else { b = (ne + sw + nw + se) * 0.25; }

        rgb = vec3<f32>(r, g, b);

    } else if (is_even_row && !is_even_col) {
        // Green Pixel (Red Row): red neighbours are E/W, blue neighbours are N/S
        let g = normalized;
        let r = (w + e) * 0.5;
        let b = (n + s) * 0.5;
        rgb = vec3<f32>(r, g, b);

    } else if (!is_even_row && is_even_col) {
        // Green Pixel (Blue Row): red neighbours are N/S, blue neighbours are E/W
        let g = normalized;
        let r = (n + s) * 0.5;
        let b = (w + e) * 0.5;
        rgb = vec3<f32>(r, g, b);

    } else {
        // Blue Pixel
        let b = normalized;

        let grad_v = abs(n - s);
        let grad_h = abs(e - w);
        var g: f32;
        if (grad_v < grad_h) { g = (n + s) * 0.5; }
        else if (grad_h < grad_v) { g = (e + w) * 0.5; }
        else { g = (n + s + e + w) * 0.25; }

        let grad_nesw = abs(ne - sw);
        let grad_nwse = abs(nw - se);
        var r: f32;
        if (grad_nesw < grad_nwse) { r = (ne + sw) * 0.5; }
        else if (grad_nwse < grad_nesw) { r = (nw + se) * 0.5; }
        else { r = (ne + sw + nw + se) * 0.25; }

        rgb = vec3<f32>(r, g, b);
    }

    return rgb;
}

// Helper to safely load a neighbour pixel with correct per-channel black-level correction.
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

    if (params.cfa_pattern == 0u) { // RGGB
        if (is_even_row && is_even_col) { bl_index = 0u; }
        else if (is_even_row && !is_even_col) { bl_index = 1u; }
        else if (!is_even_row && is_even_col) { bl_index = 2u; }
        else { bl_index = 3u; }
    } else if (params.cfa_pattern == 1u) { // GRBG
        if (is_even_row && is_even_col) { bl_index = 1u; }
        else if (is_even_row && !is_even_col) { bl_index = 0u; }
        else if (!is_even_row && is_even_col) { bl_index = 3u; }
        else { bl_index = 2u; }
    } else if (params.cfa_pattern == 2u) { // GBRG
        if (is_even_row && is_even_col) { bl_index = 1u; }
        else if (is_even_row && !is_even_col) { bl_index = 3u; }
        else if (!is_even_row && is_even_col) { bl_index = 0u; }
        else { bl_index = 2u; }
    } else { // BGGR (3)
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

// Debayer a neighbour pixel and convert it through white-balance and the
// colour matrix, yielding sRGB linear.  Used by noise-reduction and
// sharpening so those operations work in the same colour space as the
// centre pixel (Rec.709 luma coefficients are only valid in sRGB).
fn cam_to_srgb_linear(coords: vec2<i32>, dimensions: vec2<u32>, cm: mat3x3<f32>) -> vec3<f32> {
    let cam = debayer(coords, dimensions);
    return cm * (cam * params.wb_multipliers.rgb);
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

    // ── Step 1: Debayer → camera-native linear RGB ───────────────────────────
    var color = debayer(pixel_coords, dimensions);

    // ── STAGE 1: Sensor Clipping Detection (BEFORE WB / Exposure) ────────────
    // Measure destruction on the raw debayered values so WB gains and exposure
    // cannot amplify a clipped channel to look brighter than its neighbours.
    let pre_wb_max = max(color.r, max(color.g, color.b));
    let clip_blend = smoothstep(0.94, 0.98, pre_wb_max);

    // ── Step 2: White Balance ─────────────────────────────────────────────────
    color = color * params.wb_multipliers.rgb;

    // ── Step 3: Highlight Neutralisation (fixes magenta sky at -5 EV) ────────
    // Blend clipped pixels toward grey proportional to clip_blend.
    // We use max_c (the WB-scaled maximum) so the grey level tracks exposure.
    let max_c = max(color.r, max(color.g, color.b));
    color = mix(color, vec3<f32>(max_c), clip_blend);

    // ── Step 4: Exposure ──────────────────────────────────────────────────────
    let exposure_multiplier = pow(2.0, params.exposure);
    color = color * exposure_multiplier;

    // ── Step 5: Colour Matrix (Camera RGB → sRGB linear) ─────────────────────
    // Build once; neighbour helpers (NR, sharpening) reuse the same matrix.
    let color_matrix = transpose(mat3x3<f32>(
        params.color_matrix_0,
        params.color_matrix_1,
        params.color_matrix_2
    ));
    color = color_matrix * color;

    // ── Step 6: Clamp Negatives ───────────────────────────────────────────────
    // The camera→sRGB matrix contains negative cross-talk values. Any surviving
    // negative channel would poison luma coefficients and invert colours.
    // Must clamp BEFORE luma calculations or gamut compression.
    color = max(color, vec3<f32>(0.0));

    // ── Step 7: Chroma Noise Reduction (Rec.709 luma — valid in sRGB space) ──
    if (params.noise_reduction > 0.0) {
        let y = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
        let u = (color.b - y) * 0.565;
        let v = (color.r - y) * 0.713;

        let tl = cam_to_srgb_linear(pixel_coords + vec2<i32>(-1, -1), dimensions, color_matrix);
        let tr = cam_to_srgb_linear(pixel_coords + vec2<i32>( 1, -1), dimensions, color_matrix);
        let bl = cam_to_srgb_linear(pixel_coords + vec2<i32>(-1,  1), dimensions, color_matrix);
        let br = cam_to_srgb_linear(pixel_coords + vec2<i32>( 1,  1), dimensions, color_matrix);

        let tl_y = dot(tl, vec3<f32>(0.2126, 0.7152, 0.0722));
        let tl_u = (tl.b - tl_y) * 0.565; let tl_v = (tl.r - tl_y) * 0.713;
        let tr_y = dot(tr, vec3<f32>(0.2126, 0.7152, 0.0722));
        let tr_u = (tr.b - tr_y) * 0.565; let tr_v = (tr.r - tr_y) * 0.713;
        let bl_y = dot(bl, vec3<f32>(0.2126, 0.7152, 0.0722));
        let bl_u = (bl.b - bl_y) * 0.565; let bl_v = (bl.r - bl_y) * 0.713;
        let br_y = dot(br, vec3<f32>(0.2126, 0.7152, 0.0722));
        let br_u = (br.b - br_y) * 0.565; let br_v = (br.r - br_y) * 0.713;

        let avg_u = (tl_u + tr_u + bl_u + br_u) * 0.25;
        let avg_v = (tl_v + tr_v + bl_v + br_v) * 0.25;

        let den_u = mix(u, avg_u, params.noise_reduction);
        let den_v = mix(v, avg_v, params.noise_reduction);

        color.r = y + 1.403 * den_v;
        color.g = y - 0.344 * den_u - 0.714 * den_v;
        color.b = y + 1.770 * den_u;
    }

    // ── Step 8: Unsharp Mask Sharpening (in sRGB linear space) ───────────────
    if (params.sharpening > 0.0) {
        let s_up    = cam_to_srgb_linear(pixel_coords + vec2<i32>( 0, -1), dimensions, color_matrix);
        let s_down  = cam_to_srgb_linear(pixel_coords + vec2<i32>( 0,  1), dimensions, color_matrix);
        let s_left  = cam_to_srgb_linear(pixel_coords + vec2<i32>(-1,  0), dimensions, color_matrix);
        let s_right = cam_to_srgb_linear(pixel_coords + vec2<i32>( 1,  0), dimensions, color_matrix);

        let blur   = (s_up + s_down + s_left + s_right) * 0.25;
        let detail = color - blur;

        let detail_luma = length(detail);
        let mask_val = smoothstep(params.sharpen_masking, params.sharpen_masking + 0.05, detail_luma);

        color = color + (detail * params.sharpening * mask_val);
    }

    // ── STAGE 2: Gamut Compression (Path-to-White) ────────────────────────────
    // Smoothly desaturate over-bright valid colours toward luminance-matched
    // white. Luma is safe here because negatives were clamped in Step 6.
    let luma_gc = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    let overbright = max(0.0, max(color.r, max(color.g, color.b)) - 1.0);
    let path_to_white = smoothstep(0.0, 2.0, overbright);
    color = mix(color, vec3<f32>(luma_gc), path_to_white);

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

    // ── Step 15: Photographic Tone Curve (Extended Reinhard, Max-RGB) ────────
    // Replaces ACES: midtones stay linear and punchy, highlights roll cleanly
    // to pure white rather than lingering in a compressed grey.
    // white_point = 4.0 means a pixel 2 stops over-exposed maps to exactly 1.0.
    color = max(color, vec3<f32>(0.0));
    let pre_tone_max = max(color.r, max(color.g, color.b));
    if (pre_tone_max > 0.0) {
        let white_point = 4.0;
        let numerator   = pre_tone_max * (1.0 + (pre_tone_max / (white_point * white_point)));
        let denominator = 1.0 + pre_tone_max;
        let post_tone_max = clamp(numerator / denominator, 0.0, 1.0);
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

/// Get the shader source code for the current rendering mode
pub fn get_shader() -> &'static str {
    PASSTHROUGH_SHADER
}
