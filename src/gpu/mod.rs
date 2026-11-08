/// GPU-accelerated RAW image rendering module.
///
/// Architecture:
/// - `demosaic.rs` — Pass 1: RCD demosaic (four fragment passes)
/// - `denoise.rs`  — cached noise-reduction pre-pass (NLM luma + à-trous chroma)
/// - `params.rs`   — GpuEditParams struct (uniform buffer layout)
/// - `shaders.rs`  — WGSL shader source
/// - `shared.rs`   — Shared GPU context + ImageResources (the live render path)
/// - `render_functions.rs` — Standalone rendering functions (used by develop + export)
/// - `loupe.rs`    — the 1:1 detail loupe: a full-resolution crop through the same passes
pub mod ai_mask;
pub mod brush;
pub mod demosaic;
pub mod denoise;
pub mod local_contrast;
pub mod loupe;
pub mod params;
pub mod render_functions;
pub mod shaders;
pub mod shared;
#[cfg(test)]
mod eval_tests;
