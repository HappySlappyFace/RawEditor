//! AI mask layers: each AI mask's segmentation bitmap (`ml::masks`) as one
//! layer of an `R8Unorm` array, the colour pass's group 1 binding 2.
//!
//! Layer `i` is mask slot `i`, in the same oriented display frame and at the
//! same capped size as the brush layers (`brush::layer_size`), so the shader
//! samples both at the same UV. Bitmaps are stored in the sensor frame and
//! rotated on upload; uploads happen in `ImageResources::prepare_stages`, and
//! only when an id, the orientation or the set of decoded bitmaps changed.
//! An image without AI masks never allocates anything but the 1×1
//! placeholder.

use std::collections::HashMap;
use std::sync::Arc;

use iced_wgpu::wgpu;

use crate::core::types::MAX_MASKS;
use crate::ml::masks::Bitmap;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// Decoded bitmaps by `ai_id`, handed to the resources by the app.
pub type AiBitmaps = HashMap<u64, Arc<Bitmap>>;

/// What the layers currently hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiKey {
    pub ids: [u64; MAX_MASKS],
    pub orientation: u32,
    /// Bumped by every `set_ai_bitmaps`: a bitmap decoded after the ids were
    /// first seen must still reach the GPU.
    pub generation: u64,
}

#[derive(Debug)]
pub struct AiLayers {
    texture: wgpu::Texture,
    pub array_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

fn texture(device: &wgpu::Device, label: &str, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: MAX_MASKS as u32 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn array_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

/// A 1×1 array of zeros standing in until an AI mask exists.
pub fn placeholder(device: &wgpu::Device) -> wgpu::TextureView {
    array_view(&texture(device, "AI mask layers (empty)", 1, 1))
}

impl AiLayers {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = texture(device, "AI mask layers", width, height);
        let array_view = array_view(&texture);
        Self { texture, array_view, width, height }
    }

    /// Write slot `slot`: the bitmap rotated to `orientation` and resampled
    /// to the layer size, or zeros.
    pub fn upload(&self, queue: &wgpu::Queue, slot: usize, bitmap: Option<&Bitmap>, orientation: u32) {
        let data = match bitmap {
            Some(b) => b.to_display(orientation).resized(self.width, self.height).data,
            None => vec![0; (self.width * self.height) as usize],
        };
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: slot as u32 },
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(self.width), rows_per_image: Some(self.height) },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }
}
