# RawEditor - Master AI Context & Architecture Guide

## 1. Project Overview & Philosophy
**RawEditor** is a high-performance, native, GPU-accelerated RAW image editor and culling tool built in Rust. 
* **The Prime Directive:** Performance and visual fidelity. The application must maintain 60fps during UI interactions and slider adjustments. 
* **Design Language:** "Premium Creative Pro" meets "Turbo Nerd". Deep charcoal/zinc dark mode (`#0A0A0A` to `#111113`), stark white text, and electric orange (`#F97316`) accents. We use a "Pure Contact Sheet" aesthetic—images have 0.0 border radius and no intrusive filename text. Technical metrics are displayed in `Monospace` fonts.
* **No Bloat:** No web technologies (Electron/Tauri). Fully native compiled binaries.

## 2. Core Tech Stack
* **Language:** Rust
* **GUI Framework:** `iced` (Immediate mode, unidirectional data flow)
* **GPU Compute & Rendering:** `wgpu` (Custom WGSL shaders)
* **Async Runtime:** `tokio` (For non-blocking I/O and CPU-heavy decoding)
* **RAW Decoding:** `rawler` (and `tiff` for DCP parsing)

## 3. System Architecture

### A. The GPU Pipeline (`src/gpu/`)
The engine uses a highly optimized dual-pass rendering system to save GPU cycles and prevent framerate drops:
* **Pass 1 (Debayering):** Runs ONLY when the image loads or sensor-level data changes. Reads the raw `R16Uint` Bayer mosaic and outputs an `Rgba16Float` intermediate texture.
* **Pass 2 (Color Science):** Runs at 60fps on every slider tweak. Reads the pre-debayered float texture, applies White Balance, Noise Reduction, the DCP Color Pipeline, and outputs to the screen.

### B. Adobe DCP Color Science (`src/raw/dcp.rs`)
RawEditor uses the industry-standard Adobe DNG Color Pipeline. We do *not* use naive 3x3 sRGB matrices.
1. **CPU Interpolation:** The CPU parses `.dcp` files and performs inverse-temperature dual-illuminant interpolation (blending Illuminant A and D65 based on the Kelvin slider).
2. **GPU Textures:** The CPU bakes the interpolated `HueSatMap` into a 3D Wgpu Texture (`wgpu::TextureDimension::D3`) and the `ProfileToneCurve` into a 1D spline texture.
3. **WGSL Execution:** The shader converts Camera RGB -> XYZ -> ProPhoto RGB, applies the 3D LUT to fix color twists, applies the 1D tone curve, and translates to monitor sRGB.

### C. Multi-Tier Caching System (`src/database/` & `src/app/`)
To enable instant culling, RAW files are never decoded on the main thread. We use a 3-tier asynchronous generation pipeline and an LRU RAM Cache:
* **Tier 1 (Thumbnail):** 256px for the library grid.
* **Tier 2 (Instant Preview):** 384px for rapid full-screen scrolling.
* **Tier 3 (Working Preview):** 1280px for the Develop module and histogram generation.
* Memory usage is strictly controlled by a user-configurable budget (e.g., 1024 MB). If the budget is exceeded, the LRU cache dynamically evicts the oldest `RawDataResult` structs.

### D. The Viewport & Projection Math
The Wgpu canvas perfectly aligns with the Iced UI logical bounds.
* **DPI Awareness:** Preview resolutions are multiplied by the window's `scale_factor` to ensure pixel-perfect rendering on Retina/4K displays.
* **Projection Matrix:** The Vertex Shader uses an Orthographic Projection Matrix calculated on the CPU to automatically letterbox and center images without stretching.
* **Non-Destructive Cropping:** The crop tool utilizes normalized `[0.0, 1.0]` UV coordinates. When the user is actively cropping (`is_cropping == 1`), the GPU reveals the full sensor data and visually dims discarded pixels.

## 4. Strict Engineering Constraints (MUST READ)
1. **Wgpu Memory Alignment (CRITICAL):** When modifying `GpuEditParams` or any struct sent to the Wgpu uniform buffer, you **MUST** ensure strict 16-byte alignment. Use padding fields (e.g., `_pad: [f32; 3]`) to prevent GPU memory corruption.
2. **Never Block the UI:** All disk reads, RAW decodes, EXIF parsing, and cubic spline calculations must be wrapped in `tokio::task::spawn_blocking`. The Iced `update` function must return immediately.
3. **Separation of Concerns:** Keep Wgpu logic entirely isolated from Iced logic. Iced should only pass standard Rust primitives (structs/flags) down to the `SharedContext` and `ImageResources`.
4. **Error Handling:** Use `tracing::info!` and `tracing::error!` for all backend operations. Never use `.unwrap()` on file I/O or Wgpu buffer mapping; gracefully fall back or propagate the error to the UI state.
