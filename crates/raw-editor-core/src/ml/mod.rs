//! On-demand machine-learning features: AI masks (SAM 2.1) and AI denoise
//! (PMRID), run on the CPU through ONNX Runtime.
//!
//! **Nothing here costs anything until the user asks for it.** ONNX Runtime
//! initialises on the first session, sessions are built when a tool opens and
//! dropped when it closes, and no inference ever runs in the background. The
//! GPU pipeline never sees a model: an AI mask reaches it as a texture layer
//! (`gpu::ai_mask`), AI denoise as a different mosaic (`raw::loader::
//! effective_mosaic`).
//!
//! No iced or wgpu types in this module (constraint 6): plain data in and out.

pub mod denoise;
pub mod masks;
pub mod models;
pub mod runtime;
pub mod segment;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Shared progress + cancel flag for a long job, read by the UI on a timer.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
}

impl Progress {
    pub fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
    }

    pub fn add(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }

    /// `0.0..=1.0`, or 0 before the total is known.
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f64 / total as f64).clamp(0.0, 1.0) as f32
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// The error every cancelled job returns, so callers can tell it apart.
pub const CANCELLED: &str = "cancelled";
