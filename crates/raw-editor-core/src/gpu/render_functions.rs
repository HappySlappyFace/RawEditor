/// Phase 95: Standalone rendering functions that work with SharedContext + ImageResources
/// These replace the old RenderPipeline methods
use super::shared::{ImageResources, SharedContext};
use super::timing::FrameTimer;
use crate::core::profiler::{Count, Detail, Span, Timer};
use crate::core::output_space::OutputSpace;

/// Patch just the `view_rect` field of the already-uploaded uniform block.
///
/// A targeted 16-byte write rather than re-uploading all 784 bytes, and — more
/// importantly — it lets two passes in the same frame see different windows:
/// `queue.write_buffer` is ordered against submissions on the same queue, so
/// write/submit/write/submit gives each pass its own value without a second
/// uniform buffer or dynamic offsets.
///
/// The offset is asserted in `gpu::params`'s tests; if `view_rect` ever moves,
/// this would silently corrupt whatever now sits there.
fn write_view_rect(context: &SharedContext, resources: &ImageResources, view_rect: [f32; 4]) {
    const VIEW_RECT_OFFSET: u64 =
        std::mem::offset_of!(super::params::GpuEditParams, view_rect) as u64;
    context.queue.write_buffer(
        &resources.uniform_buffer,
        VIEW_RECT_OFFSET,
        bytemuck::cast_slice(&view_rect),
    );
}

/// What the user sees when `map_async` fails (device lost, out of memory):
/// not "wrong data size" from an encoder fed an empty buffer.
const READBACK_FAILED: &str = "the GPU readback failed (the graphics device may have been reset)";

/// Select the colour space the next renders encode into (export only; see
/// `core::output_space`). A 4-byte targeted write like `write_view_rect`:
/// `update_uniforms` always writes 0 (sRGB), so the display is unaffected.
pub fn write_output_space(context: &SharedContext, resources: &ImageResources, space: OutputSpace) {
    const OFFSET: u64 = std::mem::offset_of!(super::params::GpuEditParams, output_space) as u64;
    context.queue.write_buffer(&resources.uniform_buffer, OFFSET, bytemuck::bytes_of(&space.shader_id()));
}

/// Render to bytes at specified resolution
/// Render to bytes at specified resolution (Async + Instrumented)
/// Render the color pass and read the result back to CPU memory.
///
/// Returns `Arc<[u8]>` rather than `Vec<u8>` because every caller immediately
/// needs shared ownership: the develop path hands the same buffer to the
/// viewport shader and the histogram task. Doing the conversion here keeps it
/// to one place — the caller used to build the `Arc` *and* retain the original
/// `Vec` inside an unused `image::Handle`, holding two full-size copies of a
/// buffer that can reach tens of megabytes at zoom.
/// A pending GPU→CPU transfer: the staging buffer plus what it takes to
/// un-pad it back into tightly-packed RGBA.
///
/// The buffer comes from the context's pool and goes back to it when this is
/// dropped (`take_readback`).
struct Readback<'c> {
    pool: &'c std::sync::Mutex<Vec<wgpu::Buffer>>,
    /// `Some` until dropped. May be larger than `len` (a pooled buffer).
    buffer: Option<wgpu::Buffer>,
    /// The bytes this readback uses: `bytes_per_row * height`.
    len: u64,
    /// Read and unmapped, so the buffer can be reused. A readback whose map
    /// failed or was never awaited is not: its buffer may still be mapping.
    done: bool,
    bytes_per_row: u32,
    width: u32,
    height: u32,
}

impl Readback<'_> {
    /// The part of the buffer the copy writes, to map and read.
    fn slice(&self) -> wgpu::BufferSlice<'_> {
        self.buffer.as_ref().expect("a live readback has its buffer").slice(..self.len)
    }

    /// Copy out of the mapped range, dropping wgpu's 256-byte row padding.
    /// Only valid after the buffer has actually mapped.
    fn unpad(mut self) -> Vec<u8> {
        let data = self.slice().get_mapped_range();
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height {
            let start = (y * self.bytes_per_row) as usize;
            let end = start + (self.width * 4) as usize;
            out.extend_from_slice(&data[start..end]);
        }
        drop(data);
        self.finish();
        out
    }

    /// [`Self::unpad`] straight into shared memory. `Arc::from(Vec)` copies
    /// (the `Arc`'s counts sit in front of the data), and at a develop
    /// display's size that second copy cost as much as the readback itself:
    /// 3.6 ms of a 14.5 ms frame on a phone's fastest core.
    fn unpad_shared(mut self) -> std::sync::Arc<[u8]> {
        let row = (self.width * 4) as usize;
        let mut out = std::sync::Arc::<[u8]>::new_uninit_slice(row * self.height as usize);
        if row > 0 {
            let dst = std::sync::Arc::get_mut(&mut out).expect("a new Arc is unshared");
            let data = self.slice().get_mapped_range();
            for (y, dst_row) in dst.chunks_exact_mut(row).enumerate() {
                let start = y * self.bytes_per_row as usize;
                dst_row.write_copy_of_slice(&data[start..start + row]);
            }
        }
        self.finish();
        // SAFETY: the rows tile the slice exactly (`height` rows of `row`
        // bytes), so every byte has been written.
        unsafe { out.assume_init() }
    }

    /// Unmap, so the buffer can go back to the pool.
    fn finish(&mut self) {
        if let Some(buffer) = &self.buffer {
            buffer.unmap();
        }
        self.done = true;
    }
}

/// Readback buffers kept for reuse, and the largest kept.
///
/// A `MAP_READ` buffer lives in host-visible memory, and allocating one costs
/// milliseconds of CPU once it is a few MB: 7.8 ms for a 1349×900 display
/// render and 12 ms for 1950×1125 on a GTX 1650, against 0.02 ms for the 384px
/// overview. The display used to allocate one per slider tick, which was most
/// of the frame at any zoom whose render target is large (from fit up to about
/// 100%, where the target shrinks to the source pixels on screen). Pooling
/// makes it a one-off. Export frames (96 MB at 24 MP) are over the cap, so
/// they are freed as before rather than held.
const READBACK_POOL: usize = 3;
const READBACK_POOL_MAX_BYTES: u64 = 64 << 20;

impl Drop for Readback<'_> {
    fn drop(&mut self) {
        let Some(buffer) = self.buffer.take() else { return };
        if !self.done || buffer.size() > READBACK_POOL_MAX_BYTES {
            return;
        }
        let mut pool = self.pool.lock().unwrap_or_else(|e| e.into_inner());
        pool.push(buffer);
        if pool.len() > READBACK_POOL {
            pool.remove(0);
        }
    }
}

/// A readback buffer of at least `len` bytes: a pooled one that fits without
/// being more than twice the size, else a new one. New ones are rounded up to
/// a whole MiB, so the small changes in target size while zooming or resizing
/// still reuse them.
fn take_readback(context: &SharedContext, len: u64, detail: &mut Detail) -> wgpu::Buffer {
    {
        let mut pool = context.readbacks.lock().unwrap_or_else(|e| e.into_inner());
        let best = (0..pool.len())
            .filter(|&i| (len..=len.saturating_mul(2).max(1 << 20)).contains(&pool[i].size()))
            .min_by_key(|&i| pool[i].size());
        if let Some(i) = best {
            detail.bump(Count::ReadbackReused);
            return pool.remove(i);
        }
    }
    detail.bump(Count::ReadbackAllocated);
    context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Readback Buffer"),
        size: len.next_multiple_of(1 << 20),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

/// Record one color pass into its own texture and copy it to a staging buffer.
///
/// Returns the finished command buffer rather than submitting, so callers can
/// interleave uniform writes between passes: `write/submit/write/submit` gives
/// each pass a different `view_rect` off one uniform buffer.
///
/// `detail` takes the profiler's breakdown (`profiling` builds), and `marks`,
/// when given, the GPU timestamp marks written before the pass and after its
/// copy (`gpu::timing`).
fn encode_color_pass<'c>(
    context: &'c SharedContext,
    resources: &ImageResources,
    width: u32,
    height: u32,
    detail: &mut Detail,
    marks: Option<(&FrameTimer, u32)>,
) -> (wgpu::CommandBuffer, Readback<'c>) {
    let bindings = resources.prepare_stages(context, detail);
    encode_color_pass_with(context, &bindings, width, height, detail, marks)
}

/// [`encode_color_pass`] with the bind groups supplied — the detail loupe's
/// group 0 reads its own crop.
fn encode_color_pass_with<'c>(
    context: &'c SharedContext,
    bindings: &super::shared::ColorBindings,
    width: u32,
    height: u32,
    detail: &mut Detail,
    marks: Option<(&FrameTimer, u32)>,
) -> (wgpu::CommandBuffer, Readback<'c>) {
    let t = Timer::start();
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
    detail.stop(Span::Allocate, t);

    let t = Timer::start();
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });
    if let Some((gpu, first)) = marks {
        gpu.mark(&mut encoder, first);
    }
    {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Render Pass"),
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
        render_pass.set_pipeline(&context.pipeline);
        render_pass.set_bind_group(0, &bindings.main, &[]);
        render_pass.set_bind_group(1, &bindings.masks, &[]);
        render_pass.set_bind_group(2, &bindings.presence, &[]);
        render_pass.draw(0..3, 0..1);
    }

    detail.stop(Span::Record, t);

    let t = Timer::start();
    let bytes_per_row = (width * 4 + 255) & !255;
    let len = bytes_per_row as u64 * height as u64;
    let readback_buffer = take_readback(context, len, detail);
    detail.stop(Span::Allocate, t);

    let t = Timer::start();
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &output_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &readback_buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    if let Some((gpu, first)) = marks {
        gpu.mark(&mut encoder, first + 1);
    }
    let commands = encoder.finish();
    detail.stop(Span::Record, t);

    (
        commands,
        Readback { pool: &context.readbacks, buffer: Some(readback_buffer), len, done: false, bytes_per_row, width, height },
    )
}

/// Render with explicit bindings (whose group 0 carries its own uniform
/// buffer) and read the result back. `None` if the readback failed to map.
pub(crate) async fn render_with_bindings(
    context: &SharedContext,
    bindings: &super::shared::ColorBindings,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    let (cb, readback) = encode_color_pass_with(context, bindings, width, height, &mut Detail::default(), None);
    context.queue.submit(Some(cb));
    let (tx, rx) = tokio::sync::oneshot::channel();
    readback.slice().map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });
    tokio::task::block_in_place(|| {
        context.device.poll(wgpu::Maintain::Wait);
    });
    matches!(rx.await, Ok(Ok(()))).then(|| readback.unpad())
}

/// Like [`render_to_bytes`] but hands back the `Vec` directly.
///
/// Export writes the buffer straight into an encoder and never shares it, so
/// the `Arc::from` conversion below would be a pure extra full-frame copy —
/// 96 MB for a 24 MP frame, live at the same moment as the mapped readback
/// buffer and `unpad`'s Vec. Skipping it takes the readback peak from three
/// copies to two.
pub async fn render_to_vec(
    context: &SharedContext,
    resources: &ImageResources,
    width: u32,
    height: u32,
    view_rect: [f32; 4],
) -> Result<(Vec<u8>, f32, f32), String> {
    let t_upload_start = std::time::Instant::now();
    write_view_rect(context, resources, view_rect);
    let (cb, readback) = encode_color_pass(context, resources, width, height, &mut Detail::default(), None);
    let upload_ms = t_upload_start.elapsed().as_secs_f32() * 1000.0;

    let t_render_start = std::time::Instant::now();
    context.queue.submit(Some(cb));

    let (tx, rx) = tokio::sync::oneshot::channel();
    readback.slice().map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });

    tokio::task::block_in_place(|| {
        context.device.poll(wgpu::Maintain::Wait);
    });

    if let Ok(Ok(())) = rx.await {
        let result = readback.unpad();
        let render_ms = t_render_start.elapsed().as_secs_f32() * 1000.0;
        Ok((result, upload_ms, render_ms))
    } else {
        Err(READBACK_FAILED.into())
    }
}

/// Test-only now: the app renders through `render_display_and_overview`
/// (display) and `render_to_vec` (export).
#[cfg(test)]
pub async fn render_to_bytes(
    context: &SharedContext,
    resources: &ImageResources,
    width: u32,
    height: u32,
    view_rect: [f32; 4],
) -> (std::sync::Arc<[u8]>, f32, f32) {
    let t_upload_start = std::time::Instant::now();
    write_view_rect(context, resources, view_rect);
    let (cb, readback) = encode_color_pass(context, resources, width, height, &mut Detail::default(), None);
    let upload_ms = t_upload_start.elapsed().as_secs_f32() * 1000.0;

    let t_render_start = std::time::Instant::now();
    context.queue.submit(Some(cb));

    let (tx, rx) = tokio::sync::oneshot::channel();
    readback.slice().map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });

    // wgpu's map_async callback only fires when the device is polled.
    // block_in_place runs the blocking poll(Wait) on the current thread without
    // blocking the entire tokio executor — correct pattern for wgpu inside async.
    tokio::task::block_in_place(|| {
        context.device.poll(wgpu::Maintain::Wait);
    });

    if let Ok(Ok(())) = rx.await {
        let result = readback.unpad_shared();
        let render_ms = t_render_start.elapsed().as_secs_f32() * 1000.0;
        (result, upload_ms, render_ms)
    } else {
        (std::sync::Arc::from(Vec::new()), upload_ms, 0.0)
    }
}

/// Everything one develop render produces.
pub struct DisplayRender {
    /// The on-screen window: `width`×`height` tightly packed RGBA.
    pub display: std::sync::Arc<[u8]>,
    /// The whole image, small (`overview_dims` RGBA). Feeds the histogram, and
    /// doubles as the image's edited proxy — the filmstrip thumbnail and the
    /// placeholder shown while the image reloads.
    pub overview: std::sync::Arc<[u8]>,
    pub histogram: crate::core::histogram::HistogramData,
    pub encode_ms: f32,
    pub gpu_ms: f32,
    /// The profiler's breakdown (`profiling` builds; empty otherwise).
    pub detail: Detail,
}

/// Render the display window AND a small whole-image "overview" pass, under
/// a single `poll(Wait)`.
///
/// The overview must stay whole-image: the display pass only covers what is
/// on screen, so a histogram derived from it would drift as the user pans and
/// zooms. A dedicated small pass keeps those semantics and makes the CPU scan
/// trivial — it used to walk the entire display buffer, which at 100% zoom
/// meant ~24M pixels single-threaded per render. Being a faithful small render
/// of the current edit, its pixels are kept too (see `DisplayRender::overview`)
/// rather than generating edited thumbnails with a pass of their own.
///
/// The two passes may read different resource sets of the same image:
/// `overview_resources` is the subsampled develop preview when one is kept
/// beside a full-resolution upgrade — a 384px pass gains nothing from 24 MP,
/// and its cached stages (denoise) would otherwise re-run at full resolution
/// on every slider tick. When both are the same set they share one uniform
/// buffer, so their differing `view_rect` values are sequenced by
/// `queue.write_buffer`'s ordering against submits.
///
/// `detail` comes back filled in `DisplayRender::detail` (`profiling` builds).
#[allow(clippy::too_many_arguments)]
pub async fn render_display_and_overview(
    context: &SharedContext,
    resources: &ImageResources,
    overview_resources: &ImageResources,
    width: u32,
    height: u32,
    view_rect: [f32; 4],
    overview_dims: (u32, u32),
    mut detail: Detail,
) -> DisplayRender {
    // Timing is split so the profiler's buckets mean what they say:
    //   `encode_ms` — CPU-side resource creation and command recording
    //   `gpu_ms`    — every submit plus the readback wait
    // The two passes have to interleave (each uniform write applies to the
    // submit that follows it), so the submits can't simply be hoisted out of
    // the encode window — the elapsed time is accumulated instead.
    //
    // Everything up to the wait is in this block: the GPU timer holds a lock
    // in `profiling` builds, which must not be held across an await.
    let (overview_rb, orx, main_rb, mrx, encode_ms, mut gpu_ms, t_render_start) = {
        let gpu = context.timer.frame();
        gpu.mark_now(context, 0);
        let t_encode = std::time::Instant::now();
        write_view_rect(context, overview_resources, super::params::FULL_VIEW_RECT);
        let (overview_cb, overview_rb) = encode_color_pass(
            context, overview_resources, overview_dims.0, overview_dims.1, &mut detail, Some((&gpu, 1)),
        );
        let mut encode_ms = t_encode.elapsed().as_secs_f32() * 1000.0;

        // Counted as GPU time: `submit` can block on queue backpressure, and
        // attributing that stall to "upload" made the HUD read as though texture
        // allocation had suddenly become expensive after a zoom-in.
        let t_submit_a = std::time::Instant::now();
        let t = Timer::start();
        context.queue.submit(Some(overview_cb));
        detail.stop(Span::Submit, t);
        write_view_rect(context, resources, view_rect);
        gpu.mark_now(context, 3);
        let gpu_ms = t_submit_a.elapsed().as_secs_f32() * 1000.0;

        let t_encode_b = std::time::Instant::now();
        let (main_cb, main_rb) = encode_color_pass(context, resources, width, height, &mut detail, Some((&gpu, 4)));
        encode_ms += t_encode_b.elapsed().as_secs_f32() * 1000.0;

        let t_render_start = std::time::Instant::now();
        let t = Timer::start();
        context.queue.submit(Some(main_cb));
        detail.stop(Span::Submit, t);
        gpu.resolve(context);

        let (otx, orx) = tokio::sync::oneshot::channel();
        overview_rb.slice().map_async(wgpu::MapMode::Read, move |r| {
            let _ = otx.send(r);
        });
        let (mtx, mrx) = tokio::sync::oneshot::channel();
        main_rb.slice().map_async(wgpu::MapMode::Read, move |r| {
            let _ = mtx.send(r);
        });

        // One wait covers both submissions.
        let t = Timer::start();
        tokio::task::block_in_place(|| {
            context.device.poll(wgpu::Maintain::Wait);
        });
        detail.stop(Span::Wait, t);
        gpu.read(&mut detail);
        (overview_rb, orx, main_rb, mrx, encode_ms, gpu_ms, t_render_start)
    };

    let (overview, histogram) = if let Ok(Ok(())) = orx.await {
        let t = Timer::start();
        let pixels = overview_rb.unpad_shared();
        detail.stop(Span::Unpad, t);
        let t = Timer::start();
        let histogram = crate::core::histogram::calculate(&pixels);
        detail.stop(Span::Histogram, t);
        (pixels, histogram)
    } else {
        (std::sync::Arc::from(Vec::new()), [[0u32; 256]; 3])
    };

    let display = if let Ok(Ok(())) = mrx.await {
        let t = Timer::start();
        let pixels = main_rb.unpad_shared();
        detail.stop(Span::Unpad, t);
        pixels
    } else {
        std::sync::Arc::from(Vec::new())
    };
    gpu_ms += t_render_start.elapsed().as_secs_f32() * 1000.0;

    DisplayRender { display, overview, histogram, encode_ms, gpu_ms, detail }
}

/// A texture the display pass draws into directly — a window's or a
/// phone's surface — instead of rendering into its own and reading it back.
pub struct DisplayTarget<'a> {
    /// The surface texture's view. Its format must be one the colour pass
    /// draws into (`shared::is_display_format`; `shared::pick_surface_format`
    /// chooses one).
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    /// The view's size in pixels.
    pub size: (u32, u32),
    /// The visible slice of the image and where it goes on the target
    /// (`core::viewport::screen_placement`); `None` draws only the background.
    pub placement: Option<([f32; 4], crate::core::viewport::Rect)>,
    /// What shows around the image.
    pub background: wgpu::Color,
}

/// What [`render_to_target`] leaves behind: the CPU's share of the frame,
/// and the overview, if this frame asked for one, still on the GPU.
pub struct SurfaceRender {
    pub overview: Option<PendingOverview>,
    pub encode_ms: f32,
    /// The profiler's CPU breakdown (`profiling` builds; empty otherwise).
    /// GPU pass times are not measured here: reading them back would wait
    /// for the GPU, which is what this path avoids.
    pub detail: Detail,
}

/// The overview (histogram, edited proxy) a surface frame rendered, on its
/// way back from the GPU. Collect it a frame or so later with
/// [`PendingOverview::try_read`], which never waits; dropping it abandons it.
/// Read once: after `try_read` has returned `Some`, it is spent.
pub struct PendingOverview {
    /// `Some` until read.
    buffer: Option<wgpu::Buffer>,
    len: u64,
    bytes_per_row: u32,
    width: u32,
    height: u32,
    mapped: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

/// A read-back overview: the pixels (RGBA) and their histogram.
pub struct Overview {
    pub pixels: std::sync::Arc<[u8]>,
    pub histogram: crate::core::histogram::HistogramData,
}

impl PendingOverview {
    /// Ask for `readback`'s buffer to be mapped, once the work copying into
    /// it (already submitted) is done.
    fn start(mut readback: Readback<'_>) -> Self {
        let buffer = readback.buffer.take().expect("a live readback has its buffer");
        let (tx, mapped) = std::sync::mpsc::channel();
        buffer.slice(..readback.len).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        Self {
            buffer: Some(buffer),
            len: readback.len,
            bytes_per_row: readback.bytes_per_row,
            width: readback.width,
            height: readback.height,
            mapped,
        }
    }

    /// The overview if the GPU has finished it, without waiting; `None`
    /// while it hasn't, to try again later.
    pub fn try_read(&mut self, context: &SharedContext) -> Option<Result<Overview, String>> {
        context.device.poll(wgpu::Maintain::Poll);
        match self.mapped.try_recv() {
            Ok(mapped) => Some(self.read(context, mapped)),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("the overview's readback was dropped".into())),
        }
    }

    /// Wait for the overview: for tests, probes, and a front end leaving.
    pub fn wait(mut self, context: &SharedContext) -> Result<Overview, String> {
        context.device.poll(wgpu::Maintain::Wait);
        match self.mapped.recv() {
            Ok(mapped) => self.read(context, mapped),
            Err(_) => Err("the overview's readback was dropped".into()),
        }
    }

    fn read(&mut self, context: &SharedContext, mapped: Result<(), wgpu::BufferAsyncError>) -> Result<Overview, String> {
        mapped.map_err(|e| format!("reading the overview back: {e}"))?;
        if self.buffer.is_none() {
            return Err("the overview was already read".into());
        }
        // Back into a pooled readback, which unpads it and returns the
        // buffer to the pool.
        let readback = Readback {
            pool: &context.readbacks,
            buffer: self.buffer.take(),
            len: self.len,
            done: false,
            bytes_per_row: self.bytes_per_row,
            width: self.width,
            height: self.height,
        };
        let pixels = readback.unpad_shared();
        let histogram = crate::core::histogram::calculate(&pixels);
        Ok(Overview { pixels, histogram })
    }
}

/// [`render_display_and_overview`] for a front end that draws straight to
/// the screen: the display pass goes into `target` (no readback), and
/// nothing waits for the GPU. `present` runs once the work is submitted.
///
/// The overview (whole image, `overview_dims`, from `overview_resources`)
/// is rendered only when asked for, and comes back as a
/// [`PendingOverview`] for a later frame to collect: the histogram and the
/// edited proxy run a frame behind, which nobody can see, and the frame
/// costs the CPU only its own work. A front end keeps one in flight at a
/// time (`None` while one is).
#[allow(clippy::too_many_arguments)]
pub fn render_to_target(
    context: &SharedContext,
    resources: &ImageResources,
    overview_resources: Option<&ImageResources>,
    target: DisplayTarget<'_>,
    overview_dims: (u32, u32),
    mut detail: Detail,
    present: impl FnOnce(),
) -> Result<SurfaceRender, String> {
    let pipeline = context.display_pipeline(target.format)?;
    let t_encode = std::time::Instant::now();
    let overview = overview_resources.map(|overview_resources| {
        write_view_rect(context, overview_resources, super::params::FULL_VIEW_RECT);
        let (overview_cb, overview_rb) = encode_color_pass(
            context,
            overview_resources,
            overview_dims.0,
            overview_dims.1,
            &mut detail,
            None,
        );
        let t = Timer::start();
        context.queue.submit(Some(overview_cb));
        detail.stop(Span::Submit, t);
        overview_rb
    });
    if let Some((view_rect, _)) = target.placement {
        write_view_rect(context, resources, view_rect);
    }

    let bindings = resources.prepare_stages(context, &mut detail);
    let t = Timer::start();
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Display Encoder") });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Display Pass (surface)"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(target.background), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        if let Some((_, rect)) = target.placement {
            // Inside the target, as wgpu requires; `screen_placement`
            // already clips, this only guards rounding.
            let (w, h) = (target.size.0 as f32, target.size.1 as f32);
            let x = rect.x.clamp(0.0, w);
            let y = rect.y.clamp(0.0, h);
            let rw = rect.width.min(w - x);
            let rh = rect.height.min(h - y);
            if rw > 0.0 && rh > 0.0 {
                pass.set_viewport(x, y, rw, rh, 0.0, 1.0);
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bindings.main, &[]);
                pass.set_bind_group(1, &bindings.masks, &[]);
                pass.set_bind_group(2, &bindings.presence, &[]);
                pass.draw(0..3, 0..1);
            }
        }
    }
    let commands = encoder.finish();
    detail.stop(Span::Record, t);
    let encode_ms = t_encode.elapsed().as_secs_f32() * 1000.0;

    let t = Timer::start();
    context.queue.submit(Some(commands));
    detail.stop(Span::Submit, t);
    present();
    // Mapped once everything submitted so far is done, the overview's copy
    // included; nothing here waits for that.
    let overview = overview.map(PendingOverview::start);
    Ok(SurfaceRender { overview, encode_ms, detail })
}

/// Render to opaque RGB `u16` samples, for 16-bit TIFF export. Mirrors
/// `render_to_bytes` but targets `Rgba16Float` via `context.pipeline_16` —
/// a wgpu RenderPipeline's output format is baked in at creation time, so
/// this can't reuse the 8-bit pipeline/texture with a different format.
///
/// `Rgba16Float` (unlike `Rgba8Unorm`) doesn't clamp writes at the GPU's
/// fixed-function blend stage, so the conversion below clamps to [0,1]
/// before scaling to u16 — functionally the same clipping the 8-bit path
/// already applies in hardware, just performed here in software. 16-bit
/// buys smoother gradation in mid/shadow tones, not extra highlight
/// headroom. Alpha is dropped (always opaque for a flattened photo render),
/// matching the existing JPEG export path's RGB-only precedent.
pub async fn render_to_bytes_16bit(
    context: &SharedContext,
    resources: &ImageResources,
    width: u32,
    height: u32,
) -> Result<(Vec<u16>, f32, f32), String> {
    let t_upload_start = std::time::Instant::now();

    // Export renders the whole cropped frame. This must be reset explicitly:
    // the uniform buffer is shared, so a display render at zoom leaves a
    // windowed `view_rect` behind that would otherwise silently export only
    // the region that happened to be on screen.
    write_view_rect(context, resources, super::params::FULL_VIEW_RECT);
    let bindings = resources.prepare_stages(context, &mut Detail::default());

    let output_texture = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Output Texture 16-bit"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let upload_ms = t_upload_start.elapsed().as_secs_f32() * 1000.0;
    let t_render_start = std::time::Instant::now();

    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder 16-bit"),
        });
    {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Render Pass 16-bit"),
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
        render_pass.set_pipeline(&context.pipeline_16);
        render_pass.set_bind_group(0, &bindings.main, &[]);
        render_pass.set_bind_group(1, &bindings.masks, &[]);
        render_pass.set_bind_group(2, &bindings.presence, &[]);
        render_pass.draw(0..3, 0..1);
    }

    // 8 bytes/pixel (4 channels x f16) instead of 4 for the 8-bit path.
    let bytes_per_row = (width * 8 + 255) & !255;
    let buffer_size = (bytes_per_row * height) as u64;
    let readback_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Readback Buffer 16-bit"),
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
            buffer: &readback_buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    context.queue.submit(Some(encoder.finish()));

    let buffer_slice = readback_buffer.slice(..);
    let (tx, rx) = tokio::sync::oneshot::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });

    tokio::task::block_in_place(|| {
        context.device.poll(wgpu::Maintain::Wait);
    });

    if let Ok(Ok(())) = rx.await {
        let data = buffer_slice.get_mapped_range();
        // RGB only (alpha dropped) — 3 u16 samples per pixel.
        let mut result = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            let start = (y * bytes_per_row) as usize;
            let end = start + (width * 8) as usize;
            let row_f16: &[half::f16] = bytemuck::cast_slice(&data[start..end]);
            for pixel in row_f16.as_chunks::<4>().0 {
                for &channel in &pixel[..3] {
                    let v = (channel.to_f32().clamp(0.0, 1.0) * 65535.0).round() as u16;
                    result.push(v);
                }
            }
        }
        drop(data);
        readback_buffer.unmap();

        let render_ms = t_render_start.elapsed().as_secs_f32() * 1000.0;
        Ok((result, upload_ms, render_ms))
    } else {
        Err(READBACK_FAILED.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::EditParams;
    use crate::gpu::params::FULL_VIEW_RECT;
    use crate::gpu::shared::ImageResources;

    /// A frame with real 2-channel highlight clipping (measured: ~2900 blown
    /// Bayer blocks, 97.5% of them still carrying usable data). Override with
    /// `RAWEDITOR_TEST_RAW` to point at your own file.
    fn test_raw_path() -> String {
        std::env::var("RAWEDITOR_TEST_RAW").unwrap_or_else(|_| {
            "/home/hsf001/Pictures/Photography/TESTFiles/RAW/_DSC0177.NEF".to_string()
        })
    }

    /// Coefficient of variation (stddev / mean) of luma over `mask`.
    ///
    /// The metric that distinguishes real highlight recovery from merely
    /// darkening: a uniform multiply leaves CV unchanged, because both stddev
    /// and mean scale together. CV only rises if gradation that was compressed
    /// into a flat band gets redistributed — i.e. if detail actually returns.
    fn luma_cv(rgba: &[u8], mask: &[bool]) -> f64 {
        let vals: Vec<f64> = rgba
            .as_chunks::<4>().0.iter()
            .zip(mask)
            .filter(|(_, m)| **m)
            .map(|(p, _)| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64)
            .collect();
        if vals.is_empty() {
            return 0.0;
        }
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        if mean <= 0.0 {
            return 0.0;
        }
        let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / vals.len() as f64;
        (var.sqrt()) / mean
    }

    fn mean_luma(rgba: &[u8], mask: &[bool]) -> f64 {
        let vals: Vec<f64> = rgba
            .as_chunks::<4>().0.iter()
            .zip(mask)
            .filter(|(_, m)| **m)
            .map(|(p, _)| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64)
            .collect();
        if vals.is_empty() { return 0.0; }
        vals.iter().sum::<f64>() / vals.len() as f64
    }

    /// Render one frame at the given highlights setting.
    async fn render_at(
        ctx: &SharedContext,
        res: &ImageResources,
        raw: &crate::raw::loader::RawDataResult,
        highlights: f32,
        w: u32,
        h: u32,
    ) -> std::sync::Arc<[u8]> {
        let mut params = EditParams::default();
        params.highlights = highlights;
        // Mirror the develop path's WB/DCP resolution so the render matches
        // what the app actually produces.
        let as_shot = crate::color::as_shot_kelvin_tint(raw);
        let (kelvin, _tint, wb_override) =
            crate::color::solve_wb(&params, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
        let interpolated = raw.dcp_profile.as_ref().map(|d| {
            crate::raw::dcp::interpolate_at_temperature(d, kelvin, crate::core::tonemap::ToneSettings::from_params(&params))
        });
        res.update_uniforms(ctx, &params, interpolated.as_ref(), wb_override);
        let (bytes, _, _) = render_to_bytes(ctx, res, w, h, FULL_VIEW_RECT).await;
        bytes
    }

    /// Highlight recovery must return DETAIL, not just darken.
    ///
    /// Ignored by default: needs a GPU and the test RAW on disk.
    /// Run with `cargo test --release highlight_recovery -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn highlight_recovery_restores_detail_not_just_brightness() {
        let test_raw = test_raw_path();
        if !std::path::Path::new(&test_raw).exists() {
            eprintln!("test RAW missing, skipping");
            return;
        }
        // Multi-thread flavour is required: render_to_bytes uses
        // tokio::task::block_in_place for the wgpu poll.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .expect("runtime");

        rt.block_on(async {
            let ctx = SharedContext::new().await.expect("gpu context");
            let raw = crate::raw::loader::load_raw_data(test_raw.clone())
                .await
                .expect("decode raw");

            let base = EditParams::default();
            let as_shot = crate::color::as_shot_kelvin_tint(&raw);
            let (kelvin, _t, _wb) =
                crate::color::solve_wb(&base, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
            let (forward_matrix, dcp) = match &raw.dcp_profile {
                Some(d) => {
                    let i = crate::raw::dcp::interpolate_at_temperature(d, kelvin, crate::core::tonemap::ToneSettings::from_params(&base));
                    (i.camera_to_prophoto, Some(i))
                }
                None => (
                    crate::color::calculate_cam_to_srgb(
                        raw.color_matrix,
                        raw.wb_multipliers,
                        raw.color_matrix_is_d65,
                    ),
                    None,
                ),
            };
            assert!(dcp.is_some(), "this assertion targets the DCP path; no profile found");

            let res = ImageResources::new(
                &ctx, 1, &raw.data, raw.width, raw.height, &base,
                raw.wb_multipliers, forward_matrix, raw.cfa_pattern,
                raw.black_levels, raw.white_level, dcp.as_ref(),
                // Full resolution, not the 1280px preview: the develop
                // preview's box-filter downsample averages isolated clipped
                // pixels away, leaving too few to measure.
                None, raw.orientation, raw.lens_context.clone()).expect("image resources");

            let (w, h) = res.oriented_dims(0);

            let flat = render_at(&ctx, &res, &raw, 0.0, w, h).await;
            let recovered = render_at(&ctx, &res, &raw, -1.0, w, h).await;
            assert_eq!(flat.len(), recovered.len());

            // The blown region: near-white at highlights 0. This is where the
            // complaint lives — it renders as a flat grey when pulled down.
            let mask: Vec<bool> = flat
                .as_chunks::<4>().0.iter()
                .map(|p| {
                    let l = 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
                    l > 245.0
                })
                .collect();
            let blown = mask.iter().filter(|m| **m).count();
            eprintln!("blown pixels sampled: {blown}");
            assert!(blown > 500, "not enough blown area to judge ({blown} px)");

            let cv_flat = luma_cv(&flat, &mask);
            let cv_rec = luma_cv(&recovered, &mask);
            let mean_flat = mean_luma(&flat, &mask);
            let mean_rec = mean_luma(&recovered, &mask);

            eprintln!("highlights  0: mean {:.1}  CV {:.5}", mean_flat, cv_flat);
            eprintln!("highlights -1: mean {:.1}  CV {:.5}", mean_rec, cv_rec);
            eprintln!("CV ratio: {:.2}x", cv_rec / cv_flat.max(1e-9));

            // Recovery must darken the region...
            assert!(mean_rec < mean_flat, "recovery did not darken the highlights");
            // ...and, crucially, must add gradation rather than translate a
            // flat band. A pure uniform scale leaves CV unchanged.
            assert!(
                cv_rec > cv_flat * 1.5,
                "highlights only darkened without restoring detail: CV {:.5} -> {:.5}",
                cv_flat,
                cv_rec
            );
        });
    }

    /// The calibration guarantee: with Highlights and Shadows at zero, moving
    /// them to scene-linear space must change NOTHING. Both blocks are guarded
    /// by `!= 0.0`, so this holds by construction — but it is the property that
    /// protects the hand-tuned `profile_curve = 0.33` match against camera
    /// JPEGs, so it is worth asserting rather than assuming.
    ///
    /// Renders twice at defaults and compares byte-for-byte; also serves as a
    /// determinism check on the render path.
    #[test]
    #[ignore]
    fn neutral_settings_render_is_stable() {
        let test_raw = test_raw_path();
        if !std::path::Path::new(&test_raw).exists() {
            eprintln!("test RAW missing, skipping");
            return;
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .expect("runtime");

        rt.block_on(async {
            let ctx = SharedContext::new().await.expect("gpu context");
            let raw = crate::raw::loader::load_raw_data(test_raw.clone())
                .await
                .expect("decode raw");
            let base = EditParams::default();
            assert_eq!(base.highlights, 0.0, "test assumes neutral defaults");
            assert_eq!(base.shadows, 0.0, "test assumes neutral defaults");

            let as_shot = crate::color::as_shot_kelvin_tint(&raw);
            let (kelvin, _t, _wb) =
                crate::color::solve_wb(&base, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
            let dcp = raw.dcp_profile.as_ref().map(|d| {
                crate::raw::dcp::interpolate_at_temperature(d, kelvin, crate::core::tonemap::ToneSettings::from_params(&base))
            });
            let forward_matrix = dcp.as_ref().map(|i| i.camera_to_prophoto).unwrap_or_else(|| {
                crate::color::calculate_cam_to_srgb(
                    raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65,
                )
            });

            let res = ImageResources::new(
                &ctx, 1, &raw.data, raw.width, raw.height, &base,
                raw.wb_multipliers, forward_matrix, raw.cfa_pattern,
                raw.black_levels, raw.white_level, dcp.as_ref(),
                Some(1280), raw.orientation, raw.lens_context.clone()).expect("image resources");
            let (w, h) = res.oriented_dims(0);

            let a = render_at(&ctx, &res, &raw, 0.0, w, h).await;
            let b = render_at(&ctx, &res, &raw, 0.0, w, h).await;
            assert_eq!(a.as_ref(), b.as_ref(), "neutral render is not deterministic");
            eprintln!("neutral render stable over {} bytes", a.len());
        });
    }

    /// Readback buffers are pooled (`take_readback`). A render that lands in a
    /// buffer a different, larger render filled before it must read back its
    /// own pixels: the same bytes as through a buffer of its own.
    #[test]
    #[ignore]
    fn a_reused_readback_buffer_returns_this_renders_pixels() {
        let test_raw = test_raw_path();
        if !std::path::Path::new(&test_raw).exists() {
            eprintln!("test RAW missing, skipping");
            return;
        }
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().expect("runtime");
        rt.block_on(async {
            let ctx = SharedContext::new().await.expect("gpu context");
            let raw = crate::raw::loader::load_raw_data(test_raw.clone()).await.expect("decode raw");
            let fm = crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65);
            let res = ImageResources::new(
                &ctx, 1, &raw.data, raw.width, raw.height, &EditParams::default(),
                raw.wb_multipliers, fm, raw.cfa_pattern, raw.black_levels, raw.white_level, None,
                Some(1280), raw.orientation, raw.lens_context.clone()).expect("image resources");
            let (w, h) = res.oriented_dims(0);
            let (sw, sh) = (w - 8, h - 8);

            // Through a buffer of its own.
            ctx.readbacks.lock().unwrap().clear();
            let own = render_at(&ctx, &res, &raw, 0.0, sw, sh).await;
            ctx.readbacks.lock().unwrap().clear();

            // A larger, different render fills a buffer and returns it…
            let other = render_at(&ctx, &res, &raw, -1.0, w, h).await;
            assert_eq!(ctx.readbacks.lock().unwrap().len(), 1, "the readback buffer was not pooled");
            // …and the smaller render reads back through that same buffer.
            let reused = render_at(&ctx, &res, &raw, 0.0, sw, sh).await;
            assert_eq!(ctx.readbacks.lock().unwrap().len(), 1, "the pooled buffer was not reused");

            assert_ne!(other.len(), reused.len());
            assert_eq!(own.as_ref(), reused.as_ref(), "a reused readback buffer returned other pixels");
        });
    }


    /// Drawing straight onto a surface (`render_to_target`) shows what the
    /// read-back path shows: the same pixels in the image's rectangle (two
    /// ±1 dithers whose patterns follow the screen position, so within 2,
    /// with no shift on average), exactly the background around it, and the
    /// same overview and histogram. In both byte orders a surface may use.
    /// Synthetic input, so CI runs it.
    #[test]
    #[ignore]
    fn drawing_onto_a_surface_matches_the_read_back_render() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().expect("runtime");
        rt.block_on(async {
            let ctx = SharedContext::new().await.expect("gpu context");
            let (w, h) = (600u32, 400u32);
            let data: Vec<u16> = (0..w * h).map(|i| 600 + ((i * 7919) % 3000) as u16).collect();
            let params = EditParams { exposure: 0.7, ..EditParams::default() };
            let res = ImageResources::new(
                &ctx, 1, &data, w, h, &params, [2.0, 1.0, 1.5, 1.0],
                [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], 0, [256; 4], 4095,
                None, None, 1, Default::default(),
            )
            .expect("image resources");
            res.update_uniforms(&ctx, &params, None, None);

            // A 400×400 "screen" showing a zoomed-in slice of the image in a
            // whole-pixel 320×160 band, so its pixels line up one to one with
            // a 320×160 read-back render of the same slice.
            let (sw, sh) = (400u32, 400u32);
            let rect = crate::core::viewport::Rect { x: 40.0, y: 120.0, width: 320.0, height: 160.0 };
            let placement = ([0.2, 0.1, 0.5, 0.5], rect);
            let dims = crate::core::viewport::overview_dims(w, h);
            let reference = render_display_and_overview(
                &ctx, &res, &res, 320, 160, placement.0, dims, Detail::default(),
            )
            .await;

            let background = wgpu::Color { r: 0.25, g: 0.5, b: 0.75, a: 1.0 };
            for format in [wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Bgra8Unorm] {
                let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("stand-in surface"),
                    size: wgpu::Extent3d { width: sw, height: sh, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                let mut presented = false;
                let out = render_to_target(
                    &ctx, &res, Some(&res),
                    DisplayTarget { view: &view, format, size: (sw, sh), placement: Some(placement), background },
                    dims, Detail::default(), || presented = true,
                )
                .expect("render to target");
                assert!(presented, "{format:?}: present was not called");
                let overview = out.overview.expect("an overview was asked for").wait(&ctx).expect("overview");
                assert_eq!(overview.pixels, reference.overview, "{format:?}: overview differs");
                assert_eq!(overview.histogram, reference.histogram, "{format:?}: histogram differs");

                // Read the stand-in surface back, as RGBA.
                let bytes_per_row = (sw * 4 + 255) & !255;
                let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: (bytes_per_row * sh) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = ctx.device.create_command_encoder(&Default::default());
                encoder.copy_texture_to_buffer(
                    texture.as_image_copy(),
                    wgpu::ImageCopyBuffer {
                        buffer: &buffer,
                        layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: None },
                    },
                    wgpu::Extent3d { width: sw, height: sh, depth_or_array_layers: 1 },
                );
                ctx.queue.submit(Some(encoder.finish()));
                buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                ctx.device.poll(wgpu::Maintain::Wait);
                let mapped = buffer.slice(..).get_mapped_range();
                let pixel = |x: u32, y: u32| {
                    let i = (y * bytes_per_row + x * 4) as usize;
                    let p = &mapped[i..i + 4];
                    if format == wgpu::TextureFormat::Bgra8Unorm { [p[2], p[1], p[0], p[3]] } else { [p[0], p[1], p[2], p[3]] }
                };

                let bg = [64u8, 128, 191, 255];
                let mut worst = 0u8;
                let (mut signed, mut n) = (0i64, 0i64);
                for y in 0..sh {
                    for x in 0..sw {
                        let got = pixel(x, y);
                        let inside = (40..360).contains(&x) && (120..280).contains(&y);
                        if inside {
                            let i = (((y - 120) * 320 + (x - 40)) * 4) as usize;
                            for c in 0..3 {
                                worst = worst.max(got[c].abs_diff(reference.display[i + c]));
                                signed += got[c] as i64 - reference.display[i + c] as i64;
                                n += 1;
                            }
                        } else {
                            for c in 0..3 {
                                assert!(got[c].abs_diff(bg[c]) <= 1, "{format:?}: background at ({x},{y}) is {got:?}");
                            }
                        }
                    }
                }
                assert!(worst <= 2, "{format:?}: the image differs from the read-back render by {worst}");
                let bias = signed as f64 / n as f64;
                assert!(bias.abs() < 0.05, "{format:?}: the image is shifted by {bias:.3} on average");
            }
        });
    }

    /// End-to-end proof that a selected tone operator actually reaches the
    /// GPU and renders sanely.
    ///
    /// This is the part that cannot be checked by reading code or by the CPU
    /// unit tests in `core::tonemap`: the operator's shape travels as *texture
    /// contents*, so a missed `write_tone_curve` call would leave the curve
    /// LUT at its zero-initialised state and the image would render BLACK —
    /// with no panic, no validation error, and nothing in the logs. The
    /// all-black assertion below is aimed squarely at that.
    ///
    /// Run with
    /// `cargo test --release tone_mapper_selection -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn tone_mapper_selection_changes_the_render_without_breaking_it() {
        use crate::core::tonemap::ToneMapper;

        let test_raw = test_raw_path();
        if !std::path::Path::new(&test_raw).exists() {
            eprintln!("test RAW missing, skipping");
            return;
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .expect("runtime");

        rt.block_on(async {
            let ctx = SharedContext::new().await.expect("gpu context");
            let raw = crate::raw::loader::load_raw_data(test_raw.clone())
                .await
                .expect("decode raw");

            let base = EditParams::default();
            let as_shot = crate::color::as_shot_kelvin_tint(&raw);
            let (kelvin, _t, _wb) =
                crate::color::solve_wb(&base, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
            let dcp = raw.dcp_profile.as_ref().map(|d| {
                crate::raw::dcp::interpolate_at_temperature(
                    d,
                    kelvin,
                    crate::core::tonemap::ToneSettings::from_params(&base),
                )
            });
            let fm = dcp.as_ref().map(|i| i.camera_to_prophoto).unwrap_or_else(|| {
                crate::color::calculate_cam_to_srgb(
                    raw.color_matrix,
                    raw.wb_multipliers,
                    raw.color_matrix_is_d65,
                )
            });
            let res = ImageResources::new(
                &ctx, 1, &raw.data, raw.width, raw.height, &base,
                raw.wb_multipliers, fm, raw.cfa_pattern, raw.black_levels,
                raw.white_level, dcp.as_ref(), Some(1280), raw.orientation, raw.lens_context.clone()).expect("image resources");
            let (w, h) = res.oriented_dims(0);

            // Render one frame per operator, re-resolving the DCP each time so
            // the tone LUT is re-baked exactly the way the app does it.
            let render_with = |mapper: ToneMapper| {
                let ctx = &ctx;
                let res = &res;
                let raw = &raw;
                async move {
                    let mut params = EditParams::default();
                    params.tone_mapper = mapper;
                    let tone = crate::core::tonemap::ToneSettings::from_params(&params);
                    let as_shot = crate::color::as_shot_kelvin_tint(raw);
                    let (kelvin, _t, wb) = crate::color::solve_wb(
                        &params, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix,
                    );
                    let interp = raw.dcp_profile.as_ref().map(|d| {
                        crate::raw::dcp::interpolate_at_temperature(d, kelvin, tone)
                    });
                    res.update_uniforms(ctx, &params, interp.as_ref(), wb);
                    let (bytes, _, _) =
                        render_to_bytes(ctx, res, w, h, FULL_VIEW_RECT).await;
                    bytes
                }
            };

            let camera = render_with(ToneMapper::Camera).await;
            assert!(!camera.is_empty(), "camera render produced nothing");
            let curve_less = dcp.as_ref().is_some_and(|d| !d.has_tone_curve);
            eprintln!("profile has its own tone curve: {}", !curve_less);

            for mapper in [
                ToneMapper::Filmic,
                ToneMapper::Reinhard,
                ToneMapper::Hable,
                ToneMapper::AcesFitted,
                ToneMapper::Aces2,
                ToneMapper::Gt,
            ] {
                let out = render_with(mapper).await;
                assert_eq!(out.len(), camera.len(), "{mapper}: size changed");

                let mean = out
                    .as_chunks::<4>().0.iter()
                    .map(|p| {
                        0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
                    })
                    .sum::<f64>()
                    / (out.len() / 4) as f64;

                // A missing LUT upload renders black; a runaway curve renders
                // white. Both are silent failures without this.
                assert!(
                    mean > 5.0,
                    "{mapper} rendered essentially black (mean luma {mean:.1}) — \
                     the tone curve LUT probably was not uploaded"
                );
                assert!(
                    mean < 250.0,
                    "{mapper} rendered essentially white (mean luma {mean:.1})"
                );

                // It must actually be a DIFFERENT rendering, or the selection
                // silently did nothing — except Filmic on a profile with no
                // curve of its own ("Adobe Standard"), where Camera IS the
                // built-in filmic: the two must then agree (apart from
                // rounding where they are applied at different steps).
                let differing = out
                    .iter()
                    .zip(camera.iter())
                    .filter(|(a, b)| a != b)
                    .count();
                let frac = differing as f64 / out.len() as f64;
                if mapper == ToneMapper::Filmic && curve_less {
                    assert!(
                        frac < 0.05,
                        "Camera on a curve-less profile should render as Filmic, but {:.2}% of channels differ",
                        frac * 100.0
                    );
                } else {
                    assert!(
                        frac > 0.05,
                        "{mapper} changed only {:.2}% of channels — selection had no effect",
                        frac * 100.0
                    );
                }

                eprintln!("{mapper:>9}: mean luma {mean:6.1}   {:.1}% of channels differ from Camera",
                    frac * 100.0);
            }
        });
    }

    /// General-purpose "did I change the render?" harness.
    ///
    /// Capture a baseline BEFORE a refactor, then compare after:
    /// ```text
    /// BASELINE=write cargo test --release render_matches_baseline -- --ignored
    /// # ...make the change...
    /// cargo test --release render_matches_baseline -- --ignored --nocapture
    /// ```
    /// Asserts ≤1 LSB per channel — not bit-exactness, because reassociating
    /// float work legitimately shifts the last bit. Anything larger is a real
    /// difference. Used to prove the camera→ProPhoto matrix fold was a pure
    /// optimisation (result: max delta 1, 70 of 10.7M channels affected).
    #[test]
    #[ignore]
    fn render_matches_baseline() {
        let test_raw = test_raw_path();
        if !std::path::Path::new(&test_raw).exists() { return; }
        let dump = std::env::var("BASELINE_PATH").unwrap_or_else(|_| {
            std::env::temp_dir()
                .join("raw-editor-render-baseline.bin")
                .to_string_lossy()
                .into_owned()
        });
        let writing = std::env::var("BASELINE").as_deref() == Ok("write");

        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        rt.block_on(async {
            let ctx = SharedContext::new().await.unwrap();
            let raw = crate::raw::loader::load_raw_data(test_raw.clone()).await.unwrap();
            let base = EditParams::default();
            let as_shot = crate::color::as_shot_kelvin_tint(&raw);
            let (kelvin, _t, _wb) = crate::color::solve_wb(&base, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
            let dcp = raw.dcp_profile.as_ref().map(|d| crate::raw::dcp::interpolate_at_temperature(d, kelvin, crate::core::tonemap::ToneSettings::from_params(&base)));
            assert!(dcp.is_some(), "needs the DCP path");
            let fm = dcp.as_ref().map(|i| i.camera_to_prophoto).unwrap();
            let res = ImageResources::new(&ctx, 1, &raw.data, raw.width, raw.height, &base,
                raw.wb_multipliers, fm, raw.cfa_pattern, raw.black_levels, raw.white_level,
                dcp.as_ref(), Some(1280), raw.orientation, raw.lens_context.clone()).unwrap();
            let (w, h) = res.oriented_dims(0);
            let bytes = render_at(&ctx, &res, &raw, 0.0, w, h).await;

            if writing {
                std::fs::write(&dump, bytes.as_ref()).unwrap();
                eprintln!("BASELINE written: {} bytes -> {dump}", bytes.len());
                return;
            }
            let Ok(old) = std::fs::read(&dump) else {
                eprintln!("no baseline at {dump}; run with BASELINE=write first — skipping");
                return;
            };
            assert_eq!(old.len(), bytes.len(), "render size changed");
            let mut max_delta = 0i32;
            let mut n_diff = 0usize;
            for (a, b) in old.iter().zip(bytes.iter()) {
                let d = (*a as i32 - *b as i32).abs();
                if d > 0 { n_diff += 1; }
                if d > max_delta { max_delta = d; }
            }
            eprintln!("EQUIV max per-channel delta = {max_delta}  ({n_diff} of {} channels differ)", old.len());
            assert!(max_delta <= 1, "matrix fold changed the render by {max_delta} LSB — likely a transposition");
        });
    }

    /// The develop render's overview is the edited proxy: it must come back
    /// at the requested (crop-aware) size, agree with the display pass, and
    /// follow the edit — the whole point of showing it instead of the camera
    /// JPEG while an image reloads.
    ///
    /// Run with `cargo test --release overview_follows -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn overview_follows_the_edit_and_matches_the_display() {
        let test_raw = test_raw_path();
        if !std::path::Path::new(&test_raw).exists() {
            eprintln!("test RAW missing, skipping");
            return;
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .expect("runtime");

        rt.block_on(async {
            let ctx = SharedContext::new().await.expect("gpu context");
            let raw = crate::raw::loader::load_raw_data(test_raw.clone())
                .await
                .expect("decode raw");
            let base = EditParams::default();
            let as_shot = crate::color::as_shot_kelvin_tint(&raw);
            let (kelvin, _t, _wb) =
                crate::color::solve_wb(&base, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
            let dcp = raw.dcp_profile.as_ref().map(|d| {
                crate::raw::dcp::interpolate_at_temperature(
                    d,
                    kelvin,
                    crate::core::tonemap::ToneSettings::from_params(&base),
                )
            });
            let forward_matrix = dcp.as_ref().map(|i| i.camera_to_prophoto).unwrap_or_else(|| {
                crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65)
            });
            let res = ImageResources::new(
                &ctx, 1, &raw.data, raw.width, raw.height, &base,
                raw.wb_multipliers, forward_matrix, raw.cfa_pattern,
                raw.black_levels, raw.white_level, dcp.as_ref(),
                Some(1280), raw.orientation, raw.lens_context.clone()).expect("image resources");

            let mean = |rgba: &[u8]| {
                let n = (rgba.len() / 4).max(1) as f64;
                rgba.as_chunks::<4>().0.iter()
                    .map(|p| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64)
                    .sum::<f64>() / n
            };

            let (w, h) = res.oriented_dims(0);
            let dims = crate::core::viewport::overview_dims(w, h);
            let mut means = Vec::new();
            for exposure in [0.0, -2.0] {
                let params = EditParams { exposure, ..base };
                res.update_uniforms(&ctx, &params, dcp.as_ref(), None);
                let out = render_display_and_overview(&ctx, &res, &res, w, h, FULL_VIEW_RECT, dims, Detail::default()).await;
                assert_eq!(out.overview.len(), (dims.0 * dims.1 * 4) as usize, "overview size");
                let (display, overview) = (mean(&out.display), mean(&out.overview));
                eprintln!("exposure {exposure:+}: display mean {display:.1}, overview mean {overview:.1} ({}x{})", dims.0, dims.1);
                // Same image, same edit: the two passes must agree in tone.
                assert!((display - overview).abs() < 6.0, "overview diverges from display");
                means.push(overview);
            }
            assert!(means[1] < means[0] - 20.0, "overview did not follow a -2 EV edit: {means:?}");
        });
    }

    /// Visual-inspection harness: render a RAW with given edits and write PNG
    /// crops, for judging demosaic / denoise / lens work by eye.
    ///
    /// ```text
    /// DUMP_DIR=/tmp/out DUMP_TAG=rcd DUMP_CROPS="2900,1900,600,400;100,100,600,400" \
    ///   [DUMP_PARAMS='{"luma_noise":0.5,...}'] [DUMP_PREVIEW=1280] \
    ///   [DUMP_PROFILE='/path/to/profile.dcp'] \
    ///   cargo test --release dump_render_crops -- --ignored --nocapture
    /// ```
    /// Crops are in oriented full-resolution pixels. Also writes a ~1600px
    /// downscale of the whole frame.
    #[test]
    #[ignore]
    fn dump_render_crops() {
        let Ok(dir) = std::env::var("DUMP_DIR") else {
            eprintln!("DUMP_DIR not set, skipping");
            return;
        };
        let test_raw = test_raw_path();
        let tag = std::env::var("DUMP_TAG").unwrap_or_else(|_| "render".into());
        let crops: Vec<[u32; 4]> = std::env::var("DUMP_CROPS")
            .unwrap_or_default()
            .split(';')
            .filter_map(|c| {
                let v: Vec<u32> = c.split(',').filter_map(|n| n.trim().parse().ok()).collect();
                (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
            })
            .collect();
        let preview: Option<u32> = std::env::var("DUMP_PREVIEW").ok().and_then(|v| v.parse().ok());

        // GPU validation errors only reach `tracing`; surface them here.
        let _ = tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter("warn").try_init();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        rt.block_on(async {
            let ctx = SharedContext::new().await.unwrap();
            let mut raw = crate::raw::loader::load_raw_data(test_raw.clone()).await.unwrap();
            eprintln!("lens: {:?}", raw.lens_context);
            // The decode's automatic pick otherwise; the app's per-camera
            // default can differ, so a look can be pinned explicitly
            // ("none" renders the profile-less fallback path).
            match std::env::var("DUMP_PROFILE").as_deref() {
                Ok("none") => raw.dcp_profile = None,
                Ok(path) => {
                    raw.dcp_profile = crate::raw::dcp::load_profile(std::path::Path::new(path));
                    assert!(raw.dcp_profile.is_some(), "could not load {path}");
                }
                Err(_) => {}
            }
            let params: EditParams = std::env::var("DUMP_PARAMS")
                .ok()
                .map(|j| {
                    let mut v: serde_json::Value = serde_json::to_value(EditParams::default()).unwrap();
                    let patch: serde_json::Value = serde_json::from_str(&j).expect("DUMP_PARAMS json");
                    for (k, val) in patch.as_object().unwrap() {
                        v[k] = val.clone();
                    }
                    serde_json::from_value(v).unwrap()
                })
                .unwrap_or_default();
            let as_shot = crate::color::as_shot_kelvin_tint(&raw);
            let (kelvin, _t, wb) =
                crate::color::solve_wb(&params, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
            let dcp = raw.dcp_profile.as_ref().map(|d| {
                crate::raw::dcp::interpolate_at_temperature(d, kelvin, crate::core::tonemap::ToneSettings::from_params(&params))
            });
            let fm = dcp.as_ref().map(|i| i.camera_to_prophoto).unwrap_or_else(|| {
                crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65)
            });
            let res = ImageResources::new(&ctx, 1, &raw.data, raw.width, raw.height, &params,
                raw.wb_multipliers, fm, raw.cfa_pattern, raw.black_levels, raw.white_level,
                dcp.as_ref(), preview, raw.orientation, raw.lens_context.clone()).unwrap();
            res.update_uniforms(&ctx, &params, dcp.as_ref(), wb);
            let (w, h) = res.oriented_dims(params.quarter_turns);
            let t = std::time::Instant::now();
            let (bytes, _, ms) = render_to_bytes(&ctx, &res, w, h, FULL_VIEW_RECT).await;
            eprintln!("rendered {w}x{h} in {ms:.1} ms (wall {:.1} ms)", t.elapsed().as_secs_f32() * 1000.0);
            let img = image::RgbaImage::from_raw(w, h, bytes.to_vec()).unwrap();
            let dir = std::path::Path::new(&dir);
            std::fs::create_dir_all(dir).unwrap();
            let small = image::imageops::resize(&img, 1600, 1600 * h / w, image::imageops::FilterType::Triangle);
            small.save(dir.join(format!("{tag}_full.png"))).unwrap();
            for (i, c) in crops.iter().enumerate() {
                let sub = image::imageops::crop_imm(&img, c[0], c[1], c[2], c[3]).to_image();
                sub.save(dir.join(format!("{tag}_crop{i}.png"))).unwrap();
            }
            eprintln!("wrote {} crops to {}", crops.len(), dir.display());
        });
    }

    /// Render `params` the way the app does (WB/DCP resolved from them) at
    /// the develop preview resolution. Returns RGBA and its size.
    async fn render_params(
        ctx: &SharedContext,
        raw: &crate::raw::loader::RawDataResult,
        params: &EditParams,
    ) -> (Vec<u8>, u32, u32) {
        render_params_with(ctx, raw, params, None).await
    }

    /// [`render_params`] through the 16-bit pipeline: RGB, no dither.
    async fn render_params_16(
        ctx: &SharedContext,
        raw: &crate::raw::loader::RawDataResult,
        params: &EditParams,
    ) -> (Vec<u16>, u32, u32) {
        let as_shot = crate::color::as_shot_kelvin_tint(raw);
        let (kelvin, _t, wb) =
            crate::color::solve_wb(params, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
        let dcp = raw.dcp_profile.as_ref().map(|d| {
            crate::raw::dcp::interpolate_at_temperature(d, kelvin, crate::core::tonemap::ToneSettings::from_params(params))
        });
        let fm = dcp.as_ref().map(|i| i.camera_to_prophoto).unwrap_or_else(|| {
            crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65)
        });
        let res = ImageResources::new(ctx, 1, &raw.data, raw.width, raw.height, params,
            raw.wb_multipliers, fm, raw.cfa_pattern, raw.black_levels, raw.white_level,
            dcp.as_ref(), Some(1280), raw.orientation, raw.lens_context.clone()).expect("image resources");
        res.update_uniforms(ctx, params, dcp.as_ref(), wb);
        let (w, h) = res.oriented_dims(params.quarter_turns);
        let (samples, _, _) = render_to_bytes_16bit(ctx, &res, w, h).await.expect("16-bit render");
        (samples, w, h)
    }

    async fn render_params_with(
        ctx: &SharedContext,
        raw: &crate::raw::loader::RawDataResult,
        params: &EditParams,
        ai: Option<std::sync::Arc<crate::gpu::ai_mask::AiBitmaps>>,
    ) -> (Vec<u8>, u32, u32) {
        let as_shot = crate::color::as_shot_kelvin_tint(raw);
        let (kelvin, _t, wb) =
            crate::color::solve_wb(params, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
        let dcp = raw.dcp_profile.as_ref().map(|d| {
            crate::raw::dcp::interpolate_at_temperature(d, kelvin, crate::core::tonemap::ToneSettings::from_params(params))
        });
        let fm = dcp.as_ref().map(|i| i.camera_to_prophoto).unwrap_or_else(|| {
            crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65)
        });
        let res = ImageResources::new(ctx, 1, &raw.data, raw.width, raw.height, params,
            raw.wb_multipliers, fm, raw.cfa_pattern, raw.black_levels, raw.white_level,
            dcp.as_ref(), Some(1280), raw.orientation, raw.lens_context.clone()).expect("image resources");
        if let Some(ai) = ai {
            res.set_ai_bitmaps(ai);
        }
        res.update_uniforms(ctx, params, dcp.as_ref(), wb);
        let (w, h) = res.oriented_dims(params.quarter_turns);
        let (bytes, _, _) = render_to_bytes(ctx, &res, w, h, FULL_VIEW_RECT).await;
        (bytes.to_vec(), w, h)
    }

    /// An AI mask darkens exactly its bitmap's coverage, and the bitmap —
    /// stored in the sensor frame — follows the EXIF orientation and a
    /// quarter turn.
    ///
    /// Run with `cargo test --release ai_mask_hits -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ai_mask_hits_only_its_bitmap() {
        use crate::core::types::MaskParams;
        let test_raw = test_raw_path();
        if !std::path::Path::new(&test_raw).exists() {
            return;
        }
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        rt.block_on(async {
            let ctx = SharedContext::new().await.unwrap();
            let raw = crate::raw::loader::load_raw_data(test_raw.clone()).await.unwrap();
            let (bw, bh) = (raw.width / 4, raw.height / 4);
            let data = (0..bw * bh).map(|i| if i % bw < bw / 2 { 255 } else { 0 }).collect();
            let bitmap = crate::ml::masks::Bitmap { width: bw, height: bh, data };
            let ai: crate::gpu::ai_mask::AiBitmaps = [(42u64, std::sync::Arc::new(bitmap))].into_iter().collect();
            let ai = std::sync::Arc::new(ai);

            for turns in [0u32, 1] {
                let base = EditParams { quarter_turns: turns, ..EditParams::default() };
                let (plain, w, h) = render_params(&ctx, &raw, &base).await;
                let mut p = base;
                p.masks[0] = MaskParams::new_ai();
                p.masks[0].ai_id = 42;
                p.masks[0].exposure = -2.5;
                p.mask_count = 1;
                let (masked, _, _) = render_params_with(&ctx, &raw, &p, Some(ai.clone())).await;
                // The sensor's left and right halves, wherever this
                // orientation puts them on screen.
                let o = crate::color::compose_orientation(raw.orientation, turns);
                let around = |su: f32, sv: f32| {
                    let (u, v) = crate::color::sensor_to_display_uv(su, sv, o);
                    (u - 0.1, v - 0.1, u + 0.1, v + 0.1)
                };
                let (inside, outside) = (around(0.25, 0.5), around(0.75, 0.5));
                let (i0, i1) = (region_mean(&plain, w, h, inside), region_mean(&masked, w, h, inside));
                let (o0, o1) = (region_mean(&plain, w, h, outside), region_mean(&masked, w, h, outside));
                eprintln!("turns {turns}: inside {i0:.1} → {i1:.1}, outside {o0:.1} → {o1:.1}");
                assert!(i1 < i0 * 0.6, "turns {turns}: the AI mask did not darken its region");
                assert!((o1 - o0).abs() < 0.5, "turns {turns}: the AI mask leaked outside its bitmap");

                // Without decoded bitmaps the mask is empty, never garbage.
                let (blank, _, _) = render_params(&ctx, &raw, &p).await;
                let (b, _) = (region_mean(&blank, w, h, inside), 0);
                assert!((b - i0).abs() < 0.5, "turns {turns}: a missing bitmap changed the image");
            }
        });
    }

    /// Every export colour space must describe the same picture: read back
    /// through `core::output_space` (the matrices and curves its ICC profile
    /// states), a colour sRGB can hold comes out the same from all four. A
    /// shader matrix or curve that drifted from the profile — the export
    /// would then display wrong in every colour-managed viewer — fails here.
    #[test]
    #[ignore]
    fn export_colour_spaces_agree_on_colours_srgb_can_hold() {
        use crate::core::output_space::OutputSpace;
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        rt.block_on(async {
            let ctx = SharedContext::new().await.unwrap();
            // The test photo if it is here, else a frame of gentle colours
            // (so CI, which has no photos, still checks the maths).
            let path = test_raw_path();
            let raw = if std::path::Path::new(&path).exists() {
                crate::raw::loader::load_raw_data(path).await.unwrap()
            } else {
                gentle_colours_raw(512, 384)
            };
            let params = EditParams { saturation: 30.0, ..EditParams::default() };
            let as_shot = crate::color::as_shot_kelvin_tint(&raw);
            let (kelvin, _t, wb) =
                crate::color::solve_wb(&params, as_shot, raw.dcp_profile.as_deref(), raw.color_matrix);
            let dcp = raw.dcp_profile.as_ref().map(|d| {
                crate::raw::dcp::interpolate_at_temperature(d, kelvin, crate::core::tonemap::ToneSettings::from_params(&params))
            });
            let fm = dcp.as_ref().map(|i| i.camera_to_prophoto).unwrap_or_else(|| {
                crate::color::calculate_cam_to_srgb(raw.color_matrix, raw.wb_multipliers, raw.color_matrix_is_d65)
            });
            let res = ImageResources::new(&ctx, 1, &raw.data, raw.width, raw.height, &params,
                raw.wb_multipliers, fm, raw.cfa_pattern, raw.black_levels, raw.white_level,
                dcp.as_ref(), Some(640), raw.orientation, raw.lens_context.clone()).expect("image resources");
            res.update_uniforms(&ctx, &params, dcp.as_ref(), wb);
            let (w, h) = res.oriented_dims(params.quarter_turns);

            // Linear sRGB of every pixel, read back through the space's own
            // curve and matrix.
            let mut renders = Vec::new();
            for space in OutputSpace::ALL {
                write_output_space(&ctx, &res, space);
                let (samples, _, _) = render_to_bytes_16bit(&ctx, &res, w, h).await.expect("readback");
                let back = space.space_to_srgb_matrix();
                let lin: Vec<[f32; 3]> = samples
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|p| {
                        let v = [0, 1, 2].map(|i| space.decode(p[i] as f32 / 65535.0));
                        crate::color::mat3_apply(&back, v)
                    })
                    .collect();
                renders.push((space, lin));
            }
            write_output_space(&ctx, &res, OutputSpace::Srgb);

            let srgb = &renders[0].1;
            // Colours well inside sRGB: no gamut map touched them anywhere.
            // Judged on the sRGB render, so a channel it pinned at 0 or 1 marks
            // a colour it had to bring in (a dark cyan past sRGB's red edge
            // reads as low chroma once mapped), which a wider space keeps.
            let inside: Vec<usize> = (0..srgb.len())
                .filter(|&i| {
                    let lab = crate::core::perceptual::linear_srgb_to_oklab(srgb[i]);
                    let unpinned = srgb[i].iter().all(|&v| v > 1e-3 && v < 0.999);
                    unpinned && lab[0] > 0.05 && lab[0] < 0.97 && lab[1].hypot(lab[2]) < 0.06
                })
                .collect();
            assert!(inside.len() > srgb.len() / 4, "test image has too few ordinary colours");
            for (space, lin) in &renders[1..] {
                // Relative to the pixel's level: the 16-bit target is half
                // float, whose step grows with the value, so an absolute
                // error would depend on how bright the test image is.
                let mut errs: Vec<f32> = inside
                    .iter()
                    .map(|&i| {
                        let level = srgb[i].iter().cloned().fold(0.0, f32::max).max(0.02);
                        (0..3).map(|c| (lin[i][c] - srgb[i][c]).abs()).fold(0.0, f32::max) / level
                    })
                    .collect();
                errs.sort_by(f32::total_cmp);
                let p99 = errs[errs.len() * 99 / 100];
                let mean = errs.iter().sum::<f32>() / errs.len() as f32;
                eprintln!("{space}: mean {mean:.2e}, p99 {p99:.2e} over {} px", errs.len());
                // Half-float rounding gives ~7e-4 mean, 1.6e-3 p99; one matrix
                // coefficient 1% off gives 2–3e-3 mean, ~7e-3 p99.
                assert!(mean < 1.2e-3 && p99 < 3.5e-3, "{space} disagrees with sRGB: mean {mean}, p99 {p99}");
            }
        });
    }

    /// A synthetic frame of ordinary colours: brightness ramps left to right,
    /// and a gentle tint turns through every hue top to bottom. Neutral after
    /// the as-shot WB, apart from the tint.
    fn gentle_colours_raw(w: u32, h: u32) -> crate::raw::loader::RawDataResult {
        let mut raw = crate::ml::segment::test_support::disc_raw(w, h);
        let colours = crate::gpu::demosaic::tile_colors(raw.cfa_pattern);
        let span = raw.white_level as f32;
        for y in 0..h {
            let hue = y as f32 / h as f32 * std::f32::consts::TAU;
            for x in 0..w {
                let c = colours[((y % 2) * 2 + x % 2) as usize] as usize;
                let tint = 1.0 + 0.06 * (hue + c as f32 * std::f32::consts::TAU / 3.0).sin();
                let level = span * (0.02 + 0.6 * x as f32 / w as f32);
                raw.data[(y * w + x) as usize] = (level * tint / raw.wb_multipliers[c]) as u16;
            }
        }
        raw
    }

    fn luma(p: &[u8]) -> f64 {
        0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
    }

    /// Mean luma of the pixels of `rgba` (w×h) inside the UV rect.
    fn region_mean(rgba: &[u8], w: u32, h: u32, (u0, v0, u1, v1): (f32, f32, f32, f32)) -> f64 {
        let (mut sum, mut n) = (0.0, 0usize);
        for y in (v0 * h as f32) as u32..(v1 * h as f32) as u32 {
            for x in (u0 * w as f32) as u32..(u1 * w as f32) as u32 {
                sum += luma(&rgba[((y * w + x) * 4) as usize..]);
                n += 1;
            }
        }
        sum / n.max(1) as f64
    }

    /// Brush strokes, erase strokes and a luminance-range mask must each land
    /// where they should and nowhere else. A brush pipeline that fails to
    /// build (it once did, over a reserved WGSL word) renders NOTHING — no
    /// panic, no log in a release build — which only a check like this sees.
    ///
    /// Run with `cargo test --release masks_hit_only -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn masks_hit_only_their_regions() {
        use crate::core::types::MaskParams;
        let test_raw = test_raw_path();
        if !std::path::Path::new(&test_raw).exists() {
            return;
        }
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        rt.block_on(async {
            let ctx = SharedContext::new().await.unwrap();
            let raw = crate::raw::loader::load_raw_data(test_raw.clone()).await.unwrap();
            let base = EditParams::default();
            let (plain, w, h) = render_params(&ctx, &raw, &base).await;

            // Mask 0: brush, −2.5 EV, a horizontal band at v = 0.5, with an
            // erase stroke crossing it at u = 0.5.
            let mut p = base;
            p.masks[0] = MaskParams::new_brush();
            p.masks[0].exposure = -2.5;
            p.mask_count = 1;
            p.brush.begin(0, false, 0.03, 0.0, 1.0).unwrap();
            p.brush.extend([0.1, 0.5], 1.0);
            p.brush.extend([0.9, 0.5], 1.0);
            p.brush.begin(0, true, 0.03, 0.0, 1.0).unwrap();
            p.brush.extend([0.5, 0.4], 1.0);
            p.brush.extend([0.5, 0.6], 1.0);
            let (painted, _, _) = render_params(&ctx, &raw, &p).await;

            let band = (0.2, 0.49, 0.4, 0.51);
            let erased = (0.495, 0.49, 0.505, 0.51);
            let outside = (0.2, 0.7, 0.4, 0.8);
            let (b0, b1) = (region_mean(&plain, w, h, band), region_mean(&painted, w, h, band));
            let (e0, e1) = (region_mean(&plain, w, h, erased), region_mean(&painted, w, h, erased));
            let (o0, o1) = (region_mean(&plain, w, h, outside), region_mean(&painted, w, h, outside));
            eprintln!("band {b0:.1} → {b1:.1}, erased {e0:.1} → {e1:.1}, outside {o0:.1} → {o1:.1}");
            assert!(b1 < b0 * 0.6, "brush stroke did not darken its band");
            assert!((e1 - e0).abs() < 1.0, "erase stroke did not restore the band");
            assert!((o1 - o0).abs() < 0.5, "brush leaked outside its stroke");

            // Mask 0 again as a luminance range: −1.5 EV above 0.6 display.
            let mut r = base;
            r.masks[0] = MaskParams::new_range();
            r.masks[0].luma_lo = 0.6;
            r.masks[0].luma_smooth = 0.02;
            r.masks[0].exposure = -1.5;
            r.mask_count = 1;
            let (ranged, _, _) = render_params(&ctx, &raw, &r).await;
            let (mut dark_shift, mut dark_n, mut bright_shift, mut bright_n) = (0.0, 0, 0.0, 0);
            for (a, b) in plain.as_chunks::<4>().0.iter().zip(ranged.as_chunks::<4>().0) {
                let (la, lb) = (luma(a), luma(b));
                if la < 120.0 {
                    dark_shift += (lb - la).abs();
                    dark_n += 1;
                } else if la > 190.0 {
                    bright_shift += la - lb;
                    bright_n += 1;
                }
            }
            let (dark, bright) = (dark_shift / dark_n.max(1) as f64, bright_shift / bright_n.max(1) as f64);
            eprintln!("range mask: dark pixels moved {dark:.2}, bright pixels darkened {bright:.1}");
            assert!(dark < 0.5, "range mask touched the shadows");
            assert!(bright > 20.0, "range mask did not darken the highlights");
        });
    }

    /// A minimal dual-illuminant profile with no table, and `curve` as its
    /// ProfileToneCurve (`None`: none, like "Adobe Standard").
    fn synthetic_profile(curve: Option<Vec<(f32, f32)>>) -> std::sync::Arc<crate::raw::dcp::DcpProfile> {
        let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        // Camera white → D50 white, so a neutral stays neutral.
        let forward = [0.9642, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.8249];
        std::sync::Arc::new(crate::raw::dcp::DcpProfile {
            illuminant1: 17,
            illuminant2: 21,
            color_matrix_1: identity,
            color_matrix_2: identity,
            forward_matrix_1: Some(forward),
            forward_matrix_2: Some(forward),
            hue_sat_dims: (0, 0, 0),
            hue_sat_data_1: None,
            hue_sat_data_2: None,
            tone_curve: curve,
            baseline_exposure_offset: 0.0,
            camera_model: None,
            profile_name: Some("Synthetic".into()),
        })
    }

    /// A flat frame clipped at the sensor's white level. Its as-shot WB is a
    /// camera's (red ×2, blue ×1.5), so clipped white lands above 1.0
    /// scene-linear, as it does in a real frame.
    fn clipped_frame(profile: Option<std::sync::Arc<crate::raw::dcp::DcpProfile>>) -> crate::raw::loader::RawDataResult {
        let mut raw = crate::ml::segment::test_support::disc_raw(64, 64);
        raw.data = vec![raw.white_level as u16; 64 * 64];
        raw.dcp_profile = profile;
        raw
    }

    fn centre_luma(img: &[u8], w: u32, h: u32) -> f64 {
        luma(&img[(((h / 2) * w + w / 2) * 4) as usize..])
    }

    /// Sensor-clipped white must render as (near) white on the default
    /// Camera tone, with or without a profile curve. With a curve-less
    /// profile the Camera path once kneed the highlights before the built-in
    /// filmic did the same, and clipped white came out 233/255 grey.
    /// Synthetic, so it runs wherever a GPU (lavapipe) does.
    #[test]
    #[ignore]
    fn clipped_white_renders_white_on_every_profile_kind() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        rt.block_on(async {
            let ctx = SharedContext::new().await.unwrap();
            let gentle_s = vec![(0.0, 0.0), (0.25, 0.2), (0.75, 0.8), (1.0, 1.0)];
            for (kind, profile) in [
                ("no profile", None),
                ("a profile without a curve", Some(synthetic_profile(None))),
                ("a profile with a curve", Some(synthetic_profile(Some(gentle_s)))),
            ] {
                let raw = clipped_frame(profile);
                let (img, w, h) = render_params(&ctx, &raw, &EditParams::default()).await;
                let l = centre_luma(&img, w, h);
                eprintln!("{kind}: clipped white renders at {l:.1}");
                assert!(l >= 244.0, "{kind}: clipped white renders grey ({l:.1}/255)");
            }
        });
    }

    /// On a profile without a curve, Camera IS the built-in filmic: choosing
    /// Filmic must not change a neutral frame at any level. (Colours may
    /// differ slightly: the operator is applied hue-preservingly in ProPhoto
    /// at the profile-curve slot, the built-in one at Step 15.)
    #[test]
    #[ignore]
    fn camera_on_a_curve_less_profile_renders_as_filmic() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        rt.block_on(async {
            let ctx = SharedContext::new().await.unwrap();
            let filmic = EditParams { tone_mapper: crate::core::tonemap::ToneMapper::Filmic, ..EditParams::default() };
            for fraction in [0.05f32, 0.2, 0.5, 0.8, 1.0] {
                let mut raw = clipped_frame(Some(synthetic_profile(None)));
                // Neutral after white balance: each site at level / its WB.
                let colours = crate::gpu::demosaic::tile_colors(raw.cfa_pattern);
                let level = raw.white_level as f32 * fraction;
                for (i, v) in raw.data.iter_mut().enumerate() {
                    let (x, y) = (i % 64, i / 64);
                    let c = colours[(y % 2) * 2 + x % 2] as usize;
                    *v = (level / raw.wb_multipliers[c]) as u16;
                }
                let (a, w, h) = render_params(&ctx, &raw, &EditParams::default()).await;
                let (b, _, _) = render_params(&ctx, &raw, &filmic).await;
                let (la, lb) = (centre_luma(&a, w, h), centre_luma(&b, w, h));
                assert!((la - lb).abs() <= 1.5, "at {fraction} of white: Camera {la:.1}, Filmic {lb:.1}");
            }
        });
    }

    /// Denoising must cut noise without moving tone: averaging is done on
    /// linear values exactly so that it cannot darken the shadows.
    ///
    /// Measured on the 16-bit render: the 8-bit one is dithered (±1 LSB), and
    /// that noise is the output's, not the image's.
    ///
    /// Run with `cargo test --release denoise_reduces -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn denoise_reduces_noise_without_shifting_tone() {
        let test_raw = std::env::var("RAWEDITOR_TEST_RAW_NOISY").unwrap_or_else(|_| {
            "/home/hsf001/Pictures/Photography/TESTFiles/RAW/_DSC0200.NEF".to_string()
        });
        if !std::path::Path::new(&test_raw).exists() {
            return;
        }
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        rt.block_on(async {
            let ctx = SharedContext::new().await.unwrap();
            let raw = crate::raw::loader::load_raw_data(test_raw.clone()).await.unwrap();
            let off = EditParams { color_noise: 0.0, ..EditParams::default() };
            let on = EditParams { luma_noise: 0.6, color_noise: 0.6, ..EditParams::default() };
            let (a, w, h) = render_params_16(&ctx, &raw, &off).await;
            let (b, _, _) = render_params_16(&ctx, &raw, &on).await;
            // Luma in 8-bit units, from 16-bit RGB.
            let luma16 = |p: &[u16]| (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64) / 257.0;
            // Pixel-to-pixel luma differences: dominated by noise in flat areas.
            let roughness = |img: &[u16]| {
                let mut s = 0.0;
                for y in 0..h {
                    for x in 0..w - 1 {
                        let i = ((y * w + x) * 3) as usize;
                        s += (luma16(&img[i..]) - luma16(&img[i + 3..])).abs();
                    }
                }
                s / (h * (w - 1)) as f64
            };
            let mean = |img: &[u16]| img.as_chunks::<3>().0.iter().map(|p| luma16(p)).sum::<f64>() / (w * h) as f64;
            let (ra, rb) = (roughness(&a), roughness(&b));
            let (ma, mb) = (mean(&a), mean(&b));
            eprintln!("roughness {ra:.3} → {rb:.3} ({:.0}% less), mean luma {ma:.2} → {mb:.2}", (1.0 - rb / ra) * 100.0);
            // The roughness counts the whole frame, texture and edges included,
            // which NR deliberately keeps: since the October overhaul the noise
            // model believes only the quietest blocks, so a detailed frame is
            // no longer over-smoothed. This frame measures ~14% at 0.6 (it was
            // ~29% before the overhaul, when this asked for 20%). The quality
            // of the NR itself is judged against ground truth by
            // `eval_tests::denoise_moves_towards_ground_truth`; this guards
            // that it still clearly acts on a real frame.
            assert!(rb < ra * 0.9, "denoise did not reduce pixel-level noise");
            assert!((mb - ma).abs() < 0.5, "denoise shifted overall tone");
        });
    }
}
