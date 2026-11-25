//! GPU timestamps for the performance overlay, in `profiling` builds only:
//! how long the GPU spends on each part of a develop frame, which the HUD's
//! wall-clock "GPU" figure (submit + wait + readback) cannot tell apart.
//!
//! Six marks bracket a frame, written between commands with
//! `CommandEncoder::write_timestamp` (only `TIMESTAMP_QUERY` is needed, not
//! timestamps inside passes, which tile-based mobile GPUs lack):
//!
//! ```text
//! 0 [overview stages] 1 [overview colour pass] 2 … 3 [display stages] 4 [display colour pass + copy] 5
//! ```
//!
//! Without the feature, or on a device that cannot time, every type here is
//! empty and every call does nothing.

use super::shared::SharedContext;
use crate::core::profiler::Detail;

/// The marks in a frame.
pub const MARKS: u32 = 6;

/// The device features timing needs: `TIMESTAMP_QUERY` when profiling and the
/// adapter has it, else none (a device asked for a feature it lacks fails).
pub fn features(adapter: &wgpu::Adapter) -> wgpu::Features {
    #[cfg(feature = "profiling")]
    return adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
    #[cfg(not(feature = "profiling"))]
    {
        let _ = adapter;
        wgpu::Features::empty()
    }
}

#[cfg(feature = "profiling")]
struct Inner {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    mapped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Nanoseconds per tick.
    period: f32,
}

/// The context's timer. One frame times at a time (`frame`).
#[derive(Default)]
pub struct GpuTimer {
    #[cfg(feature = "profiling")]
    inner: Option<std::sync::Mutex<Inner>>,
}

impl std::fmt::Debug for GpuTimer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GpuTimer")
    }
}

impl GpuTimer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        #[cfg(feature = "profiling")]
        {
            if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
                tracing::info!("profiling: this GPU cannot time passes; GPU pass times stay at zero");
                return Self::default();
            }
            let size = MARKS as u64 * 8;
            let set = device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("Profiler timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: MARKS,
            });
            let resolve = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Profiler timestamp resolve"),
                size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Profiler timestamp read"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            Self {
                inner: Some(std::sync::Mutex::new(Inner {
                    set,
                    resolve,
                    read,
                    mapped: Default::default(),
                    period: queue.get_timestamp_period(),
                })),
            }
        }
        #[cfg(not(feature = "profiling"))]
        {
            let _ = (device, queue);
            Self::default()
        }
    }

    /// Time one frame, if no other frame is being timed.
    pub fn frame(&self) -> FrameTimer<'_> {
        FrameTimer {
            #[cfg(feature = "profiling")]
            inner: self.inner.as_ref().and_then(|m| m.try_lock().ok()),
            _life: std::marker::PhantomData,
        }
    }
}

/// One frame's marks. Untimed: [`FrameTimer::none`].
pub struct FrameTimer<'a> {
    #[cfg(feature = "profiling")]
    inner: Option<std::sync::MutexGuard<'a, Inner>>,
    _life: std::marker::PhantomData<&'a ()>,
}

impl FrameTimer<'_> {
    /// Times nothing.
    #[inline(always)]
    pub fn none() -> Self {
        FrameTimer {
            #[cfg(feature = "profiling")]
            inner: None,
            _life: std::marker::PhantomData,
        }
    }

    /// Write mark `i` into `encoder`, at that point in its commands.
    #[inline(always)]
    pub fn mark(&self, encoder: &mut wgpu::CommandEncoder, i: u32) {
        #[cfg(feature = "profiling")]
        if let Some(inner) = &self.inner {
            encoder.write_timestamp(&inner.set, i);
        }
        #[cfg(not(feature = "profiling"))]
        let _ = (encoder, i);
    }

    /// Write mark `i` now: after everything submitted so far, before anything
    /// submitted next.
    #[inline(always)]
    pub fn mark_now(&self, context: &SharedContext, i: u32) {
        #[cfg(feature = "profiling")]
        if self.inner.is_some() {
            let mut encoder = context.device.create_command_encoder(&Default::default());
            self.mark(&mut encoder, i);
            context.queue.submit(Some(encoder.finish()));
        }
        #[cfg(not(feature = "profiling"))]
        let _ = (context, i);
    }

    /// Queue the marks' copy back. Call after the frame's last submit and
    /// before its `poll(Wait)`, which then covers it.
    #[inline(always)]
    pub fn resolve(&self, context: &SharedContext) {
        #[cfg(feature = "profiling")]
        if let Some(inner) = &self.inner {
            let mut encoder = context.device.create_command_encoder(&Default::default());
            encoder.resolve_query_set(&inner.set, 0..MARKS, &inner.resolve, 0);
            encoder.copy_buffer_to_buffer(&inner.resolve, 0, &inner.read, 0, MARKS as u64 * 8);
            context.queue.submit(Some(encoder.finish()));
            inner.mapped.store(false, std::sync::atomic::Ordering::SeqCst);
            let mapped = inner.mapped.clone();
            inner.read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                mapped.store(r.is_ok(), std::sync::atomic::Ordering::SeqCst);
            });
        }
        #[cfg(not(feature = "profiling"))]
        let _ = context;
    }

    /// After the frame's `poll(Wait)`: the GPU pass times into `detail`.
    #[inline(always)]
    pub fn read(&self, detail: &mut Detail) {
        #[cfg(feature = "profiling")]
        if let Some(inner) = &self.inner {
            if !inner.mapped.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let t: Vec<u64> = {
                let data = inner.read.slice(..).get_mapped_range();
                bytemuck::cast_slice::<u8, u64>(&data).to_vec()
            };
            inner.read.unmap();
            let ms = |a: usize, b: usize| t[b].saturating_sub(t[a]) as f32 * inner.period / 1.0e6;
            use crate::core::profiler::Span;
            detail.add_ms(Span::GpuStages, ms(0, 1) + ms(3, 4));
            detail.add_ms(Span::GpuOverview, ms(1, 2));
            detail.add_ms(Span::GpuDisplay, ms(4, 5));
        }
        #[cfg(not(feature = "profiling"))]
        let _ = detail;
    }
}
