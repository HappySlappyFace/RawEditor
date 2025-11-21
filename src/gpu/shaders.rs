/// WGSL shader code for real-time RAW image processing
///
/// This shader applies non-destructive edits to RAW sensor data in real-time.
/// Phase 10: Simple passthrough with exposure and contrast
/// Phase 11+: Full debayering, color science, and advanced adjustments

/// Passthrough shader for RAW image rendering
/// 
/// This is a simple shader that:
/// 1. Samples the input texture (RAW data as RGB for now)
/// 2. Applies exposure adjustment (additive)
/// 3. Applies contrast adjustment (multiplicative)
/// 4. Returns the final color
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
    // Base tex coords (0-1)
    var tex_x = (x + 1.0) * 0.5;
    var tex_y = (y + 1.0) * 0.5;
    
    // Center coordinates around (0.5, 0.5)
    tex_x -= 0.5;
    tex_y -= 0.5;
    
    // Apply zoom (divide by zoom to zoom in)
    tex_x /= params.zoom;
    tex_y /= params.zoom;
    
    // Apply pan offset
    tex_x -= params.pan_x;
    tex_y -= params.pan_y;
    
    // Move back to (0,0) origin
    tex_x += 0.5;
    tex_y += 0.5;
    
    output.tex_coords = vec2<f32>(tex_x, tex_y);
    
    return output;
}

// ========== Fragment Shader ==========

// Uniform buffer for edit parameters
struct EditParams {
    exposure: f32,        // -5.0 to +5.0 stops
    contrast: f32,        // -100.0 to +100.0
    highlights: f32,      // -100.0 to +100.0
    shadows: f32,         // -100.0 to +100.0
    whites: f32,          // -100.0 to +100.0
    blacks: f32,          // -100.0 to +100.0
    vibrance: f32,        // -100.0 to +100.0
    saturation: f32,      // -100.0 to +100.0
    temperature: f32,     // -100 to +100 (converted from i32)
    tint: f32,            // -100 to +100 (converted from i32)
    padding1: f32,        // Padding for 16-byte alignment
    padding2: f32,        // Padding for 16-byte alignment
    // Phase 14: Color science metadata
    wb_multipliers: vec4<f32>,  // White balance [R, G, B, G2]
    color_matrix_0: vec3<f32>,  // Color matrix row 0
    padding3: f32,               // Padding after vec3
    color_matrix_1: vec3<f32>,  // Color matrix row 1
    padding4: f32,               // Padding after vec3
    color_matrix_2: vec3<f32>,  // Color matrix row 2
    padding5: f32,               // Padding after vec3
    // Phase 25: Zoom & Pan
    zoom: f32,                   // Zoom level (1.0 = 100%)
    pan_x: f32,                  // Pan offset X
    pan_y: f32,                  // Pan offset Y
    // Phase 34: CFA Pattern
    cfa_pattern: u32,            // Offset 128
    
    // Padding to align black_levels to 16 bytes
    // Padding to align black_levels to 16 bytes
    pad_cfa_1: f32,              // 128
    pad_cfa_2: f32,              // 132
    pad_cfa_3: f32,              // 136
    pad_cfa_4: f32,              // 140
    
    // Phase 36: Per-channel Black Levels (vec4 alignment = 16 bytes)
    black_levels: vec4<u32>,     // Offset 144
    
    white_level: u32,            // Offset 160
    
    // Padding to reach 176 bytes
    pad_end_1: f32,              // 164
    pad_end_2: f32,              // 168
    pad_end_3: f32,              // 172
    
    // Phase 38: Manual Black Level Offsets
    black_offsets: vec4<f32>,    // Offset 176
    
    // Phase 39: Black Level Phase Correction
    black_phase_x: u32,          // Offset 192
    black_phase_y: u32,          // Offset 196
    
    // Phase 49: Noise Reduction
    noise_reduction: f32,        // Offset 200
    
    // Phase 50: Sharpening
    sharpening: f32,             // Offset 204
    
    // Padding to reach 224 bytes
    pad_phase_1: f32,            // 208
    pad_phase_2: f32,            // 212
    pad_phase_3: f32,            // 216
    pad_phase_4: f32,            // 220
    // Total: 224 bytes
}

@group(0) @binding(0)
var input_texture: texture_2d<u32>;  // RAW u16 data stored as u32

@group(0) @binding(1)
var texture_sampler: sampler;  // Not used for integer textures, but kept for compatibility

@group(0) @binding(2)
var<uniform> params: EditParams;

// Simple nearest-neighbor debayering with CFA pattern support
// CFA patterns: 0=RGGB, 1=GRBG, 2=GBRG, 3=BGGR
fn debayer(coords: vec2<i32>, dimensions: vec2<u32>) -> vec3<f32> {
    // Load RAW pixel value (12-bit in u16, stored as u32)
    let raw_value = textureLoad(input_texture, coords, 0).r;
    
    // CRITICAL: Black level must be indexed by CFA COLOR, not spatial position!
    // black_levels array is [R, G1, G2, B] indexed by color channel
    // We need to determine which color THIS pixel is, then use that index
    
    let x = u32(coords.x);
    let y = u32(coords.y);
    let is_even_row = (y & 1u) == 0u;
    let is_even_col = (x & 1u) == 0u;
    
    // Determine CFA color index for this pixel based on pattern
    // black_levels[0] = R, black_levels[1] = G (red row), black_levels[2] = G (blue row), black_levels[3] = B
    var bl_index: u32;
    
    if (params.cfa_pattern == 0u) { // RGGB
        if (is_even_row && is_even_col) { bl_index = 0u; }       // R
        else if (is_even_row && !is_even_col) { bl_index = 1u; } // G1 (red row)
        else if (!is_even_row && is_even_col) { bl_index = 2u; } // G2 (blue row)  
        else { bl_index = 3u; }                                   // B
    } else if (params.cfa_pattern == 1u) { // GRBG
        if (is_even_row && is_even_col) { bl_index = 1u; }       // G1
        else if (is_even_row && !is_even_col) { bl_index = 0u; } // R
        else if (!is_even_row && is_even_col) { bl_index = 3u; } // B
        else { bl_index = 2u; }                                   // G2
    } else if (params.cfa_pattern == 2u) { // GBRG
        if (is_even_row && is_even_col) { bl_index = 1u; }       // G1
        else if (is_even_row && !is_even_col) { bl_index = 3u; } // B
        else if (!is_even_row && is_even_col) { bl_index = 0u; } // R
        else { bl_index = 2u; }                                   // G2
    } else { // BGGR (3)
        if (is_even_row && is_even_col) { bl_index = 3u; }       // B
        else if (is_even_row && !is_even_col) { bl_index = 1u; } // G1
        else if (!is_even_row && is_even_col) { bl_index = 2u; } // G2
        else { bl_index = 0u; }                                   // R
    }
    
    // Apply black level correction BEFORE normalization
    // This is the critical step: subtract black per-CFA-color, then clamp to 0
    let black = f32(params.black_levels[bl_index]) + params.black_offsets[bl_index];
    let white = f32(params.white_level);
    let corrected = max(0.0, f32(raw_value) - black);
    
    // Normalize to [0,1] range
    let range = max(1.0, white - black);
    let normalized = corrected / range;
    
    // Determine what color this pixel is for demosaicing
    // (Reusing the CFA pattern logic from above)
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
    
    // Phase 47: Bilinear Interpolation Demosaicing
    // For each CFA color, properly interpolate missing channels using 4 neighbors
    var rgb: vec3<f32>;
    
    if (is_red) {
        // Red pixel: R is native, interpolate G and B
        let r = normalized;
        
        // Green: Average of 4 orthogonal neighbors (↑↓←→)
        let g = (
            get_neighbor(coords + vec2<i32>(0, -1), dimensions) +  // up
            get_neighbor(coords + vec2<i32>(0, 1), dimensions) +   // down
            get_neighbor(coords + vec2<i32>(-1, 0), dimensions) +  // left
            get_neighbor(coords + vec2<i32>(1, 0), dimensions)     // right
        ) * 0.25;
        
        // Blue: Average of 4 diagonal neighbors (↖↗↙↘)
        let b = (
            get_neighbor(coords + vec2<i32>(-1, -1), dimensions) + // top-left
            get_neighbor(coords + vec2<i32>(1, -1), dimensions) +  // top-right
            get_neighbor(coords + vec2<i32>(-1, 1), dimensions) +  // bottom-left
            get_neighbor(coords + vec2<i32>(1, 1), dimensions)     // bottom-right
        ) * 0.25;
        
        rgb = vec3<f32>(r, g, b);
        
    } else if (is_even_row && !is_even_col) {
        // Green Pixel (Red Row): G is native, R left/right, B top/bottom
        let g = normalized;
        
        // Red: Average of left and right neighbors
        let r = (
            get_neighbor(coords + vec2<i32>(-1, 0), dimensions) +  // left
            get_neighbor(coords + vec2<i32>(1, 0), dimensions)     // right
        ) * 0.5;
        
        // Blue: Average of top and bottom neighbors
        let b = (
            get_neighbor(coords + vec2<i32>(0, -1), dimensions) +  // top
            get_neighbor(coords + vec2<i32>(0, 1), dimensions)     // bottom
        ) * 0.5;
        
        rgb = vec3<f32>(r, g, b);
        
    } else if (!is_even_row && is_even_col) {
        // Green Pixel (Blue Row): G is native, B left/right, R top/bottom
        let g = normalized;
        
        // Red: Average of top and bottom neighbors
        let r = (
            get_neighbor(coords + vec2<i32>(0, -1), dimensions) +  // top
            get_neighbor(coords + vec2<i32>(0, 1), dimensions)     // bottom
        ) * 0.5;
        
        // Blue: Average of left and right neighbors
        let b = (
            get_neighbor(coords + vec2<i32>(-1, 0), dimensions) +  // left
            get_neighbor(coords + vec2<i32>(1, 0), dimensions)     // right
        ) * 0.5;
        
        rgb = vec3<f32>(r, g, b);
        
    } else {
        // Blue pixel: B is native, interpolate G and R
        let b = normalized;
        
        // Green: Average of 4 orthogonal neighbors (↑↓←→)
        let g = (
            get_neighbor(coords + vec2<i32>(0, -1), dimensions) +  // up
            get_neighbor(coords + vec2<i32>(0, 1), dimensions) +   // down
            get_neighbor(coords + vec2<i32>(-1, 0), dimensions) +  // left
            get_neighbor(coords + vec2<i32>(1, 0), dimensions)     // right
        ) * 0.25;
        
        // Red: Average of 4 diagonal neighbors (↖↗↙↘)
        let r = (
            get_neighbor(coords + vec2<i32>(-1, -1), dimensions) + // top-left
            get_neighbor(coords + vec2<i32>(1, -1), dimensions) +  // top-right
            get_neighbor(coords + vec2<i32>(-1, 1), dimensions) +  // bottom-left
            get_neighbor(coords + vec2<i32>(1, 1), dimensions)     // bottom-right
        ) * 0.25;
        
        rgb = vec3<f32>(r, g, b);
    }
    
    return rgb;
}

// Helper to safely load neighbor pixel WITH CORRECT BLACK LEVEL CORRECTION
// CRITICAL: This must match the processing in debayer() for the current pixel!
fn get_neighbor(coords: vec2<i32>, dimensions: vec2<u32>) -> f32 {
    // Clamp to texture bounds
    let clamped = vec2<i32>(
        clamp(coords.x, 0, i32(dimensions.x) - 1),
        clamp(coords.y, 0, i32(dimensions.y) - 1)
    );
    
    let raw_value = textureLoad(input_texture, clamped, 0).r;
    
    // Apply same black level correction as debayer()
    let x = u32(clamped.x);
    let y = u32(clamped.y);
    let is_even_row = (y & 1u) == 0u;
    let is_even_col = (x & 1u) == 0u;
    
    // Determine CFA color index for this neighbor pixel
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
    
    // Apply black level correction and normalization (same as debayer())
    let black = f32(params.black_levels[bl_index]) + params.black_offsets[bl_index];
    let white = f32(params.white_level);
    let corrected = max(0.0, f32(raw_value) - black);
    let range = max(1.0, white - black);
    let normalized = corrected / range;
    
    return normalized;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Phase 25: Discard fragments outside texture bounds (when zoomed out)
    if input.tex_coords.x < 0.0 || input.tex_coords.x > 1.0 ||
       input.tex_coords.y < 0.0 || input.tex_coords.y > 1.0 {
        // Return black for areas outside the image
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    
    // Get texture dimensions
    let dimensions = textureDimensions(input_texture);
    
    // Convert normalized texture coordinates to pixel coordinates
    let pixel_coords = vec2<i32>(
        i32(input.tex_coords.x * f32(dimensions.x)),
        i32(input.tex_coords.y * f32(dimensions.y))
    );
    
    // Phase 14: Color Science Pipeline (in correct order!)
    
    // 1. Debayer to get RAW RGB color (still in linear camera space)
    var color = debayer(pixel_coords, dimensions);
    
    // Phase 49: Chroma Noise Reduction (if enabled)
    if (params.noise_reduction > 0.0) {
        // RGB to YUV conversion (ITU-R BT.601)
        // Y = luminance (detail), U and V = chrominance (color)
        let y = 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
        let u = (color.b - y) * 0.565;
        let v = (color.r - y) * 0.713;
        
        // Sample 4 diagonal neighbors and convert to YUV
        let tl = debayer(pixel_coords + vec2<i32>(-1, -1), dimensions);
        let tr = debayer(pixel_coords + vec2<i32>(1, -1), dimensions);
        let bl = debayer(pixel_coords + vec2<i32>(-1, 1), dimensions);
        let br = debayer(pixel_coords + vec2<i32>(1, 1), dimensions);
        
        // Convert neighbors to YUV
        let tl_y = 0.299 * tl.r + 0.587 * tl.g + 0.114 * tl.b;
        let tl_u = (tl.b - tl_y) * 0.565;
        let tl_v = (tl.r - tl_y) * 0.713;
        
        let tr_y = 0.299 * tr.r + 0.587 * tr.g + 0.114 * tr.b;
        let tr_u = (tr.b - tr_y) * 0.565;
        let tr_v = (tr.r - tr_y) * 0.713;
        
        let bl_y = 0.299 * bl.r + 0.587 * bl.g + 0.114 * bl.b;
        let bl_u = (bl.b - bl_y) * 0.565;
        let bl_v = (bl.r - bl_y) * 0.713;
        
        let br_y = 0.299 * br.r + 0.587 * br.g + 0.114 * br.b;
        let br_u = (br.b - br_y) * 0.565;
        let br_v = (br.r - br_y) * 0.713;
        
        // Average the UV channels of neighbors
        let avg_u = (tl_u + tr_u + bl_u + br_u) * 0.25;
        let avg_v = (tl_v + tr_v + bl_v + br_v) * 0.25;
        
        // Mix original UV with averaged UV based on strength
        let denoised_u = mix(u, avg_u, params.noise_reduction);
        let denoised_v = mix(v, avg_v, params.noise_reduction);
        
        // Convert YUV back to RGB (keep original Y for sharpness!)
        color.r = y + 1.403 * denoised_v;
        color.g = y - 0.344 * denoised_u - 0.714 * denoised_v;
        color.b = y + 1.770 * denoised_u;
    }
    
    // Phase 50: Unsharp Mask Sharpening (if enabled)
    if (params.sharpening > 0.0) {
        // Sample 4 orthogonal neighbors for blur calculation
        let up = debayer(pixel_coords + vec2<i32>(0, -1), dimensions);
        let down = debayer(pixel_coords + vec2<i32>(0, 1), dimensions);
        let left = debayer(pixel_coords + vec2<i32>(-1, 0), dimensions);
        let right = debayer(pixel_coords + vec2<i32>(1, 0), dimensions);
        
        // Calculate blur (local average)
        let blur = (up + down + left + right) * 0.25;
        
        // Extract high-frequency detail
        let detail = color - blur;
        
        // Apply sharpening: add detail back based on strength
        color = color + (detail * params.sharpening);
    }
    
    // 2. Apply White Balance (normalize sensor response)
    color = color * params.wb_multipliers.rgb;
    
    // 2.5. Apply Manual White Balance (Phase 18: Temperature & Tint)
    // Temperature: Blue/Yellow axis (cooler/warmer)
    // Scale by 0.3 for noticeable but not extreme adjustments
    color.r = color.r * (1.0 + params.temperature * 0.3);  // More yellow when positive
    color.b = color.b * (1.0 - params.temperature * 0.3);  // Less blue when positive
    
    // Tint: Green/Magenta axis
    // Positive = more green, Negative = more magenta (less green)
    color.g = color.g * (1.0 + params.tint * 0.3);
    
    // 3. Apply Color Matrix (camera RGB → sRGB color space)
    // Reconstruct 3x3 matrix from padded vec3 rows
    let color_matrix = mat3x3<f32>(
        params.color_matrix_0,
        params.color_matrix_1,
        params.color_matrix_2
    );
    color = color_matrix * color;
    
    // 4. Apply Exposure (still in linear space)
    let exposure_multiplier = pow(2.0, params.exposure);
    color = color * exposure_multiplier;
    
    // 5. Apply Highlights & Shadows (Phase 17: Smart Tone - Luminance-weighted adjustments)
    // Calculate luminance to determine which pixels are bright vs dark
    let lum_for_tone = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    
    // Highlights: Affects bright pixels more (lum=1.0 gets full effect, lum=0.0 gets none)
    // Negative values recover blown highlights, positive values boost them
    color = color * (1.0 + (lum_for_tone * params.highlights));
    
    // Shadows: Affects dark pixels more (lum=0.0 gets full effect, lum=1.0 gets none)
    // Positive values lift shadows, negative values crush them
    color = color * (1.0 + ((1.0 - lum_for_tone) * params.shadows));
    
    // 6. Apply Contrast (around midpoint 0.5)
    let contrast_factor = 1.0 + (params.contrast / 100.0);
    color = (color - 0.5) * contrast_factor + 0.5;
    
    // 7. Apply Levels (Phase 16: Whites & Blacks tone control)
    // Standard levels formula: (color - black_point) / (white_point - black_point)
    // This controls the dynamic range by remapping black and white points
    color = (color - vec3<f32>(params.blacks)) / (vec3<f32>(params.whites - params.blacks + 0.0001));
    
    // 8. Apply Saturation (Phase 15 color boost)
    // Calculate luminance using Rec. 709 coefficients
    var luma = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    // Saturation factor: -100 = grayscale, 0 = original, +100 = 2x saturation
    let sat_factor = 1.0 + (params.saturation / 100.0);
    // Mix between grayscale and original color
    color = mix(vec3<f32>(luma), color, sat_factor);
    
    // 9. Apply Vibrance (Phase 27: Smart saturation protecting skin tones)
    // Calculate pixel's saturation (max(r,g,b) - min(r,g,b))
    let sat = max(color.r, max(color.g, color.b)) - min(color.r, min(color.g, color.b));
    // Calculate vibrance amount, weighted by (1.0 - saturation)
    // This applies *less* vibrance to *more* saturated pixels (protects skin tones)
    let vibrance_amount = params.vibrance * (1.0 - sat);
    // Apply vibrance (mix from grayscale)
    luma = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    color = mix(vec3<f32>(luma), color, 1.0 + vibrance_amount);
    
    // 10. Apply sRGB Gamma Correction (linear → sRGB for display)
    // This is critical for proper brightness perception!
    color = pow(color, vec3<f32>(1.0 / 2.2));
    
    // 11. Clamp to valid range
    color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    
    return vec4<f32>(color, 1.0);
}
"#;

/// Get the shader source code for the current rendering mode
pub fn get_shader() -> &'static str {
    PASSTHROUGH_SHADER
}
