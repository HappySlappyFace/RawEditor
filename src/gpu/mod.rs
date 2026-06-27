/// GPU-accelerated RAW image rendering module.
///
/// Architecture:
/// - `params.rs`   — GpuEditParams struct (uniform buffer layout)
/// - `pipeline.rs` — RenderPipeline struct + uniform update methods
/// - `init.rs`     — RenderPipeline::new (device/texture/bind group setup)
/// - `render.rs`   — RenderPipeline render methods and histogram
/// - `shaders.rs`  — WGSL shader source
/// - `shared.rs`   — Shared GPU context
/// - `render_functions.rs` — Standalone rendering functions
pub mod params;
pub mod pipeline;
pub mod init;
pub mod render;
pub mod render_functions;
pub mod shaders;
pub mod shared;
