//! The editing session: the app's logic, with no UI toolkit in it.
//!
//! Its methods change state and return [`Effects`]; a front end runs them
//! and hands each result back to [`Session::handle`] as a [`SessionEvent`].
//! Decisions that need a dialog come back as values for the front end to act
//! on; the session never opens one.

mod ai_denoise;
mod ai_mask;
mod camera_profiles;
mod delete;
mod develop;
mod export;
mod helpers;
mod lensfun;
mod library;
mod loader;
mod loading;
mod loupe;
mod masks;
mod lens;
mod navigation;
mod proxy;
mod models;
mod updates;
mod state;
#[cfg(any(test, feature = "test-support"))]
pub mod testkit;
#[cfg(test)]
mod ai_flows;
#[cfg(test)]
mod tests;

pub use camera_profiles::{CameraProfileState, ProfileJob};
pub use state::*;
pub use delete::{DeleteRequest, HardDeleteOutcome};
pub use export::{ExportFormat, ExportJob, ExportJobState, ExportSettings};
pub use lensfun::{LensfunJob, LensfunState};
pub use library::{collect_raw_files, CacheResult, CacheStep, ImportResult};
pub use loader::{LoadJob, MAX_CONCURRENT_PREVIEW_LOADS, MAX_CONCURRENT_RAW_LOADS};
pub use loading::{shared_context, BuiltResources};
pub use loupe::{HeldNoise, LoupeState, LoupeView, LOUPE_SIZE};
pub use ai_mask::{load_ai_bitmaps, AiBitmapState, AiSelect, ModelMissing, SelectEngine, Selection};
pub use ai_denoise::{export_mosaic, AiDenoiseState, DenoiseCrop, DenoiseEngine, PREVIEW_SIZE as AI_DENOISE_PREVIEW_SIZE};
pub use masks::MaskAdjustment;
pub use lens::{DefringeParam, LensParam, LensToggle};
pub use develop::{
    CopyCategory, CropHandle, EditSection, GtParam, MaskHandle, RawSampleError, RenderOutput,
    StrokeEvent, SurfaceFrame,
};
pub use navigation::{Moved, PreviewHandle, SelectionChange, TabLoad};
pub use proxy::{edit_key, load_proxy, read_jpeg, CachedPreview, EditedProxy, EDITED_PROXY_CAPACITY};
pub use models::ModelsState;
pub use updates::{UpdatesAtStartup, UpdatesState};

/// Work for the host to run; each item resolves to a [`SessionEvent`].
pub type Effects = raw_editor_session::Effects<SessionEvent>;

/// The result of a piece of work the session asked for.
#[derive(Debug, Clone)]
// A decoded RAW travels in its variant once, by move, as it did in `Message`.
#[allow(clippy::large_enum_variant)]
pub enum SessionEvent {
    /// An update check finished (`manual`: Check for Updates was clicked).
    UpdateChecked { result: Result<Option<crate::core::updates::Available>, String>, manual: bool },
    /// What lensfun currently publishes.
    LensfunChecked(Result<crate::core::lensfun::Remote, String>),
    /// A lens database download finished (and was swapped in).
    LensfunInstalled(Result<crate::core::lensfun::Installed, String>),
    /// The lens database was read; `true` when it was a rescan that may have
    /// changed what is installed.
    LensDbLoaded(bool),
    /// An AI model download finished.
    ModelDownloaded(crate::ml::models::ModelId, Result<(), String>),
    /// A RawTherapee profile download for this camera finished: the file, or
    /// `None` when RawTherapee has none.
    CameraProfileDownloaded(String, Result<Option<String>, String>),
    /// Imported profiles, and the files that could not be.
    CameraProfilesImported(Vec<String>, Vec<String>),
    /// Checked the downloaded profiles for newer versions.
    CameraProfilesUpdated(Result<crate::core::camera_profiles::UpdateReport, String>),
    /// The profile folders were read; `true` when it was a rescan.
    CameraProfilesScanned(bool),
    /// An import finished (added and skipped counts).
    ImportComplete(Result<ImportResult, String>),
    /// One cache-tier job finished.
    CacheProcessed(CacheResult),
    /// A delete from disk finished.
    HardDeleteComplete(HardDeleteOutcome),
    /// Export the next queued image, or finish the batch.
    ProcessNextExport,
    /// Carries the exported image's OWN saved edit params alongside its RAW
    /// data. They must travel with the job rather than be read from
    /// `current_edit_params`, which is whatever image happens to be open
    /// in Develop — a batch export previously applied that one image's edits to
    /// every image in the queue.
    /// (Boxed: `EditParams` is ~21 KB and would size every `Message`.)
    ExportRawLoaded(
        i64,
        Box<crate::core::types::EditParams>,
        Result<std::sync::Arc<crate::raw::loader::RawDataResult>, String>,
    ),
    /// An export image's full-resolution resources are built.
    ExportPipelineReady(
        i64,
        Box<crate::core::types::EditParams>,
        Result<
            (
                std::sync::Arc<crate::gpu::shared::SharedContext>,
                std::sync::Arc<crate::gpu::shared::ImageResources>,
            ),
            String,
        >,
    ),
    /// An export image was written (its path), or failed.
    ExportSaveComplete(i64, Result<std::path::PathBuf, String>),
    /// A working preview (the camera JPEG) was decoded for the RAM cache.
    PreviewCached(i64, Result<(u32, u32, std::sync::Arc<[u8]>), String>),
    /// A working preview was read for display: its pixels (`None` when they
    /// could not be decoded), their size, and the file.
    WorkingPreviewReady(i64, Option<std::sync::Arc<[u8]>>, (u32, u32), String),
    /// A direct RAW decode (for Develop) finished.
    RawDataLoaded(i64, Result<crate::raw::loader::RawDataResult, String>),
    /// A RAW preload for a neighbouring image finished.
    RawPreloaded(i64, Result<std::sync::Arc<crate::raw::loader::RawDataResult>, String>),
    /// The open image's GPU resources are built.
    ImageResourcesReady(i64, BuiltResources),
    /// The full-resolution rebuild (`Session::trigger_full_res_upgrade`)
    /// finished. Separate from `ImageResourcesReady` so only its own result
    /// releases the one-at-a-time upgrade guard, and so a failed upgrade
    /// keeps the working preview instead of failing the editor.
    FullResReady(i64, BuiltResources),
    /// A develop render completed. Boxed: the histogram alone is 3 KB.
    RenderFinished(Box<RenderOutput>),
    /// The detail loupe for an image was prepared (or could not be).
    LoupeReady(i64, Result<std::sync::Arc<crate::gpu::loupe::Loupe>, String>),
    /// A detail-loupe render finished.
    LoupeRendered(i64, Result<crate::gpu::loupe::LoupeFrame, String>),
    /// The AI-select engine for the session with this token was built.
    AiEngineReady(u64, Result<SelectEngine, String>),
    /// A decoder answer for the AI-select session with this token.
    AiMaskReady(u64, Result<Selection, String>),
    /// An image's AI-mask bitmaps were read from disk.
    AiBitmapsLoaded(i64, Vec<(u64, Result<std::sync::Arc<crate::ml::masks::Bitmap>, String>)>),
    /// The AI denoise dialog's engine for an image was prepared.
    AiDenoiseReady(i64, Result<DenoiseEngine, String>),
    /// PMRID finished the dialog's preview crop.
    AiDenoiseCropReady(i64, Result<DenoiseCrop, String>),
    /// The dialog's preview rendered.
    AiDenoiseRendered(i64, Result<crate::gpu::loupe::LoupeFrame, String>),
    /// Apply's whole-image run finished (amount it was applied at).
    AiDenoiseApplied(i64, f32, Result<(), String>),
    /// A missing AI denoise cache was regenerated.
    AiDenoiseRegenerated(i64, Result<(), String>),
    /// A noise-reduction drag paused: let the main view catch up, if this is
    /// still the latest tick's timer.
    NoiseSettled(u64),
    /// An edited proxy finished saving to disk: (image, proxy generation,
    /// path or error). See `session::proxy`.
    EditedProxySaved(i64, u64, Result<String, String>),
    /// A saved edited proxy was read back into memory: (image, pixels).
    EditedProxyLoaded(i64, Result<(u32, u32, std::sync::Arc<[u8]>), String>),
}

/// What [`Session::handle`] made of an event.
#[must_use]
pub enum Handled {
    /// The session dealt with it; run these next.
    Done(Effects),
    /// The front end has a part in this one (a dialog to close, a
    /// selection to move): it calls the session's method for it itself.
    FrontEnd(Box<SessionEvent>),
}

impl Session {
    /// Hand back the result of an effect. Every event is the session's alone
    /// except the few that need the front end ([`Handled::FrontEnd`]):
    /// `ModelDownloaded` (the download offer closes and its feature
    /// starts), `CacheProcessed` (what finishing is announced as) and
    /// `HardDeleteComplete` (the selection moves to another image).
    pub fn handle(&mut self, event: SessionEvent) -> Handled {
        use SessionEvent as E;
        Handled::Done(match event {
            E::UpdateChecked { result, manual } => {
                self.update_checked(result, manual);
                Effects::none()
            }
            E::LensfunChecked(r) => {
                self.lensfun_checked(r);
                Effects::none()
            }
            E::LensfunInstalled(r) => self.lensfun_installed(r),
            E::LensDbLoaded(changed) => self.lens_db_loaded(changed),
            E::CameraProfileDownloaded(camera, r) => self.camera_profile_downloaded(camera, r),
            E::CameraProfilesImported(ok, failed) => self.camera_profiles_imported(ok, failed),
            E::CameraProfilesUpdated(r) => self.camera_profiles_updated(r),
            E::CameraProfilesScanned(changed) => self.camera_profiles_scanned(changed),
            E::ImportComplete(res) => self.import_complete(res),
            E::ProcessNextExport => self.process_next_export(),
            E::ExportRawLoaded(id, params, res) => self.export_raw_loaded(id, *params, res),
            E::ExportPipelineReady(id, params, res) => self.export_pipeline_ready(id, *params, res),
            E::ExportSaveComplete(id, res) => self.export_save_complete(id, res),
            E::PreviewCached(id, res) => {
                self.preview_cached(id, res);
                Effects::none()
            }
            E::WorkingPreviewReady(id, bytes, dims, path) => {
                self.working_preview_ready(id, bytes, dims, path);
                Effects::none()
            }
            E::RawDataLoaded(id, res) => self.raw_data_loaded(id, res),
            E::RawPreloaded(id, res) => self.raw_preloaded(id, res),
            E::ImageResourcesReady(id, res) => self.image_resources_ready(id, res),
            E::FullResReady(id, res) => self.full_res_ready(id, res),
            E::RenderFinished(out) => self.render_finished(*out),
            E::LoupeReady(id, r) => self.loupe_ready(id, r),
            E::LoupeRendered(id, r) => self.loupe_rendered(id, r),
            E::NoiseSettled(token) => self.noise_settled(token),
            E::AiEngineReady(token, r) => self.ai_engine_ready(token, r),
            E::AiMaskReady(token, r) => self.ai_mask_ready(token, r),
            E::AiBitmapsLoaded(id, loaded) => self.ai_bitmaps_loaded(id, loaded),
            E::AiDenoiseReady(id, r) => self.ai_denoise_ready(id, r),
            E::AiDenoiseCropReady(id, r) => self.ai_denoise_crop_ready(id, r),
            E::AiDenoiseRendered(id, r) => self.ai_denoise_rendered(id, r),
            E::AiDenoiseApplied(id, a, r) => self.ai_denoise_applied(id, a, r),
            E::AiDenoiseRegenerated(id, r) => self.ai_denoise_regenerated(id, r),
            E::EditedProxySaved(id, generation, result) => {
                self.proxy_saved(id, generation, result);
                Effects::none()
            }
            E::EditedProxyLoaded(id, result) => {
                self.proxy_loaded(id, result);
                Effects::none()
            }
            front @ (E::ModelDownloaded(..) | E::CacheProcessed(_) | E::HardDeleteComplete(_)) => {
                return Handled::FrontEnd(Box::new(front))
            }
        })
    }
}

/// Run blocking `work` on tokio's blocking pool and report its result as an
/// event. A panic in `work` arrives as `Err("background task failed: …")`.
fn blocking<T: Send + 'static>(
    label: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(Result<T, String>) -> SessionEvent + Send + 'static,
) -> Effects {
    Effects::perform(label, async move {
        done(tokio::task::spawn_blocking(work).await.map_err(|e| format!("background task failed: {e}")))
    })
}

// The state below moved here from `RawEditor` as is (comments included).
pub use raw_editor_session::Changed;
use crate::{database, gpu, raw};
use lru::LruCache;
use std::collections::{HashMap, HashSet};
use crate::database::models::Image as ImageData;
use std::num::NonZeroUsize;
use std::sync::Arc;

#[derive(Debug)]
pub struct Session {
    /// `AppSettings::update_check`: `None` until the first-run prompt is answered.
    pub update_check: Option<bool>,
    /// The update check's progress and result.
    pub updates: UpdatesState,
    /// Per-camera default profile (`AppSettings::camera_profile_defaults`).
    pub camera_profile_defaults: std::collections::HashMap<String, String>,

    /// Image IDs the open Delete modal acts on — a stable snapshot taken
    /// when the modal opens, not re-read live from multi_selection.
    pub pending_delete_ids: Vec<i64>,
    /// Guards against a double Enter-press firing two overlapping delete
    /// batches while a hard-delete is already in flight.
    pub is_deleting: bool,

    // Phase 85: User-configurable cache capacity
    pub cache_capacity: usize,

    // Phase 89: Export Studio
    pub export_settings: ExportSettings,
    /// Remaining work, consumed with `pop()`. Stored REVERSED relative to
    /// display order so popping yields images front-to-back.
    pub export_queue: Vec<i64>,
    /// The whole batch in display order, with per-image progress. Survives the
    /// run so the Export tab can show results afterwards.
    pub export_jobs: Vec<ExportJob>,
    pub is_exporting: bool,

    /// The catalog database (Phase 23: Optional during startup)
    pub library: Option<database::library::Library>,
    /// Status message to display to the user
    pub status: String,
    /// What changed that the front end caches a drawing of (`Changed`);
    /// it drains this after each call.
    pub changed: Changed,
    /// What the camera-preview image (Cull, and Develop before the first
    /// render) should show now, when a call changed it; the front end
    /// drains this after each call and holds the picture in its own form.
    pub camera_preview: Option<PreviewHandle>,
    /// All images loaded from the database
    pub images: Vec<ImageData>,
    /// Images still awaiting cache-tier generation. Seeded from SQLite once
    /// when the workers start, then decremented in memory as each completes —
    /// re-querying (and reloading the whole catalog) per completed image made
    /// import O(N^2) and froze the UI.
    pub pending_cache_count: i64,
    /// Currently selected image ID
    pub selected_image_id: Option<i64>,
    /// Phase 73: RAM cache for 1280px previews (Look-Ahead)
    pub preview_cache: LruCache<i64, CachedPreview>,
    /// Small renders of images as edited, newest per image.
    pub edited_proxies: LruCache<i64, EditedProxy>,
    /// Last `EditedProxy::generation` handed out.
    pub proxy_generation: u64,
    /// Images the user left while the render of their final edit was still in
    /// flight, with that edit: `Session::resolve_awaiting_proxy` saves the proxy when
    /// the render lands.
    pub proxy_awaiting: HashMap<i64, crate::core::types::EditParams>,
    /// Saved proxies being read back for Develop's neighbours — same queue
    /// discipline as `queued_loads`.
    pub pending_proxy_loads: HashSet<i64>,
    pub queued_proxy_loads: Vec<(i64, String)>,
    /// RAW preload RAM budget in MB (0 = disabled)
    pub raw_preload_budget_mb: u32,
    /// Physical RAM to keep free during batch export, in MB (0 = disabled).
    /// Unlike `raw_preload_budget_mb`, which caps the app's OWN cache, this is
    /// a check against the whole system — see `core::memory`.
    pub min_free_ram_mb: u32,
    /// True while a batch is paused waiting for memory, so the UI can say so
    /// and the log can fire once per transition instead of once per check.
    pub export_paused_for_memory: bool,
    /// Decoded RAW cache for fast develop navigation
    pub raw_cache: LruCache<i64, Arc<raw::loader::RawDataResult>>,
    /// Approximate memory usage of raw_cache in bytes
    pub raw_cache_bytes: usize,
    /// Cache workers currently running (`library::spawn_cache_workers` tops
    /// this up to `CACHE_WORKERS`; a retiring worker gives its slot back).
    pub cache_workers_live: usize,
    /// The open image's RAW, held outside the preload budget (which can be 0,
    /// or evict the open image first). Read through `raw_for`.
    pub current_raw: Option<(i64, Arc<raw::loader::RawDataResult>)>,
    /// Number of previews to keep preloaded behind current image
    pub preview_preload_behind: usize,
    /// Number of previews to keep preloaded ahead of current image
    pub preview_preload_ahead: usize,
    /// Number of RAW files to preload behind current image
    pub raw_preload_behind: usize,
    /// Number of RAW files to preload ahead of current image
    pub raw_preload_ahead: usize,
    /// Currently active tab
    pub current_tab: AppTab,
    /// Current edit parameters for the selected image
    pub current_edit_params: crate::core::types::EditParams,
    // Phase 140: Currently loaded DCP profile
    pub current_dcp_profile: Option<Arc<crate::raw::dcp::DcpProfile>>,
    /// Memo for `resolve_wb_and_dcp`, keyed on the only two inputs the
    /// interpolation depends on: `(kelvin, ToneSettings)`.
    ///
    /// Without it, every slider tick — exposure, contrast, a mask drag —
    /// rebuilt the full HueSatMap LUT and re-baked the 1024-entry tone curve,
    /// then handed the result to `update_uniforms`, which discarded it via its
    /// own dirty-check on the same two values.
    ///
    /// `RefCell` rather than a plain field so `resolve_wb_and_dcp` can keep its
    /// `&RawEditor` signature: the crop and mask drag paths call it while
    /// holding an immutable borrow of `gpu_context`/`image_resources`.
    /// Cleared on image load, where `current_dcp_profile` changes.
    pub dcp_memo: std::cell::RefCell<Option<DcpMemo>>,
    /// As-shot white balance of the current image as (Kelvin, Adobe tint),
    /// solved from the camera's WB multipliers via the color matrix (Robertson
    /// CCT). Anchor for the Temp/Tint sliders; None until a raw is loaded.
    pub as_shot_wb: Option<(f32, f32)>,

    // Phase 95: Unified GPU Pipeline Architecture
    /// Shared GPU context (created once, reused for all images)
    pub gpu_context: Option<Arc<gpu::shared::SharedContext>>,
    /// Where the context is created, exactly once (see
    /// `session::shared_context`): tasks started before the first
    /// finishes would otherwise each build their own device.
    pub gpu_init: Arc<tokio::sync::OnceCell<Arc<gpu::shared::SharedContext>>>,
    /// Current image resources (created per image, wrapped in Arc)
    pub image_resources: Option<Arc<gpu::shared::ImageResources>>,
    /// The subsampled develop preview of the same image, kept once
    /// `image_resources` has been upgraded to full resolution. Renders use it
    /// whenever full resolution would add nothing — a view that does not
    /// magnify it, the whole-image overview, a noise-reduction drag — because
    /// the cached stages (denoise) cost per SOURCE pixel: at 24 MP every NR
    /// tick re-ran them over the whole sensor, a few fps on an iGPU. Always
    /// None or the same image as `image_resources`; cleared with it.
    pub preview_resources: Option<Arc<gpu::shared::ImageResources>>,
    /// Editor readiness status
    pub editor_readiness: EditorReadiness,

    /// Phase 21: Histogram data [R[256], G[256], B[256]]
    pub histogram_data: std::cell::RefCell<[[u32; 256]; 3]>,
    /// True while Space is held to peek at the unedited original.
    ///
    /// Actually read, unlike the `show_before` flag this replaces: it selects
    /// which params `handlers::develop::refresh_for_peek_state` uploads, and
    /// drives the BEFORE badge in the develop view. Never persisted, and the
    /// peek's params are never written to `current_edit_params` — doing so
    /// would let a flush point save them over the user's real edits.
    pub before_peek: bool,
    /// Phase 25: Zoom level (1.0 = 100%, 2.0 = 200%, etc.)
    pub zoom: f32,
    /// Phase 115: Native GPU Shader Viewport raw pixel buffers
    pub working_preview_bytes: Option<std::sync::Arc<[u8]>>,
    pub rendered_preview_bytes: Option<std::sync::Arc<[u8]>>,
    /// Monotonic id of the preview pixel contents, handed to the viewport
    /// shader so it can skip re-uploading an unchanged texture every redraw.
    /// Bump via `bump_preview_generation` whenever either bytes field is
    /// replaced. Starts at 1; 0 is the shader's placeholder sentinel.
    pub preview_generation: u64,
    /// Region of the image the preview buffer ACTUALLY on screen covers,
    /// `(u0, v0, du, dv)` in view UV.
    ///
    /// Published only when new bytes land, never when a render is merely
    /// requested — the two must move together or the still-current texture
    /// gets drawn at the incoming window's placement, which reads as the whole
    /// image jumping for a frame.
    pub rendered_view_rect: [f32; 4],
    /// Region the in-flight (or most recently requested) render covers.
    ///
    /// Separate from `rendered_view_rect` so "do we need to re-render?" can be
    /// answered against what is already on its way, rather than re-requesting
    /// the same window on every pan event while it is still rendering.
    pub requested_view_rect: [f32; 4],
    /// Phase 115: Pixel dimensions for the above preview byte buffers
    pub working_preview_dims: (u32, u32),
    pub rendered_preview_dims: (u32, u32),
    /// Phase 25: Pan offset in normalized coordinates
    pub pan_offset: cgmath::Vector2<f32>,
    /// Phase 26: Viewport size for zoom-to-cursor calculations (actual displayed size)
    pub viewport_size: (f32, f32), // (width, height) in screen pixels
    /// Set by the per-tick edit path, cleared by `flush_edits` at commit
    /// points. Keeps SQLite writes off the slider-drag hot path.
    pub edits_dirty: bool,
    pub scale_factor: f32,
    /// Phase 103: High-quality rendered preview (async updated)
    /// Phase 41: Current image metadata for inspection
    pub current_metadata: Option<raw::loader::RawDataResult>,
    /// Which image `current_metadata` was read from. It is only refreshed by
    /// a Develop RAW load, so without this the Cull overlay showed whatever
    /// photo was last opened in Develop. Read it via `selected_metadata`.
    pub current_metadata_id: Option<i64>,
    /// Phase 54: Edit settings clipboard for copy/paste.
    ///
    /// Carries the copied params AND which categories were selected, so paste
    /// can merge rather than overwrite — see `core::types::EditClipboard`.
    pub edit_clipboard: Option<crate::core::types::EditClipboard>,
    /// Current selection in the Copy Settings picker. Persisted to
    /// `AppSettings` so it survives a restart.
    pub copy_categories: crate::core::types::CopyCategories,
    /// Phase 55: Multi-selection set (image IDs)
    pub multi_selection: HashSet<i64>,
    /// Phase 83: Auto-advance to next image after rating/flagging
    pub auto_advance: bool,
    /// Phase 59: Minimum rating filter (0 = show all, 1-5 = show rating or higher)
    pub min_filter_rating: u8,
    /// Library folder filter (path prefix)
    pub library_folder_filter: Option<String>,
    /// Phase 65: Undo/Redo History Map<ImageID, (HistoryStack, CurrentIndex)>
    pub history_map: HashMap<i64, (Vec<crate::core::types::EditParams>, usize)>,
    /// Phase 67: Interactive crop mode
    pub is_cropping: bool,
    /// WB eyedropper tool active: next click on the preview samples a neutral.
    /// Mutually exclusive with `is_cropping`.
    pub is_wb_picking: bool,
    /// Level tool armed: the next drag on the preview draws a line that
    /// should be level (or upright), and sets Straighten from it. Mutually
    /// exclusive with every other tool.
    pub is_leveling: bool,
    /// Local adjustment mask tool (placement mode). Mutually exclusive with
    /// crop and WB picker.
    pub mask_tool: MaskTool,
    /// Index into current_edit_params.masks of the mask being edited
    /// (overlay handles + sidebar sliders). None = no mask selected.
    pub selected_mask: Option<usize>,
    /// The brush is armed: a drag on the image paints into the selected mask.
    /// Like every tool, exclusive with the others.
    pub brush_armed: bool,
    /// Brush tool settings for painting masks (UI state).
    pub brush_settings: BrushSettings,
    /// A stroke is in progress.
    pub is_painting: bool,
    /// Tint the selected mask's coverage over the render.
    pub show_mask_overlay: bool,
    /// The colour-range eyedropper is armed for this mask.
    pub color_picking_mask: Option<usize>,
    /// Phase 78: Async Task Deduplication (track pending preview loads)
    pub pending_loads: HashSet<i64>,
    /// Phase 81: Throttled preview loader queue
    pub queued_loads: Vec<(i64, String)>,
    /// Pending RAW preload jobs
    pub pending_raw_loads: HashSet<i64>,
    /// Throttled RAW preload queue
    pub queued_raw_loads: Vec<(i64, String)>,
    /// Images whose selected-image RAW decode is in flight (started outside the
    /// preload queue), so a second selection does not start a second decode.
    pub direct_raw_loads: HashSet<i64>,

    /// Phase 104: Performance Profiler
    pub profiler: crate::core::profiler::Profiler,

    /// Phase 106: Render Throttling
    pub is_rendering: bool,
    pub pending_render: bool,
    /// Params pushed while a render was in flight, with the image they belong
    /// to. The render task is the only writer of the uniform block while it
    /// runs (it patches `view_rect` between its own submits, and the stages
    /// read `stage_params` at render time), so the UI thread parks the push
    /// here and `Session::render` applies it before the next frame.
    pub deferred_push: Option<(i64, crate::core::types::EditParams)>,
    /// True while a full-resolution resource rebuild is in flight. One at a
    /// time, across images: each holds a full-size working set on the GPU,
    /// and navigating quickly must not stack them. Cleared only by that
    /// rebuild's own result (`SessionEvent::FullResReady`).
    pub full_res_upgrading: bool,
    /// The image whose full-resolution rebuild last failed (out of GPU
    /// memory, say): not retried on every render while it stays selected.
    pub full_res_failed: Option<i64>,
    /// A noise-reduction slider is being dragged (set by its change handlers,
    /// cleared by `commit_current_state` — the slider's release). While set,
    /// a zoomed-in view renders from `preview_resources` so the drag stays
    /// interactive; the release re-renders at full resolution.
    pub stage_drag: bool,
    /// When the last undo step was pushed (`commit_current_state`). A double-click
    /// reset on a slider amends that step instead of adding a second one.
    pub last_commit: Option<std::time::Instant>,
    /// The on-screen render came from `preview_resources` only because of a
    /// `stage_drag`, so it is softer than the view calls for.
    pub display_from_drag_preview: bool,
    /// See [`HeldNoise`]. `Some` from a noise slider's first tick until the
    /// drag pauses (`NOISE_SETTLE`) or ends.
    pub held_noise: Option<HeldNoise>,
    /// Matches the settle timer to the latest noise-slider tick.
    pub noise_settle_token: u64,
    pub loupe: LoupeState,
    pub lensfun: LensfunState,
    /// The open AI-select session, if any (`handlers::ai_mask`).
    pub ai_select: Option<AiSelect>,
    /// The selected image's decoded AI-mask bitmaps.
    pub ai_bitmaps: AiBitmapState,
    /// AI model downloads (`handlers::models`).
    pub models: ModelsState,
    /// The AI denoise modal (`handlers::ai_denoise`).
    pub ai_denoise: AiDenoiseState,
    pub camera_profiles: CameraProfileState,
}

impl Session {
    /// A session with no catalogue yet (it loads in the background), set up
    /// from the saved settings.
    pub fn new(settings: &crate::core::settings::AppSettings) -> Self {
        let cache_capacity = settings.cache_capacity.clamp(20, 500);
        let raw_preload_budget_mb = settings.raw_preload_budget_mb.min(4096);
        let preview_preload_behind = settings
            .preview_preload_behind
            .min(PREVIEW_PRELOAD_BEHIND_MAX);
        let preview_preload_ahead = settings
            .preview_preload_ahead
            .min(PREVIEW_PRELOAD_AHEAD_MAX);
        let raw_preload_behind = settings.raw_preload_behind.min(RAW_PRELOAD_BEHIND_MAX);
        let raw_preload_ahead = settings.raw_preload_ahead.min(RAW_PRELOAD_AHEAD_MAX);
        Self {
                update_check: settings.update_check,
                updates: UpdatesState::default(),
                library: None, // Phase 23: Database loads in background
                status: "Starting…".to_string(),
                images: Vec::new(), // Empty until database loads
                pending_cache_count: 0,
                selected_image_id: None,
                preview_cache: LruCache::new(NonZeroUsize::new(cache_capacity).unwrap()),
                edited_proxies: LruCache::new(
                    NonZeroUsize::new(EDITED_PROXY_CAPACITY).unwrap_or(NonZeroUsize::MIN),
                ),
                proxy_generation: 0,
                proxy_awaiting: HashMap::new(),
                pending_proxy_loads: HashSet::new(),
                queued_proxy_loads: Vec::new(),
                raw_preload_budget_mb,
                min_free_ram_mb: settings.min_free_ram_mb.min(MIN_FREE_RAM_MB_MAX),
                export_paused_for_memory: false,
                raw_cache: LruCache::new(NonZeroUsize::new(RAW_CACHE_ENTRY_CAPACITY).unwrap()),
                raw_cache_bytes: 0,
                cache_workers_live: 0,
                current_raw: None,
                preview_preload_behind,
                preview_preload_ahead,
                raw_preload_behind,
                raw_preload_ahead,
                current_tab: AppTab::Library, // Start in Library view
                current_edit_params: crate::core::types::EditParams::default(),
                current_dcp_profile: None, // Phase 140: No DCP profile initially
                dcp_memo: std::cell::RefCell::new(None),
                as_shot_wb: None,

                // Phase 95: Unified GPU Pipeline Architecture
                gpu_context: None, // Will be initialized on first image load
                gpu_init: Arc::new(tokio::sync::OnceCell::new()),
                image_resources: None,
                preview_resources: None,
                editor_readiness: EditorReadiness::NoSelection,

                histogram_data: std::cell::RefCell::new([[0; 256]; 3]),
                before_peek: false,
                zoom: 1.0,
                pan_offset: cgmath::Vector2::new(0.0, 0.0),
                viewport_size: (800.0, 400.0),
                edits_dirty: false,
                scale_factor: 1.0,
                working_preview_bytes: None,
                changed: Changed::NONE,
                camera_preview: None,
                brush_armed: false,
                rendered_preview_bytes: None,
                preview_generation: 1,
                rendered_view_rect: crate::gpu::params::FULL_VIEW_RECT,
                requested_view_rect: crate::gpu::params::FULL_VIEW_RECT,
                working_preview_dims: (1, 1),
                rendered_preview_dims: (1, 1),
                current_metadata: None,
                current_metadata_id: None,
                edit_clipboard: None,
                copy_categories: settings.copy_categories,
                multi_selection: HashSet::new(),
                auto_advance: settings.auto_advance,
                min_filter_rating: 0,
                library_folder_filter: None,
                pending_delete_ids: Vec::new(),
                is_deleting: false,
                // Phase 85
                cache_capacity,
                // Phase 89
                export_settings: ExportSettings::default(),
                export_queue: Vec::new(),
                export_jobs: Vec::new(),
                is_exporting: false,

                // Phase 104/105: Profiler
                profiler: crate::core::profiler::Profiler::new(),
                is_rendering: false,
                pending_render: false,
                deferred_push: None,
                full_res_upgrading: false,
                full_res_failed: None,
                stage_drag: false,
                last_commit: None,
                display_from_drag_preview: false,
                held_noise: None,
                noise_settle_token: 0,
                loupe: LoupeState::default(),
                lensfun: LensfunState::default(),
                ai_select: None,
                ai_bitmaps: Default::default(),
                models: Default::default(),
                ai_denoise: Default::default(),
                camera_profiles: CameraProfileState::default(),
                camera_profile_defaults: settings.camera_profile_defaults.clone(),

                // Background task
                // background_task: None, // This field is not defined in the struct
                history_map: HashMap::new(),
                is_cropping: false,
                is_wb_picking: false,
                is_leveling: false,
                mask_tool: MaskTool::Inactive,
                selected_mask: None,
                brush_settings: BrushSettings::default(),
                is_painting: false,
                show_mask_overlay: false,
                color_picking_mask: None,
                pending_loads: HashSet::new(),
                queued_loads: Vec::new(),
                pending_raw_loads: HashSet::new(),
                queued_raw_loads: Vec::new(),
                direct_raw_loads: HashSet::new(),
        }
    }
}

impl Default for Session {
    /// The session for default settings (tests).
    fn default() -> Self {
        Self::new(&crate::core::settings::AppSettings::default())
    }
}
