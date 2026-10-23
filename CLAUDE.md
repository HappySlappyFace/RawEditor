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
cargo clippy --all-targets -- -D warnings   # lint; CI enforces this exactly, so keep it at zero
```

There is no separate frontend build — this is a single native Rust binary (no web tech, no Electron/Tauri).

## Core Tech Stack
- **Language:** Rust (edition 2021)
- **GUI:** `iced` 0.13 (immediate-mode-style, Elm-inspired unidirectional data flow), `iced_aw` for extra widgets
- **GPU:** `wgpu` with custom WGSL shaders
- **Async:** `tokio` for background I/O and CPU-heavy decoding (`spawn_blocking`)
- **RAW decoding:** `rawler` (dnglab fork of rawloader), `tiff`/`kamadak-exif` for DCP/EXIF metadata
- **Database:** `rusqlite` (bundled SQLite)
- **Network:** `ureq` (blocking, rustls — no OpenSSL) + `bzip2-rs`/`tar`, used ONLY when the user clicks to download the lensfun database (`core::lensfun`) or RawTherapee's open-source camera profiles (`core::camera_profiles`: GitHub's contents API + `raw.githubusercontent.com`). The app makes no other network requests; keep it that way unless the user explicitly wants otherwise.

## Architecture

### Module layout
- `src/main.rs` — entry point / iced application bootstrap
- `src/app/` — application state and update loop, split Elm-style:
  - `state.rs` / `message.rs` — `RawEditor` state struct and `Message` enum
  - `update.rs` + `handlers/{camera_profiles,delete,develop,export,lens,lensfun,library,loading,loupe,masks,navigation,proxy,scroll,window}.rs` — message handlers grouped by feature area
  - `views/{develop,library,cull,export,layout,modals,preferences}.rs` — iced view builders per tab/screen
  - `loader.rs` — async multi-tier image loading orchestration
- `src/gpu/` — the wgpu rendering pipeline (see dual-pass system below):
  `demosaic.rs` (Pass 1, RCD), `denoise.rs` (cached NR pre-pass), `loupe.rs` (1:1 detail loupe, §G), `local_contrast.rs`
  (cached Texture/Clarity band pre-pass), `brush.rs` (mask stroke rasteriser), `shaders.rs` (Pass 2 WGSL), `shared.rs` (`SharedContext` + `ImageResources`:
  pipeline creation, textures, bind groups, uniform uploads), `params.rs`
  (uniform struct layout), `render_functions.rs` (render + readback entry points),
  `eval_tests.rs` (ignored GPU quality tests against synthetic ground truth — see §G)
- `src/raw/` — RAW decoding (`loader`), DCP profiles (`dcp`), embedded-JPEG extraction + cache tiers (`jpeg`, `processor`), EXIF carried into exports (`exif`). kamadak-exif only opens standard TIFF containers — RW2 (`IIU`) and ORF (`IIRO`) come back empty — so the loader fills every gap from rawler's own metadata parse (`ExifInfo::fill_from`), and thumbnail orientation falls back the same way; without it those files had no focal length (so no lens profile), no exposure info and no rotation. `RAW_EXTENSIONS` is the importable set — Bayer formats only, since the debayer pass assumes a 2×2 CFA (the loader rejects anything else with a reason)
- `src/database/` — SQLite schema, queries, models (the image catalog)
- `src/core/` — cross-cutting types, histogram computation, logging/tracing setup, profiler, user settings, `paths` (the data and cache folders — every on-disk path derives from it), `viewport.rs` (see §D), and the CPU halves of the editing features: `noise` (per-image noise model), `haze` (dehaze airlight + transmission map), `highlights` (clipped-highlight colour correction), `curves` (point curves → LUT), `lens` (lensfun DB, lens/defringe params → shader block), `lensfun` (downloading/updating that DB), `camera_profiles` (installed DCPs, the per-image choice, RawTherapee downloads — see §B), `brush` (mask strokes) — see §G — and, Linux only, `desktop` (the desktop entry that gives a Wayland window its taskbar icon: Wayland windows carry none, so the app keeps `~/.local/share/applications/raw-editor.desktop` pointing at the running binary; the window's app id must stay equal to its file name)
- `src/ui/` — reusable iced widgets (filmstrip, histogram view, icons, palette/styles, profiler graph), plus `preview_renderer.rs` + `viewport.wgsl` (the develop viewport shader widget and its crop/mask canvas overlay), and `motion.rs` + `widgets/` (animation — see §H)
- `src/color.rs` — color space math (matrices, conversions) shared by CPU and GPU paths

### A. The GPU Pipeline (`src/gpu/`)
Dual-pass rendering to avoid wasting GPU cycles:
- **Pass 1 (Demosaic, `demosaic.rs`)** — RCD as four fragment passes into transient textures, run once per image load. Reads raw `R16Uint` Bayer mosaic → outputs camera-native `Rgba16Float`. Demosaics in as-shot-WB space (ratio methods assume neutral = equal channels) and divides it back out, so WB sliders never re-run it. It has its own small uniform (`GpuDemosaicParams`), not `GpuEditParams`. Every mosaic sample is capped at its colour's ceiling (sensors can read past the metadata white level — a D3300 piles clipped sites up at 4095 against a white of 3880).
- **Cached pre-pass stages** — denoise, the Texture/Clarity bands (built from the denoise output, so after it), the point-curve LUT and the brush-mask layers. `ImageResources::prepare_stages` (called by EVERY render entry point) re-runs a stage only when its own settings changed, keyed off the params last pushed by `update_uniforms`/`new`; export and proxies get them for free. A new expensive stage belongs here, not in Pass 2.
- **Pass 2 (Color Science)** — re-runs on every slider tweak. Reads the demosaic (or the denoise output), applies lens correction, White Balance, highlight reconstruction, dehaze, the Texture/Clarity gain, the DCP Color Pipeline, defringe, sharpening and local masks, and is read back to CPU bytes that the viewport shader widget then draws.
- Shader source lives in `src/gpu/shaders.rs`; pipeline/bind-group/uniform setup in `src/gpu/shared.rs`; uniform struct layout in `src/gpu/params.rs`; the render + readback entry points in `src/gpu/render_functions.rs`.
- `SharedContext` holds two Pass-2 pipelines: `pipeline` (→ `Rgba8Unorm`, display) and `pipeline_16` (→ `Rgba16Float`, 16-bit TIFF export only). A wgpu `RenderPipeline`'s output format is baked in at creation, so the export path needs its own rather than swapping the target's format.
- **Windowed rendering:** Pass 2 renders only `GpuEditParams.view_rect` — the `(u0, v0, du, dv)` slice of the displayed image that is actually on screen (see §D). The vertex shader maps the target across it, so `(0,0,1,1)` is the identity and covers the whole image.
- **Device limits come from the adapter** (texture dimension, buffer size), not `Limits::default()`, whose 8192px / 256 MB floor fails full-res work on ≥45 MP sensors. Size-check before allocating (`SharedContext::check_texture_size` / `check_buffer_size`) and return an error; the device's uncaptured-error handler only logs, so an unchecked oversize no longer panics — it renders wrong instead.
- **Overview pass (histogram + edited proxy):** a small whole-image pass (`core::viewport::overview_dims`, 384px long edge, crop-aware) rendered in the *same frame* as the display window and sharing one `poll(Wait)` — see `render_display_and_overview`. It feeds the histogram and is kept as the image's edited proxy (§F). It must stay whole-image, or it would drift as the user pans/zooms. The two passes read one uniform buffer; their differing `view_rect` values are sequenced by `write_view_rect`'s targeted 16-byte `queue.write_buffer` between submits.

### B. Adobe DCP Color Science (`src/raw/dcp.rs`)
Uses the real Adobe DNG Color Pipeline instead of naive 3x3 sRGB matrices:
1. CPU parses `.dcp` files and performs inverse-temperature dual-illuminant interpolation (blending Illuminant A and D65 by the Kelvin slider).
2. CPU bakes the interpolated `HueSatMap` into a 3D wgpu texture (`TextureDimension::D3`) and `ProfileToneCurve` into a 1D spline texture.
3. WGSL shader path: Camera RGB → ProPhoto RGB → highlights/shadows → shoulder → 3D LUT (hue/sat twists) → 1D tone curve (Adobe RGBTone, hue-preserving) → gamut mapping → monitor sRGB.
- **Which profile** (`core::camera_profiles`): the user's own `.dcp` files in `paths::profiles_dir()`, plus RawTherapee's (GPL / public domain), downloaded per camera on request into `profiles/downloaded/`. An image's choice is `EditParams::camera_profile` (`Auto` / `Builtin` / a file name — fixed-capacity so `EditParams` stays `Copy`); `Auto` takes the per-camera default (`AppSettings::camera_profile_defaults`, keyed by a profile's camera name and looked up with the same word/alias matching as the profiles), else the best match. It is resolved where GPU resources are BUILT (`ProfileUse::resolve`, recorded on `ImageResources::profile_use`) — never per render, because the table sizes a texture — and `push_uniforms_and_render` rebuilds (from `raw_cache`) when the choice or default has moved (`handlers::camera_profiles::resources_stale`). Export resolves it the same way per image. `RawDataResult::dcp_profile` is only the automatic pick at decode; app paths must not use it for rendering.
- **One table slot.** A DCP may carry a HueSatMap (50937–50939, dual-illuminant) and/or a LookTable (50981–50982). The pipeline applies one: the LookTable if present (what the D3300 "Camera Standard" calibration uses), else the HueSatMap (RawTherapee's profiles). A profile with neither becomes a 1×1×1 identity table — a zero-sized 3D texture renders **black** with only a log line. An identity ProfileToneCurve ((0,0)–(1,1), RawTherapee's) is treated as none: under this pipeline's linear→display reading of the curve it would darken by a whole gamma.
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
  GPU (in a former pad slot — no offset moved), and the
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
- Preloads are queued nearest-first in *display order* (`navigation::neighbours`: +1, −1, +2, …) and each navigation `requeue`s the not-yet-started tail. The first `MAX_CONCURRENT_*` queue entries are the running decodes — never reorder or drop those.
- Cache workers mark rows `processing` while working; `Library::requeue_interrupted_cache_jobs` returns leftovers to `pending` at startup (only safe before the workers start). `verify_files` reconciles `file_status` in both directions.
- The embedded-JPEG finder (`raw::jpeg`) walks JPEG segment structure. Sensor data contains `FF D8 FF` by chance; first-EOI measuring let such noise outrank real previews, so it is a last resort only.
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
- `viewport.wgsl`'s projection matrix carries the entire placement (fit + zoom + pan); its fragment shader is a plain `textureSample`. The old Phase-25 `zoom`/`pan_x` slots are now `src_scale` (§G, loupe); `pan_y` is vestigial and unread.
- Preview resolutions are multiplied by the window's `scale_factor` for pixel-perfect Retina/4K rendering. It comes from `window::get_scale_factor` (startup + every resize → `Message::ScaleFactorChanged`), never from the canvas, which cannot know it — a hardcoded 1.0 there once halved HiDPI preview resolution. Render targets are additionally clamped to 1:1 with the source — past that there is no more detail to resolve. `core::viewport::render_target` computes window, target and **magnification** (screen pixels per source pixel); any render whose view magnifies the subsampled develop preview past `FULL_RES_MAGNIFICATION` requests `trigger_full_res_upgrade` (rebuild without subsampling, from `raw_cache` or else re-read from disk). Asked by the render, not the zoom handler: zoom is kept across images and tabs, so an image can load straight into a zoomed-in view (it used to stay soft until the next zoom), and HiDPI screens magnify the preview even at fit. One upgrade at a time across images (`full_res_upgrading`, released only by `Message::FullResReady`, which re-checks the current image if the user moved on); a failed one keeps the preview and is not retried for that image. A successful one keeps the subsampled set as `preview_resources`: the overview always renders from it, and so does the display whenever the view doesn't magnify it or a noise-reduction slider is mid-drag (`stage_drag`; the release re-renders at full res) — cached stages cost per *source* pixel, and NR on the full 24 MP texture is ~10× the preview's on an iGPU.
- Non-destructive crop uses normalized `[0.0, 1.0]` UV coords; while `is_cropping == 1` the GPU reveals full sensor data and dims discarded pixels (and windowing is skipped, since zoom/pan are disabled there). `EditParams::is_cropping` is **tool state riding to the shader, not an edit**: `#[serde(skip)]`, forced to 0 for export, and re-derived from `editor.is_cropping` by `sync_tool_flags()` (inside `update_pipeline`, and wherever params are replaced wholesale). Persisting it once made images reopen and export as the crop-mode visualisation.

- **Quarter turns** (`EditParams::quarter_turns`, Develop › Geometry's ⟲/⟳) compose with the EXIF orientation into the one orientation the colour pass already handles (`color::compose_orientation`, `ImageResources::display_orientation`), so a turn is a uniform upload, never a rebuild. Crop, mask shapes, brush strokes and the loupe target all live in the TURNED frame, so `EditParams::turn_quarter` carries them along (radial radii swap axes; brush radii, fractions of the height, rescale). Anything sized by the display shape takes the turns (`oriented_dims(quarter_turns)`; brush layers re-allocate when their aspect changes, or dabs draw as ellipses), and anything mapping a click to the sensor composes them (`sample_raw_rgb`). Some files record no rotation for a vertical frame at all — a D3400 NEF with Orientation = 1, LibRaw flip 0 — and the turn is the fix there, not the metadata reader.
- **Straighten** (`rotation`) is positive counter-clockwise: the colour pass samples at `R(θ)·out`. The level tool (`core::viewport::straighten_from_line`) depends on that sign; its test mirrors the shader's mapping.

### E. Large Catalogues (`views::library`, `ui::filmstrip`)
iced rebuilds the whole view after **every** message, so per-message cost must not scale with the catalogue.
- The library grid and the filmstrip are virtualised: only rows/slots near the scroll offset are built, with spacers for the rest (`GridGeometry`, `filmstrip::visible_items`). Offsets arrive via each scrollable's `on_scroll`; a tab switch rebuilds those scrollables at offset 0, so `handle_tab_changed` resets the stored offsets to match. `reveal_selection` scrolls either one to keep keyboard navigation in view.
- Never add a subscription that emits per mouse move (the old global `CursorMoved` tracking rebuilt every thumbnail on each move). Prefer widget-scoped events — the grid's wheel boost uses its `mouse_area` enter/exit.
- Never build image handles with `Handle::from_bytes` inside `view`: each call mints a new id, and iced re-decodes the image every rebuild. Build once (`LazyLock`), or use `from_path`.
- **UV is not isotropic.** Mask geometry lives in full-image UV, but what's displayed is the crop sub-rect stretched over a full-image-aspect target, so the two axes carry different pixels-per-unit. Anything angular (rotated radial masks) must un-distort by `core::viewport::mask_uv_aspect`, rotate, then redistort — rotating in raw UV shears. Rotation angles are cyclic: use `wrap_degrees`, never a clamp, or a drag sticks at ±180°.

### F. Edited Proxies (`app/handlers/proxy.rs`)
Each image's overview (§A) is kept as a small render of it *as edited*: the filmstrip and library grid show it instead of the camera JPEG (`RawEditor::thumbnail_handle`), and Develop shows it while an image reloads instead of the camera JPEG, which ignores the edit and made the image jump in tone when the real render landed.
- Captured in memory on every render of the **real** edit — never the before/after peek or crop mode (`RenderOutput::edit` is `None` for those). Costs no extra pass.
- Written to `~/.cache/raw-editor/edited/` (column `cache_path_edited`) when the user *leaves* the image, never per frame. Unique filename per save, because iced caches decoded images by path; superseded files are swept at startup.
- **Invariant: a saved proxy always matches the saved edits.** Anything that changes an image's saved edits without rendering it must call `RawEditor::invalidate_proxies` — paste onto a selection does, and `flush_edits` does whenever the selected image isn't the one being rendered (undo/reset in Library). New write paths to `edits` must too.
- Before GPU resources exist, `image_display_dims` returns the placeholder's own shape: a proxy is already cropped, the camera JPEG never is.

### G. Editing Stages (`core::{noise,haze,highlights,curves,lens,brush}`, `gpu::{denoise,local_contrast,brush}`)
- **Denoise** (`gpu/denoise.rs`): NLM on luma, then edge-avoiding à-trous scales that smooth chroma and (from step 2 up) the coarse luma mottle NLM's 9×9 window cannot reach, in an opponent space whose `Y = (r+2g+b)/4` is orthogonal to both chroma axes (so chroma smoothing cannot move brightness). Weights are computed in the variance-stabilised space of the image's own Poisson-Gaussian model (`core::noise::estimate`, measured on the mosaic AS UPLOADED, so a box-filtered preview gets its own lower noise); averages are taken of LINEAR values, which keeps it unbiased — averaging square-root-domain values darkens shadows. Scale count follows the subsample stride so export matches the screen.
- **The noise model is the foundation of every NR threshold** — they are in units of its σ. The estimator measures per-block variance (Immerkær mask per CFA plane) and believes only the quietest blocks per brightness bin (truncated mean, constants calibrated on pure noise); a median-based estimator read scene texture as noise and made every slider over-smooth on detailed frames. Change it only with `core::noise`'s texture test and `eval_tests::denoise_moves_towards_ground_truth` passing.
- **Texture / Clarity** (`gpu/local_contrast.rs`): three bases of log2 luma at `FINE`/`TEXTURE`/`CLARITY` — fractions of the image's long edge, never pixels, so preview, zoom and export pick out the same features. B0/B1 come from edge-avoiding à-trous at full resolution; B2 is a fast guided filter of B1 whose coefficients live on a fixed coarse grid and are evaluated against full-res B1 in Pass 2. (Edge-avoiding à-trous at Clarity's scale streaked along straight low-contrast edges — its taps are tens of pixels apart.) Pass 2 applies `B0−B1` and `B1−B2` as a gain riding the exposure multiplier (scene-linear, hue-preserving, shadow-protected). In log space exposure and WB are offsets, so the stage depends only on its source image: it re-runs when the denoise output changes, never for its own sliders. Bound in colour-pass group 2 with placeholders until first used.
- **Highlight reconstruction** (`core::highlights` + Step 3): darktable's "inpaint opposed", iterated. Each clipped channel (smooth clip weight from `HL_CLIP_LO` of its WB'd ceiling) is rebuilt as the geometric mean of the other two — their *current estimates*, clipped ones included, since a clipped value is a close lower bound — times a learned correction, for `HL_ITERATIONS` Gauss–Seidel rounds; an estimate only ever raises a channel. The old rule (mean of the *unclipped* channels) set a sky's green equal to its red under a clipped blue: the purple cast when exposure is pulled down. The correction is ONE per image (learned at load from unclipped quads near clipping), restricted to the warm–cool axis, stored camera-native in `GpuEditParams.highlight_k` (former pad slots) and converted to the current WB in the shader. Both restrictions are deliberate: a local map took a sky's colour from the defocused skyline below it, and the unrestricted green axis learned foliage. Rebuilt channels go white towards the brightest *estimate* as the least-clipped channel climbs from `HL_FADE_LO` — never towards the ceiling, or a sky rebuilt bluer than grey loses blue as it brightens. `reconstruct` is the CPU mirror the unit tests check; `eval_tests::clipped_sky_matches_the_unclipped_truth` checks the shader against a sky captured with headroom.
- **Dehaze** (`core::haze` + Step 3b): dark-channel prior and a fast guided filter, analysed on the CPU at load on a UV grid (same cells at preview and export) like the noise model; Pass 2 evaluates the filter with each pixel's own guide (sharp skylines) and inverts `I = J·t + A(1−t)` in WB'd camera space after highlight reconstruction. Everything is airlight-normalised, so it is WB- and exposure-invariant. A sky tolerance and a per-pixel floor (`DEHAZE_SKY_K`, `DEHAZE_KEEP`) keep 1/t from turning airlight error into a cast or crushing shadows. The coefficient map is `Rg32Float` (f16 cannot hold `a` and `b`, which nearly cancel), read with a manual bilinear.
- **Held NR + detail loupe** (`handlers::loupe`, `gpu::loupe`): the NR stage costs ~30 ms a tick on the preview (~270 ms at full res) against ~3 ms for any other slider, so while a noise slider moves the main view keeps its NR (`RawEditor::held_noise`) and the sidebar's Detail Preview shows the new value live at 1:1; the main view applies it after `NOISE_SETTLE` (250 ms) without movement, or on release. Every GPU push of the user's edit goes through `RawEditor::view_params()` (never `current_edit_params` directly), and a render under a hold is not an edited proxy (`edit = None`). The loupe denoises a CFA-aligned **crop** of the full-resolution mosaic padded by every stage's reach (`denoise::reach`, derived from the plan — change one, the other follows) and renders it through the unchanged colour pass, whose input reads go through `GpuEditParams.src_scale/src_offset` (full-image UV → input UV; the identity `(1,1)/(0,0)` everywhere else, exact in floating point — any new read of `input_texture` must use `src_uv`, and anything sized by the input's dimensions must scale them by `src_scale`). Its groups 1/2 and LUTs are the main image's, so it cannot drift from the main view; `loupe_matches_a_full_resolution_render` checks it against the export.
- **Quality harness** (`gpu/eval_tests.rs`, all `#[ignore]`, lavapipe is enough): a scene (procedural, or `RAWEDITOR_EVAL_IMAGE`) is mosaicked with known Poisson-Gaussian noise or synthetic haze and rendered through the real pipeline; NR and dehaze are scored against the clean render, and presence effects are checked for band selectivity and preview/export agreement. `nr_sweep` and `presence_dump` are the tuning tools.
- **Curves** apply in display-encoded space after tone mapping, baked on the CPU with a monotone (Fritsch–Carlson) spline; per-channel curves are pre-composed with the master in the LUT.
- **Lens** (`core::lens`): lensfun XML read at runtime — never bundled (CC-BY-SA). Search order: the user's own files (`paths::lensfun_dir`), the copy the app downloads on request (`paths::lensfun_updates_dir`, `core::lensfun`: lensfun's `versions.json` + `version_N.tar.bz2`, validated, then swapped in by rename), then system installs. `LensDb` is reloadable (`LensDb::reload`); a lens profile is resolved when a RAW is **decoded**, so anything that changes the database must drop `raw_cache` and re-decode the selected image (`handlers::lensfun::database_changed`) or the old lookup sticks until restart. Lens names are matched by tokens (every number must agree exactly; words scored by overlap; diacritics folded) and the makers must share a word *after* `MAKER_ALIASES` — rawler names Panasonic's lenses "Lumix", which lensfun files under "Panasonic". Its coefficients use Hugin units: radius 1 = half the calibration sensor's SHORTER side for distortion/TCA, half its DIAGONAL for vignetting; the camera's crop factor comes from the lensfun camera DB before EXIF (whose 35 mm figure is rounded). Step 0 maps output → source exactly like lensfun (`r_d = r_u·f(r_u)`, TCA on the distorted coordinate). The vignetting gain rides the exposure multiplier, not the sample, or brightened corners trip Step 3's clip detection. Lens corrections reframe the image, so `before_reference` preserves them like the crop. **Constrain's zoom is folded into the distortion polynomial** (`d'ₖ = dₖ/s^(k+1)`), never the UV scale: Step 0 converts back to UV with the scale it came in by, so a zoom there cancels — Constrain once did nothing for exactly that reason. `eval_tests::lens_constrain_leaves_no_black_border` checks it through the real shader.
- **Masks**: shape (linear/radial/brush/range) + signed brush layer (`clamp(shape + L, 0, 1)`), × luminance/colour range, then invert. Strokes are data in `EditParams::brush` (fixed capacity so `EditParams` stays `Copy`); deleting a mask must `BrushData::remove_mask` to renumber them. Colour range compares camera-native log-chromaticity (`MASK_CHROMA_EPS`, mirrored in WGSL), picked from the raw mosaic — exposure- and WB-invariant. `mask_overlay` is tool state exactly like `is_cropping`: `#[serde(skip)]`, set by `sync_tool_flags`, stripped by `EditParams::without_tool_state()` for every "same edit?" check and export, and it disqualifies a render from becoming the edited proxy.

### H. Motion (`ui::motion`, `ui::widgets`)
Animation primitives live in `ui::motion` (plain, unit-tested math): **springs** (`Spring`, `Animated`) for things that move — they keep velocity, so retargets bend instead of restarting; **timelines** (`Timeline` + `CubicBezier`, house durations in `motion::duration`) for things that appear; **`approach_factor`** for colour.
- **Widgets animate themselves.** State lives in the iced widget tree, advances on `window::Event::RedrawRequested` (which every frame delivers to `on_event` *before* `draw`), and requests `NextFrame` only while moving. No `Message`/`update`/`view` per frame (§E). App-level motion is the exception: the reveal glide (`scroll_glide`) rides the filmstrip's `window::frames()` tick because it drives `scrollable::scroll_to`.
- `ui::widgets::button` is a **drop-in for `iced::widget::button`** (same builder, same `Fn(&Theme, Status) -> Style`) that eases hover/press/style swaps and depresses on press. Import it instead of iced's. In virtualised lists give it `.key(id)`: state is reused *by position*, so without a key a scrolled slot crossfades from the previous photo's look. Same for `fade_image`.
- **No layer opacity in iced 0.13.** True fades are only possible where we own the colour (`Sheet` draws its own surface; `Image::opacity`). Content fades are a *veil* of the known background drawn on top in a new layer (`with_layer` — within one layer text draws after quads): `FadeThrough` (tab switch), `Collapse::veil`, the modal content stagger. A veil needs a uniform background; over photos use `Entrance` (scale/offset pop) instead. Not everything animates: the before/after peek (BEFORE badge) and the `I` info panels are deliberately instant — a comparison or a glance is only useful if the switch is.
- **`container` is transparent to the widget tree** (it forwards its content's tag/state), so wrapping in one never resets state. `column` does — the menu bar remount (`menu_generation`) relies on that.
- An overlay that captures an event starves the base tree of it, `RedrawRequested` included — an open iced_aw dropdown captures everything under the pointer, which froze anything opened from a menu at frame 0. Entries go through `Message::MenuItem`, which closes the dropdown. iced_aw 0.10 also **panics if a widget operation reaches an open dropdown** (its `operate` `expect`s a submenu on every entry); `scroll_to`/`scroll_by` tasks are operations, so the menu bar sits inside `widgets::no_operate`, which doesn't forward them.
- **Keyboard reveal** (`handlers::scroll`): stepping off an edge moves the strip/grid *just far enough* (`filmstrip::Reveal::Edge` — the item becomes the last/first visible); a tab switch jumps, centred. A held arrow key retargets every repeat, so the glide *tracks* rather than settles: `Spring::track` feeds forward the destination's speed (`hold_rate` — a time average, because repeats reach `update` in bursts when the UI thread is busy and per-repeat speeds read those as 40 000 px/s), aims at a target extrapolated between repeats (a spring chasing the per-repeat staircase lurches), and a governor fades the feed-forward if the strip gets ahead. Key-up (`Message::NavigationKeyReleased`) coasts it to rest at `x + v/ω`, which never reverses.
- **Animate large quantities as a displacement to zero.** `f32` spacing at a six-figure scroll offset is coarser than any rest threshold, so a spring on the raw offset never settled — the reveal glide ran forever, issuing `scroll_to` every frame (and, with the above, crashed when a menu opened). `ScrollGlide` springs `offset - to` instead.
- Modals: closing is detected centrally in `update` (`track_modal_exit`), which keeps the modal mounted *empty* for `MODAL_EXIT` (its state is often already gone — Delete would read "0 images"). New modals need nothing extra; new close paths are covered automatically.
- `Preferences › Reduce motion` sets the process-global `motion::set_reduced`; every primitive then lands on its end state. `RawEditor::new` deliberately doesn't touch the flag under `cfg!(test)` (it's global; tests run in parallel). Dev aid: `RAW_EDITOR_SLOW_MOTION=10 cargo run --release` plays everything 10× slower.

### I. Develop Sliders (`views::develop::slider_field`)
Every sidebar slider goes through `slider_field`. Clicking the value types a number in the units shown (`from_display` reads them back — WB inverts its as-shot anchoring); double-clicking the label or track resets it (`ui::widgets::double_click`, which has to see the press before the slider captures it). Both arrive as `Message::SliderSet { change, commit }`: the slider's own change message, then a commit exactly like a drag's release (`commit: false` for tool settings such as brush size). The field being typed is keyed by `slider_key` — the Debug of the change message, because labels repeat. iced 0.13's text field has no blur event and swallows Escape, so `update::ends_slider_entry` abandons it when the user moves on, and the subscription turns a captured Escape into `SliderEntryCancel`.

## Strict Engineering Constraints (must follow)

1. **Wgpu memory alignment (critical):** any struct sent to a wgpu uniform buffer (e.g. `GpuEditParams`) must be strictly 16-byte aligned. Add explicit padding fields (e.g. `_pad: [f32; 3]`) — misalignment causes GPU memory corruption, not just a panic. Field *offsets* are load-bearing too: `write_view_rect` patches `view_rect` by `offset_of!`, and every Rust uniform struct is mirrored by a WGSL struct that must stay byte-identical (`GpuEditParams`/`GpuMasks` ↔ the colour shader; the demosaic, denoise and brush shaders have their own). The size and offset assertions in `gpu/params.rs`'s tests are the guard — keep them passing rather than adjusting them to match a drift; change them only together with a deliberate layout change in both languages. Every WGSL source has a naga validation test — add one for any new shader (it caught `target`, a reserved word that otherwise fails only at runtime, rendering nothing).
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
   Task::none(); }` as the handler's first line. Key presses a focused widget
   already captured (text inputs, hovered sliders) are dropped before routing —
   otherwise typing in a text field fires shortcuts (Backspace → Delete).
5. **GPU memory is freed lazily, and `SharedContext` is created once.** Dropping
   a wgpu `Texture`/`Buffer` only queues it for reclamation — the allocation
   survives until the next `poll`. Every `poll` in this crate is *inside* a
   render function, before that function's own resources go out of scope, so
   long-running loops must call `SharedContext::reclaim()` between iterations
   and once at the end, or they run a full working set behind (and never free
   the last one). Separately, `editor.gpu_context` must be populated by whoever
   first creates a context: it was only ever assigned on the Develop load path,
   so exporting without opening Develop built a whole new `Instance`/`Device`/
   pipeline set **per image**. Any new path that calls `SharedContext::new()`
   has to store the result back.
6. **Separation of concerns:** keep wgpu logic isolated from iced logic. Iced code should only pass plain Rust primitives/structs/flags down into `SharedContext`/`ImageResources` — no wgpu types leaking into `src/app/`.
7. **Error handling:** use `tracing::info!`/`tracing::error!` for backend operations. Never `.unwrap()` on file I/O or wgpu buffer mapping — propagate the error to UI state or fall back gracefully.
8. **Schema changes:** when changing the SQLite schema, update every SQL query and the corresponding struct in `src/database/models.rs` in the same change; stale DBs must be deleted/migrated (no auto-migration tooling exists yet).

## Design Language
"Premium Creative Pro" meets "Turbo Nerd": deep charcoal/zinc dark mode (`#0A0A0A`–`#111113`), stark white text, electric orange (`#F97316`) accents. "Pure Contact Sheet" aesthetic — images have 0.0 border radius, no intrusive filename overlays. Technical metrics use monospace fonts. Styling lives in `src/ui/styles.rs` / `src/ui/palette.rs`.
