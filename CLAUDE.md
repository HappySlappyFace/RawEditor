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
  - `update.rs` + `handlers/{delete,develop,export,library,loading,masks,navigation,scroll,window}.rs` — message handlers grouped by feature area
  - `views/{develop,library,cull,export,layout,modals}.rs` — iced view builders per tab/screen
  - `loader.rs` — async multi-tier image loading orchestration
- `src/gpu/` — the wgpu rendering pipeline (see dual-pass system below):
  `shaders.rs` (WGSL source), `shared.rs` (`SharedContext` + `ImageResources`:
  pipeline creation, textures, bind groups, uniform uploads), `params.rs`
  (uniform struct layout), `render_functions.rs` (render + readback entry points)
- `src/raw/` — RAW decoding, DCP color profile parsing, cache-tier JPEG generation, thumbnails
- `src/database/` — SQLite schema, queries, models (the image catalog)
- `src/core/` — cross-cutting types, histogram computation, logging/tracing setup, profiler, user settings, and `viewport.rs` (see §D)
- `src/ui/` — reusable iced widgets (filmstrip, histogram view, icons, palette/styles, profiler graph), plus `preview_renderer.rs` + `viewport.wgsl` (the develop viewport shader widget and its crop/mask canvas overlay)
- `src/color.rs` — color space math (matrices, conversions) shared by CPU and GPU paths

### A. The GPU Pipeline (`src/gpu/`)
Dual-pass rendering to avoid wasting GPU cycles:
- **Pass 1 (Debayering)** — runs only on image load or sensor-level data change. Reads raw `R16Uint` Bayer mosaic → outputs `Rgba16Float` intermediate texture.
- **Pass 2 (Color Science)** — re-runs on every slider tweak. Reads the pre-debayered float texture, applies White Balance, Noise Reduction, the DCP Color Pipeline and local masks, and is read back to CPU bytes that the viewport shader widget then draws.
- Shader source lives in `src/gpu/shaders.rs`; pipeline/bind-group/uniform setup in `src/gpu/shared.rs`; uniform struct layout in `src/gpu/params.rs`; the render + readback entry points in `src/gpu/render_functions.rs`.
- `SharedContext` holds two Pass-2 pipelines: `pipeline` (→ `Rgba8Unorm`, display) and `pipeline_16` (→ `Rgba16Float`, 16-bit TIFF export only). A wgpu `RenderPipeline`'s output format is baked in at creation, so the export path needs its own rather than swapping the target's format.
- **Windowed rendering:** Pass 2 renders only `GpuEditParams.view_rect` — the `(u0, v0, du, dv)` slice of the displayed image that is actually on screen (see §D). The vertex shader maps the target across it, so `(0,0,1,1)` is the identity and covers the whole image.
- **Histogram:** a dedicated ~128px whole-image pass (`ImageResources::histogram_dims`) rendered in the *same frame* as the display window and sharing one `poll(Wait)` — see `render_display_and_histogram`. It must stay whole-image, or it would drift as the user pans/zooms. The two passes read one uniform buffer; their differing `view_rect` values are sequenced by `write_view_rect`'s targeted 16-byte `queue.write_buffer` between submits.

### B. Adobe DCP Color Science (`src/raw/dcp.rs`)
Uses the real Adobe DNG Color Pipeline instead of naive 3x3 sRGB matrices:
1. CPU parses `.dcp` files and performs inverse-temperature dual-illuminant interpolation (blending Illuminant A and D65 by the Kelvin slider).
2. CPU bakes the interpolated `HueSatMap` into a 3D wgpu texture (`TextureDimension::D3`) and `ProfileToneCurve` into a 1D spline texture.
3. WGSL shader path: Camera RGB → ProPhoto RGB → highlights/shadows → shoulder → 3D LUT (hue/sat twists) → 1D tone curve (Adobe RGBTone, hue-preserving) → gamut mapping → monitor sRGB.
- **One matrix, not two.** `InterpolatedProfile.camera_to_prophoto` is the DCP ForwardMatrix (camera → XYZ D50) with `color::XYZ_D50_TO_PROPHOTO` already folded in on the CPU, so the shader never traverses XYZ per pixel. `GpuEditParams.forward_matrix_*` is therefore polymorphic on `has_dcp`: camera → ProPhoto with a profile, camera → linear sRGB (`calculate_cam_to_srgb`) without. Both cost exactly one multiply per pixel.
- **Matrix convention:** every `[f32; 9]` in this codebase is **row-major** (the shader rebuilds them with `transpose(mat3x3(...))`); `cgmath::Matrix3` is column-major. Use `color::mat3_mul`, not cgmath, when composing them — and don't copy constants between Rust and WGSL literals without transposing.
- **Highlights/Shadows must run in scene-linear, before the shoulder.** On the DCP path they are applied in ProPhoto immediately after the matrix (`apply_highlights_shadows`), where values still exceed 1.0. Applied later they are a uniform multiply on an already-compressed band — the highlight goes grey and no texture returns. Step 10 is gated `has_dcp == 0` so the fallback path (which has no shoulder until the filmic curve) still gets it there, without double-applying.
- The weighting scalar differs by space and is passed in by the caller: **peak channel** in ProPhoto (its Y coefficients are ~(0.288, 0.712, 0.0001), so a blown blue sky would score near-zero luma), **Rec.709 luma** on the linear-sRGB fallback.

### B2. Selectable Tone Mapping (`src/core/tonemap.rs`)
A DCP bundles *colour* rendering (ForwardMatrix + HueSatMap — sensor
characterization, always in force) and *tone* rendering (ProfileToneCurve —
an opinion about contrast). Only the second is swappable, and
`EditParams::tone_mapper` swaps it: `Camera` (default), `Filmic`, `Reinhard`,
`Hable`, `AcesFitted` (Narkowicz), `Gt` (Uchimura, shape exposed via `GtParams`).

- **Tone rendering is ONE selected stage.** Three things can curve the image —
  the ProfileToneCurve slot, the Step 15 filmic block, and the pre-HueSatMap
  knee — and the codebase has been bitten by double-tone-mapping more than
  once. Any change here must keep exactly one of them active per path.
- **Operators bake into the existing 1D LUT on the CPU**, so adding one costs a
  texture upload and *zero* per-pixel work. Only `tone_mapper: u32` reaches the
  GPU (in a former pad slot — `GpuEditParams` is still 784 bytes), and the
  shader reads it solely to decide Camera-vs-not. Consequence: operators must
  be scalar functions of one channel. Anything applying matrices around the
  curve (Hill's ACES fit, AgX) cannot be baked and is deliberately absent.
- **The LUT domain is log-encoded over `[0, TONE_LUT_MAX]`, not `[0,1]`** —
  `tone_lut_encode` on both sides, mirrored constants guarded by
  `shader_tone_lut_constants_match_rust`. Operators are *defined* as the
  scene-linear→display mapping, so they must receive uncompressed values.
- **The 0.7 Reinhard knee runs for `Camera` only.** It exists because the
  ProfileToneCurve is defined on [0,1], and being per-channel is deliberate
  (it samples the profile's top value slices). Kneeing before any other
  operator would tone-map a tone-mapped signal — the same failure that made
  Highlights return grey instead of texture.
- **Output space:** everything in `core::tonemap` returns display-**linear**.
  The ProfileToneCurve is the sole exception (linear in → gamma-encoded out),
  which `bake_tone_curve` linearises back; getting this wrong is a ~1.3 EV
  overbright.
- Because they live on the CPU, the operators are ordinary unit-tested Rust.
  The GPU-side wiring is covered by the ignored
  `tone_mapper_selection_changes_the_render_without_breaking_it`, which exists
  because a missed LUT upload renders **black** with no panic and no log.

### C. Multi-Tier Caching (`src/database/`, `src/raw/processor.rs`, `src/app/loader.rs`)
RAW files are never decoded on the main thread. Three async-generated JPEG tiers plus an in-RAM LRU cache:
- **Tier 1 — Thumbnail (256px):** library grid
- **Tier 2 — Instant Preview (384px):** rapid full-screen scrolling
- **Tier 3 — Working Preview (1280px):** Develop module + histogram generation
- RAM cache is bounded by a user-configurable memory budget; over budget, the LRU evicts the oldest `RawDataResult` structs.
- On-disk cache lives under `~/.cache/raw-editor/{thumb,instant,working}`. Schema columns: `cache_path_thumb`, `cache_path_instant`, `cache_path_working` (the old single `thumbnail_path`/`preview_path` system is retired).
- `app/loader.rs` drives the head of each queue as *separate* iced subscriptions so decodes run concurrently — bounded by `MAX_CONCURRENT_PREVIEW_LOADS` (4) and `MAX_CONCURRENT_RAW_LOADS` (2, lower because each RAW can hold ~100 MB).
- **Every** `rusqlite::Connection` must be passed through `database::library::apply_pragmas` (WAL + `synchronous=NORMAL`) — including the ones opened inside `spawn_blocking` workers. Pragmas are per-connection; without them rusqlite defaults to an fsync per transaction.

### D. Viewport & Projection Math
`src/core/viewport.rs` is the single source of truth for "where the displayed
image is on screen". The display shader, the crop/mask canvas overlay, and the
zoom/pan/crop input math all consume the same `image_rect` — they used to
compute it independently and disagreed at any zoom != 1.0.

- `fitted_size` / `image_rect` — letterbox fit, grown by `zoom` about the viewport centre, shifted by `pan_offset` in **fitted-size fractions with no zoom term**, so `Δoffset = Δcursor_px / fitted_px` gives 1:1 cursor tracking at any zoom.
- Always take dims from `RawEditor::image_display_dims()` (EXIF-oriented, whole image) for on-screen geometry — never from the render buffer, whose aspect now tracks the viewport rather than the image. `buffer_dims()` is for sizing textures and nothing else.
- `visible_view_rect` computes the window Pass 2 renders, grown by `RENDER_OVERSCAN` (1.25) so panning is free until the user drags past the margin; `view_rect_contains` answers "is a re-render due?". Panning does not re-render — the viewport shader just re-projects the existing texture.
- State keeps `rendered_view_rect` (what the on-screen bytes actually cover, published **with** the bytes in `handle_render_finished`) separate from `requested_view_rect` (what the in-flight render covers). Publishing the former early draws the current texture at the incoming window's placement, which reads as the image jumping for a frame.
- `viewport.wgsl`'s projection matrix carries the entire placement (fit + zoom + pan); its fragment shader is a plain `textureSample`. `GpuEditParams.zoom`/`pan_x`/`pan_y` are vestigial and intentionally unread.
- Preview resolutions are multiplied by the window's `scale_factor` for pixel-perfect Retina/4K rendering. Render targets are additionally clamped to 1:1 with the source — past that there is no more detail to resolve. Zooming past 1.5 triggers `trigger_full_res_upgrade`, rebuilding `ImageResources` from cached RAW without subsampling.
- Non-destructive crop uses normalized `[0.0, 1.0]` UV coords; while `is_cropping == 1` the GPU reveals full sensor data and dims discarded pixels (and windowing is skipped, since zoom/pan are disabled there).
- **UV is not isotropic.** Mask geometry lives in full-image UV, but what's displayed is the crop sub-rect stretched over a full-image-aspect target, so the two axes carry different pixels-per-unit. Anything angular (rotated radial masks) must un-distort by `core::viewport::mask_uv_aspect`, rotate, then redistort — rotating in raw UV shears. Rotation angles are cyclic: use `wrap_degrees`, never a clamp, or a drag sticks at ±180°.

## Strict Engineering Constraints (must follow)

1. **Wgpu memory alignment (critical):** any struct sent to a wgpu uniform buffer (e.g. `GpuEditParams`) must be strictly 16-byte aligned. Add explicit padding fields (e.g. `_pad: [f32; 3]`) — misalignment causes GPU memory corruption, not just a panic. Field *offsets* are load-bearing too: `write_view_rect` patches `view_rect` by `offset_of!`, and the WGSL `EditParams` struct is duplicated in both the color and debayer shaders (they share one uniform buffer and must stay byte-identical). The size and offset assertions in `gpu/params.rs`'s tests are the guard — keep them passing rather than adjusting them to match a drift.
2. **Never block the UI thread:** all disk reads, RAW decodes, EXIF parsing, and cubic spline calculations must run inside `tokio::task::spawn_blocking`. The iced `update` function must return immediately; results flow back in as `Message`s. This includes SQLite: never write from a per-mouse-move path (slider or handle drags). Call `mark_edits_dirty()` there and let `flush_edits()` persist at the commit points — `CommitEdit`, drag release, image switch, tab switch, window close. Missing a flush point loses the last gesture's edits.
3. **Rendering params that aren't the user's edits:** never assign to
   `current_edit_params` to preview something (before/after peek, a proposed
   look, a thumbnail variant). `update_pipeline` marks edits dirty, and any
   flush point that fires meanwhile — CommitEdit, drag release, image switch,
   tab switch, window close — persists those temporary values to SQLite over
   the user's real work. Thread the params through
   `develop::push_uniforms_and_render(editor, &params)` instead, and note that
   `resolve_wb_and_dcp` takes `params` for the same reason: reading the
   editor's own params there would give a hybrid of two states (default
   exposure with slider-derived WB and tone curve).
4. **One key, one message:** `subscription()` returns a single
   `Option<Message>` per event and has no access to editor state, so two
   features cannot bind the same key. Shared keys route at the handler:
   `Enter` → `Message::ModalConfirm` → dispatch on `active_modal`
   (`handlers::window::handle_modal_confirm`). Modal-scoped shortcuts bind
   globally and guard with `if editor.active_modal != Modal::X { return
   Task::none(); }` as the handler's first line.
5. **Separation of concerns:** keep wgpu logic isolated from iced logic. Iced code should only pass plain Rust primitives/structs/flags down into `SharedContext`/`ImageResources` — no wgpu types leaking into `src/app/`.
6. **Error handling:** use `tracing::info!`/`tracing::error!` for backend operations. Never `.unwrap()` on file I/O or wgpu buffer mapping — propagate the error to UI state or fall back gracefully.
7. **Schema changes:** when changing the SQLite schema, update every SQL query and the corresponding struct in `src/database/models.rs` in the same change; stale DBs must be deleted/migrated (no auto-migration tooling exists yet).

## Design Language
"Premium Creative Pro" meets "Turbo Nerd": deep charcoal/zinc dark mode (`#0A0A0A`–`#111113`), stark white text, electric orange (`#F97316`) accents. "Pure Contact Sheet" aesthetic — images have 0.0 border radius, no intrusive filename overlays. Technical metrics use monospace fonts. Styling lives in `src/ui/styles.rs` / `src/ui/palette.rs`.
