// src/ui/viewport.wgsl
// Phase 115: Viewport shader.
// Renders the preview texture with pan and zoom support.

// The projection matrix carries the ENTIRE on-screen placement of the image
// (letterbox fit, zoom, pan) — it maps the fixed [-1,1] quad to exactly the
// rectangle `core::viewport::image_rect` computed on the Rust side. Zoom and
// pan are baked into this matrix (see ViewportPrimitive::prepare), not
// reapplied here, so the quad geometry itself grows/shrinks/shifts and the
// fragment shader is a plain texture sample. Letterbox background comes from
// the parent iced container behind this shader widget, not from here.
struct Uniforms {
    proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var image_texture: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv:       vec2<f32>,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0)       uv:       vec2<f32>,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = uniforms.proj * vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(image_texture, image_sampler, in.uv);
}
