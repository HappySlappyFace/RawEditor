/// GPU-accelerated RAW image rendering module
///
/// This module provides real-time, non-destructive RAW image processing
/// using wgpu and custom WGSL shaders.
///
/// Architecture:
/// - `shaders.rs` - WGSL shader source code
/// - `pipeline.rs` - wgpu render pipeline management
///
/// The pipeline converts RAW sensor data (u16) to rendered RGB output,
/// applying edit parameters in real-time on the GPU.

pub mod shaders;
pub mod pipeline;

pub use pipeline::RenderPipeline;
