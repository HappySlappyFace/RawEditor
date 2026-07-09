/// GPU-accelerated RAW image rendering module.
///
/// Architecture:
/// - `params.rs`   — GpuEditParams struct (uniform buffer layout)
/// - `shaders.rs`  — WGSL shader source
/// - `shared.rs`   — Shared GPU context + ImageResources (the live render path)
/// - `render_functions.rs` — Standalone rendering functions (used by develop + export)
pub mod params;
pub mod render_functions;
pub mod shaders;
pub mod shared;
