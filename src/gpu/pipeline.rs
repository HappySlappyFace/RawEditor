use crate::core::types::EditParams;
use iced_wgpu::wgpu;

pub use super::params::GpuEditParams;

/// Main render pipeline for RAW image processing.
pub struct RenderPipeline {
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) uniform_buffer: wgpu::Buffer,
    pub(super) texture: wgpu::Texture,
    pub(super) texture_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub preview_width: u32,
    pub preview_height: u32,
    pub image_id: i64,
    pub histogram_width: u32,
    pub histogram_height: u32,
    pub wb_multipliers: [f32; 4],
    pub forward_matrix: [f32; 9],
    pub(super) has_dcp: bool,
    pub(super) cfa_pattern: u32,
    pub(super) black_levels: [u32; 4],
    pub(super) white_level: u32,
    pub(super) current_params: std::sync::Mutex<GpuEditParams>,
}

impl std::fmt::Debug for RenderPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderPipeline")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl RenderPipeline {
    pub fn update_uniforms(&self, params: &EditParams) {
        self.update_uniforms_with_zoom(params, 1.0, 0.0, 0.0);
    }

    pub fn update_uniforms_with_zoom(
        &self,
        params: &EditParams,
        zoom: f32,
        pan_x: f32,
        pan_y: f32,
    ) {
        let mut gpu_params = GpuEditParams::from(params);
        gpu_params.wb_multipliers = self.wb_multipliers;
        let cm = &self.forward_matrix;
        gpu_params.forward_matrix_0 = [cm[0], cm[1], cm[2]];
        gpu_params.forward_matrix_1 = [cm[3], cm[4], cm[5]];
        gpu_params.forward_matrix_2 = [cm[6], cm[7], cm[8]];
        gpu_params.zoom = zoom;
        gpu_params.pan_x = pan_x;
        gpu_params.pan_y = pan_y;
        gpu_params.cfa_pattern = self.cfa_pattern;
        gpu_params.black_levels = self.black_levels;
        gpu_params.white_level = self.white_level;
        gpu_params.crop = params.crop;

        crate::debug_log!(crate::debug::DEBUG_GPU, "🎨 GPU Uniforms Updated:");
        crate::debug_log!(
            crate::debug::DEBUG_GPU,
            "   Exposure: {:.2}, Contrast: {:.0}",
            gpu_params.exposure,
            gpu_params.contrast
        );
        crate::debug_log!(
            crate::debug::DEBUG_GPU,
            "   Highlights: {:.0}, Shadows: {:.0}",
            gpu_params.highlights,
            gpu_params.shadows
        );
        crate::debug_log!(
            crate::debug::DEBUG_GPU,
            "   Temp: {}, Tint: {}",
            gpu_params.temperature,
            gpu_params.tint
        );
        crate::debug_log!(
            crate::debug::DEBUG_GPU,
            "   Zoom: {:.1}%, Pan: ({:.3}, {:.3})",
            zoom * 100.0,
            pan_x,
            pan_y
        );
        crate::debug_log!(crate::debug::DEBUG_GPU, "   Crop: {:?}", gpu_params.crop);

        if let Ok(mut current) = self.current_params.lock() {
            *current = gpu_params;
        }

        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[gpu_params]));
    }
}
