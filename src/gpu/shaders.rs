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
    
    // Generate full-screen triangle from vertex index
    // Triangle covers entire screen: (-1,-1) to (3,3)
    let x = f32((vertex_index << 1u) & 2u);
    let y = f32(vertex_index & 2u);
    
    output.clip_position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    output.tex_coords = vec2<f32>(x, y);
    
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
}

@group(0) @binding(0)
var input_texture: texture_2d<f32>;

@group(0) @binding(1)
var texture_sampler: sampler;

@group(0) @binding(2)
var<uniform> params: EditParams;

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Sample the input texture
    var color = textureSample(input_texture, texture_sampler, input.tex_coords);
    
    // Apply exposure (additive in linear space)
    // Convert stops to multiplier: 2^exposure
    let exposure_multiplier = pow(2.0, params.exposure);
    color = vec4<f32>(color.rgb * exposure_multiplier, color.a);
    
    // Apply contrast (around midpoint 0.5)
    // Formula: (color - 0.5) * (1.0 + contrast/100.0) + 0.5
    let contrast_factor = 1.0 + (params.contrast / 100.0);
    color = vec4<f32>(
        (color.rgb - 0.5) * contrast_factor + 0.5,
        color.a
    );
    
    // Clamp to valid range
    color = clamp(color, vec4<f32>(0.0), vec4<f32>(1.0));
    
    return color;
}
"#;

/// Get the shader source code for the current rendering mode
pub fn get_shader() -> &'static str {
    PASSTHROUGH_SHADER
}
