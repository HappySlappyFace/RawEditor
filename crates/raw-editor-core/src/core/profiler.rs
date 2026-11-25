//! The performance overlay's data (Window › Performance HUD, F3).
//!
//! Every build records the four figures of [`ProfilerFrame`]. The breakdown
//! behind them, [`Detail`], exists only with the `profiling` feature
//! (`cargo run-profile`): without it `Detail` and [`Timer`] are zero-sized and
//! every method is an empty inline function, so the measuring code compiles
//! away and call sites need no `#[cfg]` of their own.

use std::collections::VecDeque;

/// True in `profiling` builds, which record [`Detail`].
pub const ENABLED: bool = cfg!(feature = "profiling");

#[derive(Debug, Clone, Copy, Default)]
pub struct ProfilerFrame {
    pub update_ms: f32,
    pub upload_ms: f32,
    pub render_ms: f32,
    pub total_ms: f32,
    /// The breakdown (`profiling` builds only; empty otherwise).
    pub detail: Detail,
}

#[derive(Debug)]
pub struct Profiler {
    pub history: VecDeque<ProfilerFrame>,
    pub capacity: usize,
    /// Collected for the next render before it starts: the ticks merged into
    /// it while the previous one was in flight.
    pub next: Detail,
}

impl Default for Profiler {
    fn default() -> Self {
        Self {
            history: VecDeque::with_capacity(200),
            capacity: 200,
            next: Detail::default(),
        }
    }
}

impl Profiler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_frame(&mut self, frame: ProfilerFrame) {
        if self.history.len() >= self.capacity {
            self.history.pop_front();
        }
        self.history.push_back(frame);
    }

    /// `(last, average, max)` of a span over the history.
    pub fn span_stats(&self, span: Span) -> (f32, f32, f32) {
        let last = self.history.back().map_or(0.0, |f| f.detail.ms(span));
        let n = self.history.len().max(1) as f32;
        let sum: f32 = self.history.iter().map(|f| f.detail.ms(span)).sum();
        let max = self.history.iter().map(|f| f.detail.ms(span)).fold(0.0, f32::max);
        (last, sum / n, max)
    }

    /// Total of a count over the history.
    pub fn count_total(&self, count: Count) -> u32 {
        self.history.iter().map(|f| f.detail.count(count)).sum()
    }
}

/// Where a develop frame's time went. In order of the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// `Session::render`, up to handing the work off (the HUD's "CPU").
    Update,
    /// Bringing the cached stages up to date: curves LUT, brush layers, AI
    /// mask upload, noise reduction, Texture/Clarity. Encoding and submitting
    /// only; their GPU time is [`Span::GpuStages`].
    StageCurves,
    StageBrush,
    StageAi,
    StageDenoise,
    StageLocalContrast,
    /// The output texture and the readback buffer.
    Allocate,
    /// Recording the colour passes and the copies back.
    Record,
    /// `queue.submit` calls (they can block on backpressure).
    Submit,
    /// Waiting for the GPU and the readback (`poll(Wait)`).
    Wait,
    /// Copying the readbacks out, without the row padding.
    Unpad,
    Histogram,
    /// GPU timestamps: the stage passes, the overview colour pass, and the
    /// display colour pass with its copy. Zero if the device cannot time.
    GpuStages,
    GpuOverview,
    GpuDisplay,
    /// From `Session::render` to the finished frame reaching the session.
    Latency,
}

impl Span {
    pub const ALL: [Span; 16] = [
        Span::Update,
        Span::StageCurves,
        Span::StageBrush,
        Span::StageAi,
        Span::StageDenoise,
        Span::StageLocalContrast,
        Span::Allocate,
        Span::Record,
        Span::Submit,
        Span::Wait,
        Span::Unpad,
        Span::Histogram,
        Span::GpuStages,
        Span::GpuOverview,
        Span::GpuDisplay,
        Span::Latency,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Span::Update => "Session update",
            Span::StageCurves => "Curves LUT",
            Span::StageBrush => "Brush layers",
            Span::StageAi => "AI masks",
            Span::StageDenoise => "Noise reduction",
            Span::StageLocalContrast => "Texture/Clarity",
            Span::Allocate => "Allocate",
            Span::Record => "Record",
            Span::Submit => "Submit",
            Span::Wait => "GPU wait",
            Span::Unpad => "Read back",
            Span::Histogram => "Histogram",
            Span::GpuStages => "Stages",
            Span::GpuOverview => "Overview pass",
            Span::GpuDisplay => "Display pass",
            Span::Latency => "Latency",
        }
    }
}

/// Things counted per frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// Slider ticks merged into this frame while the last one rendered.
    Merged,
    /// Cached stages that re-ran.
    DenoiseRuns,
    LocalContrastRuns,
    BrushRuns,
    AiUploads,
    CurveBakes,
    /// Readback buffers taken from the pool, and allocated new.
    ReadbackReused,
    ReadbackAllocated,
    /// 1 when the display rendered from the full-resolution image.
    FullRes,
}

impl Count {
    pub const ALL: [Count; 9] = [
        Count::Merged,
        Count::DenoiseRuns,
        Count::LocalContrastRuns,
        Count::BrushRuns,
        Count::AiUploads,
        Count::CurveBakes,
        Count::ReadbackReused,
        Count::ReadbackAllocated,
        Count::FullRes,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Count::Merged => "Ticks merged",
            Count::DenoiseRuns => "NR re-runs",
            Count::LocalContrastRuns => "Texture/Clarity re-runs",
            Count::BrushRuns => "Brush redraws",
            Count::AiUploads => "AI mask uploads",
            Count::CurveBakes => "Curve bakes",
            Count::ReadbackReused => "Readbacks reused",
            Count::ReadbackAllocated => "Readbacks allocated",
            Count::FullRes => "Full-res frames",
        }
    }
}

/// One frame's breakdown. Zero-sized without the `profiling` feature.
#[derive(Debug, Clone, Copy, Default)]
pub struct Detail {
    #[cfg(feature = "profiling")]
    ms: [f32; Span::ALL.len()],
    #[cfg(feature = "profiling")]
    counts: [u32; Count::ALL.len()],
    #[cfg(feature = "profiling")]
    started: Option<std::time::Instant>,
}

impl Detail {
    /// Mark the frame's start, for [`Span::Latency`].
    #[inline(always)]
    pub fn start(&mut self) {
        #[cfg(feature = "profiling")]
        {
            self.started = Some(std::time::Instant::now());
        }
    }

    /// Set [`Span::Latency`] from [`Detail::start`].
    #[inline(always)]
    pub fn finish(&mut self) {
        #[cfg(feature = "profiling")]
        if let Some(t) = self.started {
            self.ms[Span::Latency as usize] = t.elapsed().as_secs_f32() * 1000.0;
        }
    }

    /// Add the time since `timer` started to `span`.
    #[inline(always)]
    pub fn stop(&mut self, span: Span, timer: Timer) {
        #[cfg(feature = "profiling")]
        {
            self.ms[span as usize] += timer.0.elapsed().as_secs_f32() * 1000.0;
        }
        #[cfg(not(feature = "profiling"))]
        let _ = (span, timer);
    }

    #[inline(always)]
    pub fn add_ms(&mut self, span: Span, ms: f32) {
        #[cfg(feature = "profiling")]
        {
            self.ms[span as usize] += ms;
        }
        #[cfg(not(feature = "profiling"))]
        let _ = (span, ms);
    }

    #[inline(always)]
    pub fn bump(&mut self, count: Count) {
        #[cfg(feature = "profiling")]
        {
            self.counts[count as usize] += 1;
        }
        #[cfg(not(feature = "profiling"))]
        let _ = count;
    }

    /// The time recorded for `span` (0 without `profiling`).
    #[inline(always)]
    pub fn ms(&self, span: Span) -> f32 {
        #[cfg(feature = "profiling")]
        return self.ms[span as usize];
        #[cfg(not(feature = "profiling"))]
        {
            let _ = span;
            0.0
        }
    }

    /// The count recorded for `count` (0 without `profiling`).
    #[inline(always)]
    pub fn count(&self, count: Count) -> u32 {
        #[cfg(feature = "profiling")]
        return self.counts[count as usize];
        #[cfg(not(feature = "profiling"))]
        {
            let _ = count;
            0
        }
    }
}

/// A running stopwatch for [`Detail::stop`]. Zero-sized, and never reads the
/// clock, without the `profiling` feature.
#[derive(Debug, Clone, Copy)]
pub struct Timer(#[cfg(feature = "profiling")] std::time::Instant);

impl Timer {
    #[cfg(feature = "profiling")]
    #[inline(always)]
    pub fn start() -> Self {
        Timer(std::time::Instant::now())
    }

    #[cfg(not(feature = "profiling"))]
    #[inline(always)]
    pub fn start() -> Self {
        Timer()
    }

    #[cfg(feature = "profiling")]
    fn elapsed_ms(self) -> f32 {
        self.0.elapsed().as_secs_f32() * 1000.0
    }
}

/// The front end's own share of the work (`profiling` builds): handling a
/// message and building the view, each as `(last, running average)`. Cells,
/// because a view is built from `&self`.
#[derive(Debug, Default)]
pub struct UiTimes {
    #[cfg(feature = "profiling")]
    update: std::cell::Cell<(f32, f32)>,
    #[cfg(feature = "profiling")]
    view: std::cell::Cell<(f32, f32)>,
}

impl UiTimes {
    #[inline(always)]
    pub fn update_done(&self, timer: Timer) {
        #[cfg(feature = "profiling")]
        Self::record(&self.update, timer.elapsed_ms());
        #[cfg(not(feature = "profiling"))]
        let _ = timer;
    }

    #[inline(always)]
    pub fn view_done(&self, timer: Timer) {
        #[cfg(feature = "profiling")]
        Self::record(&self.view, timer.elapsed_ms());
        #[cfg(not(feature = "profiling"))]
        let _ = timer;
    }

    /// `(last, average)` ms of handling one message.
    pub fn update(&self) -> (f32, f32) {
        #[cfg(feature = "profiling")]
        return self.update.get();
        #[cfg(not(feature = "profiling"))]
        (0.0, 0.0)
    }

    /// `(last, average)` ms of building the view.
    pub fn view(&self) -> (f32, f32) {
        #[cfg(feature = "profiling")]
        return self.view.get();
        #[cfg(not(feature = "profiling"))]
        (0.0, 0.0)
    }

    /// An exponential average over roughly the last 50 samples.
    #[cfg(feature = "profiling")]
    fn record(cell: &std::cell::Cell<(f32, f32)>, ms: f32) {
        let (_, avg) = cell.get();
        cell.set((ms, if avg == 0.0 { ms } else { avg + (ms - avg) * 0.02 }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The promise that makes the call sites free in normal builds.
    #[cfg(not(feature = "profiling"))]
    #[test]
    fn detail_and_timer_cost_nothing_without_profiling() {
        assert_eq!(std::mem::size_of::<Detail>(), 0);
        assert_eq!(std::mem::size_of::<Timer>(), 0);
        assert_eq!(std::mem::size_of::<UiTimes>(), 0);
        const { assert!(!ENABLED) };
    }

    #[cfg(feature = "profiling")]
    #[test]
    fn spans_and_counts_accumulate_and_average() {
        let mut p = Profiler::new();
        for ms in [1.0, 3.0] {
            let mut d = Detail::default();
            d.add_ms(Span::Wait, ms);
            d.add_ms(Span::Wait, 1.0);
            d.bump(Count::DenoiseRuns);
            p.push_frame(ProfilerFrame { detail: d, ..Default::default() });
        }
        assert_eq!(p.span_stats(Span::Wait), (4.0, 3.0, 4.0));
        assert_eq!(p.count_total(Count::DenoiseRuns), 2);
        assert_eq!(p.span_stats(Span::Unpad), (0.0, 0.0, 0.0));
    }

    /// The tables index by discriminant: `ALL` must list them in order.
    #[test]
    fn the_lists_follow_the_discriminants() {
        for (i, s) in Span::ALL.iter().enumerate() {
            assert_eq!(*s as usize, i);
        }
        for (i, c) in Count::ALL.iter().enumerate() {
            assert_eq!(*c as usize, i);
        }
    }
}
