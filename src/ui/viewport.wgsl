// src/ui/viewport.wgsl
// Phase 115: Viewport shader.
// Renders the preview texture with pan and zoom support.

struct Uniforms {
    zoom:       f32,
    pan_x:      f32,
    pan_y:      f32,
    img_aspect: f32,
    vp_aspect:  f32,
    _pad0:      f32,
    _pad1:      f32,
    _pad2:      f32,
    bg:         vec4<f32>,
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
    out.clip_pos = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Convert from [0,1] UV to [-1,1] normalised screen space.
    var ndc = (in.uv - vec2<f32>(0.5)) * 2.0;

    // Undo viewport→image aspect ratio scaling.
    // An image pixel is square on screen only when:
    //   screen_x / vp_aspect == image_x / img_aspect
    let aspect_scale = uniforms.vp_aspect / uniforms.img_aspect;
    ndc.x = ndc.x * aspect_scale;

    // Apply zoom and pan. pan_x/y are in the same normalised screen units.
    let zoomed = ndc / uniforms.zoom - vec2<f32>(uniforms.pan_x * 2.0, uniforms.pan_y * 2.0);

    // Convert back to [0,1] UV.
    let final_uv = zoomed / 2.0 + vec2<f32>(0.5);

    // If the UV falls outside [0,1] after pan/zoom, show the background.
    if final_uv.x < 0.0 || final_uv.x > 1.0 || final_uv.y < 0.0 || final_uv.y > 1.0 {
        return uniforms.bg;
    }

    return textureSample(image_texture, image_sampler, final_uv);
}
