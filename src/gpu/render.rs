use iced_wgpu::wgpu;

use super::pipeline::RenderPipeline;

impl RenderPipeline {
    /// Render directly to an iced-provided texture view (Canvas integration).
    pub fn render_to_target(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: (u32, u32),
    ) {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("RAW Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        render_pass.set_viewport(0.0, 0.0, viewport.0 as f32, viewport.1 as f32, 0.0, 1.0);
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }

    /// Render to preview resolution for fast updates (GPU downsamples automatically).
    pub fn render_to_bytes(&self, width: u32, height: u32) -> Vec<u8> {
        let output_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Output Texture (Preview)"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        self.render_to_target(&mut encoder, &output_view, (width, height));

        let bytes_per_row = width * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        let buffer_size = (padded_bytes_per_row * height) as u64;

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Output Buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(Some(encoder.finish()));

        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::error!("GPU buffer map failed: {:?}", e);
                return Vec::new();
            }
            Err(e) => {
                tracing::error!("GPU buffer map channel error: {:?}", e);
                return Vec::new();
            }
        }

        let data = buffer_slice.get_mapped_range();
        let mut output = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let start = (y * padded_bytes_per_row) as usize;
            let end = start + (width * 4) as usize;
            output.extend_from_slice(&data[start..end]);
        }

        drop(data);
        output_buffer.unmap();
        output
    }

    /// Render to full resolution for export. Slow — only call for final export.
    pub fn render_full_res_to_bytes(
        &self,
        output_format: wgpu::TextureFormat,
        crop: [f32; 4],
    ) -> Result<Vec<u8>, String> {
        let crop_w = crop[2];
        let crop_h = crop[3];
        let target_width = (self.width as f32 * crop_w) as u32;
        let target_height = (self.height as f32 * crop_h) as u32;

        tracing::info!(
            "Exporting crop: {}x{} (Original: {}x{})",
            target_width,
            target_height,
            self.width,
            self.height
        );

        let output_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Output Texture (Full Resolution)"),
            size: wgpu::Extent3d {
                width: target_width,
                height: target_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: output_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder (Full Res)"),
            });

        let mut export_params = if let Ok(current) = self.current_params.lock() {
            *current
        } else {
            return Err("Failed to lock params".to_string());
        };
        export_params.crop = crop;
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[export_params]),
        );

        self.render_to_target(&mut encoder, &output_view, (target_width, target_height));

        // Restore pre-export uniforms so subsequent preview renders are unaffected
        if let Ok(current) = self.current_params.lock() {
            self.queue.write_buffer(
                &self.uniform_buffer,
                0,
                bytemuck::cast_slice(&[*current]),
            );
        }

        let bytes_per_row = target_width * 4;
        let padded_bytes_per_row = (bytes_per_row + 255) & !255;
        let buffer_size = (padded_bytes_per_row * target_height) as u64;

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Output Buffer (Full Res)"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(target_height),
                },
            },
            wgpu::Extent3d {
                width: target_width,
                height: target_height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(Some(encoder.finish()));

        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);

        match rx.recv() {
            Ok(Ok(())) => {
                let data = buffer_slice.get_mapped_range();
                let mut output =
                    Vec::with_capacity((target_width * target_height * 4) as usize);
                for y in 0..target_height {
                    let start = (y * padded_bytes_per_row) as usize;
                    let end = start + (target_width * 4) as usize;
                    if end <= data.len() {
                        output.extend_from_slice(&data[start..end]);
                    } else {
                        tracing::warn!("Export buffer underrun at row {}", y);
                    }
                }
                drop(data);
                output_buffer.unmap();
                Ok(output)
            }
            Ok(Err(e)) => {
                tracing::error!("GPU Readback failed: {:?}", e);
                Err(format!("GPU Readback failed: {:?}", e))
            }
            Err(e) => {
                tracing::error!("GPU Readback channel error: {:?}", e);
                Err(format!("GPU Readback channel error: {:?}", e))
            }
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Render to tiny histogram-sized texture (~128px wide) for fast histogram calculation.
    pub fn render_to_histogram_bytes(&self) -> Vec<u8> {
        let output_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Histogram Output Texture"),
            size: wgpu::Extent3d {
                width: self.histogram_width,
                height: self.histogram_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Histogram Render Encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Histogram Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }

        let bytes_per_pixel = 4;
        let unpadded_bytes_per_row = self.histogram_width * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;
        let buffer_size = (padded_bytes_per_row * self.histogram_height) as u64;

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Histogram Output Buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(self.histogram_height),
                },
            },
            wgpu::Extent3d {
                width: self.histogram_width,
                height: self.histogram_height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(Some(encoder.finish()));

        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::error!("Histogram GPU buffer map failed: {:?}", e);
                return Vec::new();
            }
            Err(e) => {
                tracing::error!("Histogram GPU buffer map channel error: {:?}", e);
                return Vec::new();
            }
        }

        let data = buffer_slice.get_mapped_range();
        let mut output =
            Vec::with_capacity((self.histogram_width * self.histogram_height * 4) as usize);
        for row in 0..self.histogram_height {
            let start = (row * padded_bytes_per_row) as usize;
            let end = start + unpadded_bytes_per_row as usize;
            output.extend_from_slice(&data[start..end]);
        }

        drop(data);
        output_buffer.unmap();
        output
    }

    /// Calculate RGB histogram from rendered RGBA bytes. Returns [R[256], G[256], B[256]].
    pub fn calculate_histogram(&self, rgba_bytes: &[u8]) -> [[u32; 256]; 3] {
        let mut histograms = [[0u32; 256]; 3];
        for pixel in rgba_bytes.chunks_exact(4) {
            histograms[0][pixel[0] as usize] += 1;
            histograms[1][pixel[1] as usize] += 1;
            histograms[2][pixel[2] as usize] += 1;
        }
        histograms
    }
}
