# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build --release          # standard release build (default features, most compatible)
cargo run --release            # run in release mode (debug builds are too slow for GPU/RAW work)
cargo build-fast / run-fast    # release build/run with the fast-jpeg (SIMD) decoder feature
cargo build-compat             # explicit alias for the default-features release build
cargo test                     # run unit tests (see src/raw/tests.rs and inline #[cfg(test)] modules)
cargo test <name>              # run a single test by name/substring
cargo clippy                   # lint; cognitive-complexity-threshold = 30 (clippy.toml)
```

There is no separate frontend build — this is a single native Rust binary (no web tech, no Electron/Tauri).

## Core Tech Stack
- **Language:** Rust (edition 2021)
- **GUI:** `iced` 0.13 (immediate-mode-style, Elm-inspired unidirectional data flow), `iced_aw` for extra widgets
- **GPU:** `wgpu` with custom WGSL shaders
- **Async:** `tokio` for background I/O and CPU-heavy decoding (`spawn_blocking`)
- **RAW decoding:** `rawler` (dnglab fork of rawloader), `tiff`/`kamadak-exif` for DCP/EXIF metadata
- **Database:** `rusqlite` (bundled SQLite)

## Architecture

### Module layout
- `src/main.rs` — entry point / iced application bootstrap
- `src/app/` — application state and update loop, split Elm-style:
  - `state.rs` / `message.rs` — `RawEditor` state struct and `Message` enum
  - `update.rs` + `handlers/{develop,export,library,loading,navigation,window}.rs` — message handlers grouped by feature area
  - `views/{develop,library,cull,layout,modals}.rs` — iced view builders per tab/screen
  - `loader.rs` — async multi-tier image loading orchestration
- `src/gpu/` — the wgpu rendering pipeline (see dual-pass system below)
- `src/raw/` — RAW decoding, DCP color profile parsing, cache-tier JPEG generation, thumbnails
- `src/database/` — SQLite schema, queries, models (the image catalog)
- `src/core/` — cross-cutting types, histogram computation, logging/tracing setup, profiler, user settings
- `src/ui/` — reusable iced widgets (filmstrip, histogram view, icons, palette/styles, profiler graph)
- `src/color.rs` — color space math (matrices, conversions) shared by CPU and GPU paths

### A. The GPU Pipeline (`src/gpu/`)
Dual-pass rendering to avoid wasting GPU cycles:
- **Pass 1 (Debayering)** — runs only on image load or sensor-level data change. Reads raw `R16Uint` Bayer mosaic → outputs `Rgba16Float` intermediate texture.
- **Pass 2 (Color Science)** — runs at 60fps on every slider tweak. Reads the pre-debayered float texture, applies White Balance, Noise Reduction, the DCP Color Pipeline, outputs to screen.
- Shader source lives in `src/gpu/shaders.rs`; pipeline/uniform setup in `src/gpu/pipeline.rs`; uniform struct layout in `src/gpu/params.rs`.

### B. Adobe DCP Color Science (`src/raw/dcp.rs`)
Uses the real Adobe DNG Color Pipeline instead of naive 3x3 sRGB matrices:
1. CPU parses `.dcp` files and performs inverse-temperature dual-illuminant interpolation (blending Illuminant A and D65 by the Kelvin slider).
2. CPU bakes the interpolated `HueSatMap` into a 3D wgpu texture (`TextureDimension::D3`) and `ProfileToneCurve` into a 1D spline texture.
3. WGSL shader path: Camera RGB → XYZ → ProPhoto RGB → 3D LUT (hue/sat twists) → 1D tone curve → monitor sRGB.

### C. Multi-Tier Caching (`src/database/`, `src/raw/processor.rs`, `src/app/loader.rs`)
RAW files are never decoded on the main thread. Three async-generated JPEG tiers plus an in-RAM LRU cache:
- **Tier 1 — Thumbnail (256px):** library grid
- **Tier 2 — Instant Preview (384px):** rapid full-screen scrolling
- **Tier 3 — Working Preview (1280px):** Develop module + histogram generation
- RAM cache is bounded by a user-configurable memory budget; over budget, the LRU evicts the oldest `RawDataResult` structs.
- On-disk cache lives under `~/.cache/raw-editor/{thumb,instant,working}`. Schema columns: `cache_path_thumb`, `cache_path_instant`, `cache_path_working` (the old single `thumbnail_path`/`preview_path` system is retired).

### D. Viewport & Projection Math
- Preview resolutions are multiplied by the window's `scale_factor` for pixel-perfect Retina/4K rendering.
- The vertex shader uses a CPU-computed orthographic projection matrix to letterbox/center images without stretching.
- Non-destructive crop uses normalized `[0.0, 1.0]` UV coords; while `is_cropping == 1` the GPU reveals full sensor data and dims discarded pixels.

## Strict Engineering Constraints (must follow)

1. **Wgpu memory alignment (critical):** any struct sent to a wgpu uniform buffer (e.g. `GpuEditParams`) must be strictly 16-byte aligned. Add explicit padding fields (e.g. `_pad: [f32; 3]`) — misalignment causes GPU memory corruption, not just a panic.
2. **Never block the UI thread:** all disk reads, RAW decodes, EXIF parsing, and cubic spline calculations must run inside `tokio::task::spawn_blocking`. The iced `update` function must return immediately; results flow back in as `Message`s.
3. **Separation of concerns:** keep wgpu logic isolated from iced logic. Iced code should only pass plain Rust primitives/structs/flags down into `SharedContext`/`ImageResources` — no wgpu types leaking into `src/app/`.
4. **Error handling:** use `tracing::info!`/`tracing::error!` for backend operations. Never `.unwrap()` on file I/O or wgpu buffer mapping — propagate the error to UI state or fall back gracefully.
5. **Schema changes:** when changing the SQLite schema, update every SQL query and the corresponding struct in `src/database/models.rs` in the same change; stale DBs must be deleted/migrated (no auto-migration tooling exists yet).

## Design Language
"Premium Creative Pro" meets "Turbo Nerd": deep charcoal/zinc dark mode (`#0A0A0A`–`#111113`), stark white text, electric orange (`#F97316`) accents. "Pure Contact Sheet" aesthetic — images have 0.0 border radius, no intrusive filename overlays. Technical metrics use monospace fonts. Styling lives in `src/ui/styles.rs` / `src/ui/palette.rs`.
