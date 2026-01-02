/// Phase 95: Standalone rendering functions that work with SharedContext + ImageResources
/// These replace the old RenderPipeline methods

use super::shared::{SharedContext, ImageResources};
use iced_wgpu::wgpu;

/// Render to bytes at specified resolution
pub fn render_to_bytes(context: &SharedContext, resources: &ImageResources, width: u32, height: u32) -> Vec<u8> {
    // Create output texture
    let output_texture = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Output Texture"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
    
    // Render
    let mut encoder = context.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Render Encoder") });
    {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output_view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        render_pass.set_pipeline(&context.pipeline);
        render_pass.set_bind_group(0, &resources.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
    
    // Read back
    let bytes_per_row = (width * 4 + 255) & !255;
    let buffer_size = (bytes_per_row * height) as u64;
    let readback_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Readback Buffer"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture { texture: &output_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::ImageCopyBuffer { buffer: &readback_buffer, layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(height) } },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    context.queue.submit(Some(encoder.finish()));
    
    let buffer_slice = readback_buffer.slice(..);
    buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
    context.device.poll(wgpu::Maintain::Wait);
    
    let data = buffer_slice.get_mapped_range();
    let mut result = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let start = (y * bytes_per_row) as usize;
        let end = start + (width * 4) as usize;
        result.extend_from_slice(&data[start..end]);
    }
    drop(data);
    readback_buffer.unmap();
    
    result
}

/// Render to histogram resolution
pub fn render_to_histogram_bytes(context: &SharedContext, resources: &ImageResources) -> Vec<u8> {
    render_to_bytes(context, resources, resources.histogram_width, resources.histogram_height)
}

/// Calculate histogram from RGBA bytes
pub fn calculate_histogram(rgba_bytes: &[u8]) -> [[u32; 256]; 3] {
    let mut histogram = [[0u32; 256]; 3];
    for chunk in rgba_bytes.chunks_exact(4) {
        histogram[0][chunk[0] as usize] += 1;
        histogram[1][chunk[1] as usize] += 1;
        histogram[2][chunk[2] as usize] += 1;
    }
    histogram
}
