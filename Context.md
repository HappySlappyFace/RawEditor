# Project Context

## Vision & Objectives
- Build a native, cross-platform RAW photo editor that can replace Lightroom Classic.
- Prioritize performance (GPU acceleration, responsive UI), native technologies (no web stack), and professional workflows.
- Provide a modern develop workflow with non-destructive edits, fast navigation, and high-throughput batch processing.

## Methodology
- **Phase-based delivery.** Each phase targets a single UX or performance area (e.g., zoom polish, workflow shortcuts, cache generation). Work is completed before moving to the next phase.
- **Rust-first architecture.** All features integrate into the existing Rust/Iced codebase, with clear separation across modules such as `src/main.rs`, `src/state`, `src/gpu`, `src/raw`.
- **Async-aware design.** CPU-intensive work (RAW decoding, cache generation, database imports) runs on background threads via `tokio::task::spawn_blocking`, returning results through Iced `Message`s.
- **Console-driven verification.** Each phase adds detailed logging (zoom metrics, cache tiers, pipeline states) to validate behaviour without formal automated tests.
- **Respect existing code.** Enhancements extend current modules rather than rewriting them, ensuring maintainability and predictable integration.

## Technology Stack
- **GUI:** `iced` (Rust GUI framework).
- **GPU:** `wgpu` with custom WGSL shaders (`src/gpu/shaders.rs`, `src/gpu/pipeline.rs`).
- **RAW decoding:** `rawloader` (`src/raw/loader.rs`, `src/raw/processor.rs`).
- **Image processing:** `image` crate for JPEG decoding/resizing (`src/raw/processor.rs`, `src/raw/thumbnail.rs`).
- **Database:** SQLite via `rusqlite` (`src/state/library.rs`).
- **File system:** `dirs` / `dirs_next` for platform-aware cache paths.
- **Utilities:** `tokio` for background tasks, `walkdir` for folder traversal, `iced_aw` for UI widgets.

## Phase Timeline & Highlights

### Phase 0–20 (Foundations)
- Set up SQLite catalog (`src/state/library.rs`) and data structures (`src/state/data.rs`).
- Implemented folder import, RAW metadata ingestion, legacy thumbnail cache.
- Built Develop tab pipeline (GPU render, histogram, RAW loader) in `src/main.rs`, `src/gpu/pipeline.rs`.

### Phase 23 – Async Infrastructure
- Added background database load (`load_database_async()`) and import pipeline returning `Message::DatabaseLoaded`/`Message::ImportComplete`.
- Maximized window on startup via `iced::window::maximize`.

### Phase 24 – Workflow Polish
- Added messages: `ToggleBeforeAfter`, `ResetEdits`, `SelectNextImage`, `SelectPreviousImage` in `src/main.rs`.
- Registered keyboard subscription (`RawEditor::subscription()`) mapping Space/R/Arrow keys to workflow actions.
- Before/after caching and histogram updates implemented in `RawEditor::view_develop()`.

### Phase 25 – GPU Zoom & Pan
- Introduced `zoom`, `pan_offset`, `canvas_cache` in `RawEditor` state.
- Added `Zoom`, `Pan`, `MousePressed`, `MouseReleased`, `MouseMoved` messages for interactive navigation.
- Updated WGSL shader in `src/gpu/shaders.rs` to apply zoom/pan; uniforms supplied via `RenderPipeline::update_uniforms_with_zoom()`.
- Mouse handling resides in `RawEditor::view_develop()` with `mouse_area` integration.

### Phase 26 – Zoom Polish
- Implemented zoom-to-cursor math, double-click reset via new `Message::ResetView`.
- Tracked viewport size to eliminate drift; maintained last cursor position correctly during drag.
- Allowed edge clamping with tolerances to avoid dead zones.

### Phase 27 – Vibrance Slider
- Added `vibrance` parameter to UI (slider in `view_develop()`), state (`EditParams`), GPU uniforms.
- Shader computes `vibrance_amount = params.vibrance * (1.0 - saturation)` for skin-tone protection.
- Fixed initial scaling bug by removing redundant `/ 100.0` inside shader.

### Phase 28 – Multi-Tier Cache Processor
- Database schema updated: `cache_path_thumb`, `cache_path_instant`, `cache_path_working` (replaces `thumbnail_path`, `preview_path`).
- New processor (`src/raw/processor.rs`) extracts largest embedded JPEG once and generates 256/384/1280 px JPEG caches.
- Background queue (`process_cache_async()` in `src/main.rs`) pulls pending images, spawns processor, updates DB via `Library::set_image_cache_paths()`.
- Old thumbnail system retired (deactivated `Message::ThumbnailGenerated` handling).
- Library grid now counts cached images using `cache_path_thumb`.

### Phase 29
- Implemented the "Instant Preview" architecture, showing a 384px image immediately upon selection to eliminate UI freezing.

### Phase 30
- Built the Async Multi-Tier Loading pipeline (Instant → Working Preview → Full RAW) for seamless image transitions.

### Phase 31
- Unified the UI container hierarchy to ensure the layout stays stable during loading states.

### Phase 32
- Created a custom Canvas Preview Renderer to ensure pixel-perfect alignment (no jumping) when hot-swapping from JPEG to RAW.

### Phase 33
- Extracted EXIF metadata (ISO, Shutter, Lens) for the UI and fixed the canvas overdraw clipping bug.

### Phase 34-36 (The Checkerboard War)
- Debugged and fixed a critical shader normalization bug in `get_neighbor` that was causing CMY grid artifacts.

### Phase 37
- Upgraded the demosaicing algorithm from Nearest-Neighbor to Bilinear Interpolation, creating smooth, high-res details.

### Phase 48
- Implemented Bradford Chromatic Adaptation to correctly map camera colors to sRGB, fixing the "Green/Pink" tint issues.

### Phase 49
- Added real-time GPU Chroma Noise Reduction to eliminate color speckles.

### Phase 50
- Implemented Unsharp Mask Sharpening to restore crisp edge details.

### Phase 51
- Added Edge Masking to the sharpener, preventing noise amplification in smooth areas like skies.

### Phase 52
- Built the Rotation/Straighten tool using aspect-ratio-aware texture coordinate transformations.

### Phase 53
- Built the Filmstrip Timeline, allowing rapid image navigation at the bottom of the Develop tab.

### Phase 54
- Created the Settings Clipboard (Copy/Paste) to transfer edit parameters between images.

### Phase 55
- Enabled Multi-Selection (Ctrl+Click) and Batch Paste, allowing edits to be applied to groups of photos.

### Phase 56
- Implemented Star Ratings (0-5) with database persistence and gold star overlays on thumbnails.

### Phase 57
- Embedded Nerd Fonts directly into the binary to ensure consistent text rendering across platforms.

### Phase 58
- Replaced broken text emojis with a robust Icon System using vector glyphs.

### Phase 59
- Added the Filter Bar to instantly hide/show images based on their star rating.

### Phase 60
- Modernized the layout with a Floating HUD for metadata and a slim top navigation bar.

### Phase 61
- Refactored the sidebar sliders into a Compact "Pro" Style (Label Left, Slider Right) with neutral colors.

### Phase 62
- Built a Custom Window Chrome, removing the OS title bar for a fully immersive, borderless experience.

### Phase 63
- Finalized the UI by styling all buttons with a Neutral Theme and condensing Copy/Paste into an icon strip.

## Key Implementations & File Map
- **State management:** `src/state/data.rs`, `src/state/library.rs` (schema, queries, cache path updates, edit persistence).
- **UI & workflow:** `src/main.rs` (messages, update loop, subscriptions, import/cache tasks, view builders).
- **GPU pipeline:** `src/gpu/pipeline.rs` (render pipeline setup, uniform updates, zoom/pan integration).
- **Shader logic:** `src/gpu/shaders.rs` (exposure, tone, saturation, vibrance passes, clamps).
- **RAW processing:** `src/raw/loader.rs`, `src/raw/processor.rs`, `src/raw/thumbnail.rs` (legacy).
- **Cache directories:** `~/.cache/raw-editor/{thumb,instant,working}`, created on demand in `raw::processor`.

## Challenges & Resolutions
- **Schema changes causing crashes (`no such column`).** Solved by updating every SQL query and `Image` struct simultaneously, and instructing users to delete outdated DB files.
- **Legacy thumbnail loop running alongside new cache system.** Resolved by short-circuiting `Message::ThumbnailGenerated` and removing legacy queue invocations.
- **Zoom drift and edge dead zones.** Fixed by storing viewport dimensions, clamping cursor coordinates, and adjusting drag logic order.
- **Vibrance double scaling.** Removed redundant `/ 100.0` scaling inside shader to match slider range.
- **Instant preview gap.** Documented as future work: caches exist but Develop tab still loads RAW; upcoming phase must swap to cached JPEG before RAW pipeline completes.

## Lessons Learned & Best Practices
- When altering the database schema, audit all SQL queries and data structures in the same change.
- Disable superseded background jobs immediately to avoid resource contention (e.g., legacy thumbnails vs. new cache processor).
- Maintain detailed console logging for each pipeline step; it speeds up diagnosis of performance or logic bugs.
- Keep UI interactions decoupled from heavy work by using background tasks and message-driven updates.
- Ensure consistent units across UI sliders, state, and shaders to avoid scaling bugs.

## Open Items & Future Work
- **Phase 29 (planned):** Display `cache_path_working` (1280px) immediately in Develop tab, then load RAW asynchronously and swap to GPU output once ready.
- **Cache validation:** Detect missing cache files and requeue processing.
- **Parallel processing:** Explore processing multiple cache jobs concurrently while maintaining UI responsiveness.
- **User feedback:** Surface cache progress/errors within the UI instead of relying solely on console output.
- **Migration tooling:** Provide helpers or automated migration scripts for future schema changes.

## Troubleshooting Tips
- **Schema errors:** Delete `raw_editor.db` when schema changes; ensure queries reference new columns (`cache_path_thumb`, etc.).
- **Cache issues:** Check `~/.cache/raw-editor/` for `thumb`, `instant`, `working` folders; if empty, verify background processor logs in console.
- **Performance checks:** For zoom/pan issues, confirm viewport metrics logged in console and that `ResetView` is functioning.
- **Vibrance behaviour:** Verify shader vibrance amount matches slider range (no extra scaling) and confirm color results against expectation.

## Glossary
- **Instant cache (384px):** Mid-resolution JPEG used for fast previews.
- **Working cache (1280px):** High-resolution JPEG targeted for immediate Develop tab display before RAW pipeline loads.
- **Legacy thumbnail system:** Original `thumbnail_path` workflow; now deprecated in favour of the multi-tier cache processor.
- **GPU pipeline:** `RenderPipeline` handling exposure/tone/color adjustments via WGSL shaders.

---
_Last updated: 2025-11-26_
