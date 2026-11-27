//! RawEditor on Android, native half. The Kotlin UI (`android/`) owns the
//! controls and a `SurfaceView`; this library owns everything else — the
//! session, the GPU and that surface — on one render thread
//! (`engine`), behind the JNI functions in `api`.
//!
//! Only [`controls`] builds off Android, so its tests run with the rest of
//! the workspace.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

mod controls;

#[cfg(target_os = "android")]
mod api;
#[cfg(target_os = "android")]
mod engine;
#[cfg(target_os = "android")]
mod logcat;
