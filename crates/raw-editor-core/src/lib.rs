//! RawEditor's engine: the GPU pipeline, RAW decoding, colour science, the
//! catalogue and the AI features. No UI toolkit: the desktop app (`iced`)
//! drives it, and nothing here may depend on that.

// Tests build params up field by field (`let mut p = default(); p.x = …`),
// which reads better than one giant literal. Production code is still held to
// the lint by the non-test build.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

pub mod color;
pub mod core;
pub mod database;
pub mod gpu;
pub mod ml;
pub mod raw;
