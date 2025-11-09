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
    output.tex_coords = vec2<f32>((x + 1.0) * 0.5, (y + 1.0) * 0.5);
    
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
}

@group(0) @binding(0)
var input_texture: texture_2d<u32>;  // RAW u16 data stored as u32

@group(0) @binding(1)
var texture_sampler: sampler;  // Not used for integer textures, but kept for compatibility

@group(0) @binding(2)
var<uniform> params: EditParams;

// Simple nearest-neighbor debayering
// Assumes RGGB Bayer pattern (most common)
fn debayer(coords: vec2<i32>, dimensions: vec2<u32>) -> vec3<f32> {
    // Load RAW pixel value (12-bit in u16, stored as u32)
    let raw_value = textureLoad(input_texture, coords, 0).r;
    
    // Convert to normalized float (0.0 - 1.0)
    // 12-bit max = 4096
    let normalized = f32(raw_value) / 4096.0;
    
    // Determine position in Bayer pattern
    // Some cameras start at (1,1) instead of (0,0) - uncomment next 2 lines if colors are still wrong:
    let x = coords.x;  // Try uncommenting this
    let y = coords.y + 1;  // Try uncommenting this
    let is_even_row = (y % 2) == 0;
    let is_even_col = (x % 2) == 0;
    
    var rgb: vec3<f32>;
    
    // GBRG pattern (Green-Blue-Red-Green):
    // G B G B ...
    // R G R G ...
    // G B G B ...
    // Alternative Nikon pattern (trying this one)
    
    if is_even_row {
        if is_even_col {
            // Green pixel (blue row) - sample blue right, red below
            let g = normalized;
            let b = get_neighbor(coords + vec2<i32>(1, 0), dimensions);  // Blue to the right
            let r = get_neighbor(coords + vec2<i32>(0, 1), dimensions);  // Red below
            rgb = vec3<f32>(r, g, b);
        } else {
            // Blue pixel - sample green from neighbors, red from diagonal
            let b = normalized;
            let g = get_neighbor(coords - vec2<i32>(1, 0), dimensions);  // Green to the left
            let r = get_neighbor(coords + vec2<i32>(-1, 1), dimensions);  // Red diagonal
            rgb = vec3<f32>(r, g, b);
        }
    } else {
        if is_even_col {
            // Red pixel - sample green from neighbors, blue from diagonal
            let r = normalized;
            let g = get_neighbor(coords + vec2<i32>(1, 0), dimensions); // Green to the right
            let b = get_neighbor(coords + vec2<i32>(0, -1), dimensions);  // Blue above
            rgb = vec3<f32>(r, g, b);
        } else {
            // Green pixel (red row) - sample red left, blue above
            let g = normalized;
            let r = get_neighbor(coords - vec2<i32>(1, 0), dimensions);  // Red to the left
            let b = get_neighbor(coords + vec2<i32>(0, -1), dimensions);  // Blue above
            rgb = vec3<f32>(r, g, b);
        }
    }
    
    return rgb;
}

// Helper to safely load neighbor pixel
fn get_neighbor(coords: vec2<i32>, dimensions: vec2<u32>) -> f32 {
    // Clamp to texture bounds
    let clamped = vec2<i32>(
        clamp(coords.x, 0, i32(dimensions.x) - 1),
        clamp(coords.y, 0, i32(dimensions.y) - 1)
    );
    let raw_value = textureLoad(input_texture, clamped, 0).r;
    return f32(raw_value) / 4096.0;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
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
    
    // 2. Apply White Balance (normalize sensor response)
    color = color * params.wb_multipliers.rgb;
    
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
    let luminance = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    // Saturation factor: -100 = grayscale, 0 = original, +100 = 2x saturation
    let sat_factor = 1.0 + (params.saturation / 100.0);
    // Mix between grayscale and original color
    color = mix(vec3<f32>(luminance), color, sat_factor);
    
    // 9. Apply sRGB Gamma Correction (linear → sRGB for display)
    // This is critical for proper brightness perception!
    color = pow(color, vec3<f32>(1.0 / 2.2));
    
    // 10. Clamp to valid range
    color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    
    return vec4<f32>(color, 1.0);
}
"#;

/// Get the shader source code for the current rendering mode
pub fn get_shader() -> &'static str {
    PASSTHROUGH_SHADER
}
