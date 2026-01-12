pub mod pipeline;
pub mod render_functions;
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
pub mod shared; // Phase 95: Unified pipeline architecture // Phase 95: Standalone rendering functions
