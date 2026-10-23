use crate::app::message::{AppTab, Message};
use crate::database;
use crate::database::models::Image as ImageData;
use crate::gpu;
use crate::raw;
use crate::ui::preview_renderer::{CropHandle, MaskHandle};
use iced::widget::image::Handle;
use iced::{Point, Task};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Phase 95: Simplified editor status (pipeline is now in separate fields)
#[derive(Clone, Debug, PartialEq)]
pub enum EditorReadiness {
    /// No image selected
    NoSelection,
    /// Loading RAW data
    Loading(i64),
    /// Image loaded and ready
    Ready(i64),
    /// Failed to load
    Failed(i64, String),
}

/// Phase 67: Drag mode for mouse interaction
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragMode {
    None,
    Pan,
    CropHandle(CropHandle),
    Crop,
    /// Dragging a handle of the selected local adjustment mask
    MaskHandle(MaskHandle),
}

/// Brush tool settings (UI state, remembered across masks).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushSettings {
    /// Radius as a fraction of the image height.
    pub size: f32,
    pub feather: f32,
    pub flow: f32,
    /// Erase instead of paint (Alt inverts it while held).
    pub erase: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self { size: 0.05, feather: 0.6, flow: 1.0, erase: false }
    }
}

/// Local adjustment mask tool mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskTool {
    Inactive,
    /// Next click on the preview places a linear gradient
    PlacingLinear,
    /// Next click on the preview places a radial gradient
    PlacingRadial,
}

use lru::LruCache;
use std::num::NonZeroUsize;

/// Phase 84: Modal system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    None,
    Help,
    Preferences,
    Export,
    Delete,
    /// Per-category picker shown by Ctrl+C. Its selection lives in
    /// `RawEditor::copy_categories`, mirroring how Export pairs with
    /// `export_settings` and Delete with `pending_delete_ids`.
    CopySettings,
    /// Version / author / links, from Help > About.
    About,
    /// Confirm forgetting a folder. Its target lives in
    /// `RawEditor::pending_remove_folder`.
    RemoveFolder,
}

/// Noise-reduction settings the main view keeps showing while a drag on one
/// of its sliders is in progress — the stage is too slow to re-run per tick
/// (`handlers::loupe`). Tagged with its image so it can never leak onto
/// another one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeldNoise {
    pub image_id: Option<i64>,
    pub luma: f32,
    pub detail: f32,
    pub color: f32,
}

/// The last frame the detail loupe rendered.
#[derive(Debug, Clone)]
pub struct LoupeView {
    pub image_id: i64,
    pub handle: iced::widget::image::Handle,
    /// Its view-UV window — what a drag in loupe pixels moves.
    pub window: [f32; 4],
    /// Physical pixels per side.
    pub size: u32,
}

/// A Develop slider's value being typed: a click on the number swaps it for
/// a text field (`views::develop::slider_field`).
#[derive(Debug, Clone, PartialEq)]
pub struct SliderEntry {
    /// Which slider — `views::develop::slider_key` of its change message.
    pub key: String,
    pub text: String,
}

/// The Develop sidebar's 1:1 detail loupe (`handlers::loupe`, `gpu::loupe`).
#[derive(Debug)]
pub struct LoupeState {
    /// Where it looks, in view UV (the post-crop display image). Kept across
    /// images, like the zoom.
    pub center: [f32; 2],
    pub gpu: Option<std::sync::Arc<crate::gpu::loupe::Loupe>>,
    /// The image whose loupe is being prepared (RAW read, noise model).
    pub building: Option<i64>,
    /// An image whose loupe could not be prepared, and why — not retried.
    pub failed: Option<(i64, String)>,
    pub frame: Option<LoupeView>,
    /// One render at a time; `pending` asks for another when it lands.
    pub in_flight: bool,
    pub pending: bool,
    /// The next click on the image places it.
    pub targeting: bool,
}

impl Default for LoupeState {
    fn default() -> Self {
        Self {
            center: [0.5, 0.5],
            gpu: None,
            building: None,
            failed: None,
            frame: None,
            in_flight: false,
            pending: false,
            targeting: false,
        }
    }
}

/// A lens-database job in progress (`handlers::lensfun`). One at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensfunJob {
    /// Asking lensfun what it currently publishes.
    Checking,
    Downloading,
    /// Re-reading the lens folders (files added by hand).
    Rescanning,
}

/// Lens-profile database downloads and checks, for Preferences › Lens
/// Profiles and the Lens Corrections panel.
#[derive(Debug, Default)]
pub struct LensfunState {
    pub busy: Option<LensfunJob>,
    /// The last check's answer.
    pub remote: Option<Result<crate::core::lensfun::Remote, String>>,
    /// The last download's outcome.
    pub installed: Option<Result<crate::core::lensfun::Installed, String>>,
}

/// A camera-profile job in progress (`handlers::camera_profiles`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileJob {
    Downloading,
    Importing,
    Updating,
    Rescanning,
}

/// Camera-profile downloads, imports and updates.
#[derive(Debug, Default)]
pub struct CameraProfileState {
    pub busy: Option<ProfileJob>,
    /// The last job's outcome, for the panels to report.
    pub notice: Option<Result<String, String>>,
    /// A camera RawTherapee was found to have no profile for — the Develop
    /// prompt says so instead of offering the download again.
    pub unavailable: Option<String>,
}

/// The pages of the Preferences side menu, in menu order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreferencesSection {
    #[default]
    General,
    Performance,
    Memory,
    CameraProfiles,
    LensProfiles,
    Folders,
    Licenses,
}

impl PreferencesSection {
    pub const ALL: [PreferencesSection; 7] = [
        PreferencesSection::General,
        PreferencesSection::Performance,
        PreferencesSection::Memory,
        PreferencesSection::CameraProfiles,
        PreferencesSection::LensProfiles,
        PreferencesSection::Folders,
        PreferencesSection::Licenses,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0)
    }
}

/// A modal on its way out: which one, and a token so a timer from an earlier
/// close can't cut a later one short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModalExit {
    pub modal: Modal,
    pub token: u64,
}

/// Which scrollable a [`ScrollGlide`] drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlideTarget {
    Grid,
    Filmstrip,
}

/// A spring-driven scroll towards the selection (`handlers::scroll`). A
/// spring rather than a fixed-length tween so holding an arrow key — a new
/// target every repeat — keeps one continuous motion instead of restarting.
///
/// The spring runs on the *displacement* from `to`, towards zero, not on the
/// offset itself: offsets reach six figures in a long filmstrip, where `f32`
/// is too coarse for a spring to settle, and a glide that never ended kept
/// issuing `scroll_to` every frame (see `ui::motion::Animated`).
#[derive(Debug, Clone, Copy)]
pub struct ScrollGlide {
    pub target: GlideTarget,
    /// Where the glide comes to rest.
    pub to: f32,
    /// Current offset minus `to`; springs to 0.
    pub displacement: crate::ui::motion::Animated,
    /// How fast `to` is advancing (px/s, signed) while an arrow key is held,
    /// fed forward so the strip keeps pace instead of trailing; 0 otherwise.
    /// See `handlers::scroll::hold_rate`.
    pub rate: f32,
    /// Sign of travel of the current hold.
    pub direction: f32,
    /// When the current hold began; `None` once released.
    pub hold_start: Option<std::time::Instant>,
    /// Distance `to` has travelled this hold, exponentially decayed.
    pub flux: f32,
    /// Retargets so far this hold.
    pub retargets: u32,
    pub last_retarget: Option<std::time::Instant>,
    pub last_tick: Option<std::time::Instant>,
}

impl ScrollGlide {
    pub fn offset(&self) -> f32 {
        self.to + self.displacement.value
    }
}

/// Phase 89: Export Format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Jpeg,
    Png,
    Tiff,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportFormat::Jpeg => write!(f, "JPEG"),
            ExportFormat::Png => write!(f, "PNG"),
            ExportFormat::Tiff => write!(f, "TIFF"),
        }
    }
}

/// Per-image progress within a batch, for the Export tab's queue list.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportJobState {
    Pending,
    Working,
    Done(std::path::PathBuf),
    Failed(String),
}

/// One entry in the export batch. Held in DISPLAY order, separately from
/// `export_queue` (which is the remaining work and is consumed from the back),
/// so the UI can show a stable list while the queue drains.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportJob {
    pub id: i64,
    pub filename: String,
    pub state: ExportJobState,
}

/// Phase 89: Export Settings
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSettings {
    pub format: ExportFormat,
    pub quality: u8,
    pub resize: bool,
    pub max_width: u32,
    pub subfolder: String,
    pub base_path: std::path::PathBuf,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jpeg,
            quality: 80,
            resize: false,
            max_width: 2048,
            subfolder: "Export".to_string(),
            base_path: dirs::picture_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
        }
    }
}

/// A preloaded 1280px working preview, held in the RAM look-ahead cache.
///
/// Stores the decoded pixels, not an `iced` `Handle`. The Develop viewport is a
/// wgpu shader that needs the raw bytes; caching only a `Handle` meant the
/// preloader's decode was thrown away and the JPEG was re-read from disk the
/// moment the user navigated to the image it had just preloaded.
#[derive(Debug, Clone)]
pub struct CachedPreview {
    pub width: u32,
    pub height: u32,
    pub bytes: Arc<[u8]>,
}

impl CachedPreview {
    /// Build an `iced` image handle sharing this entry's allocation.
    ///
    /// `Bytes::from_owner` adopts the `Arc` instead of copying, so the Cull
    /// tab's `Image` widget and the Develop viewport reference the same pixels.
    /// Note `Handle::from_rgba` mints a fresh id each call, so callers should
    /// build this once per cache hit rather than per frame.
    pub fn to_handle(&self) -> Handle {
        Handle::from_rgba(
            self.width,
            self.height,
            iced::advanced::image::Bytes::from_owner(self.bytes.clone()),
        )
    }
}

/// A small render of an image as edited — see `handlers::proxy`.
#[derive(Debug, Clone)]
pub struct EditedProxy {
    pub preview: CachedPreview,
    /// Built once per capture: `Handle::from_rgba` mints a new id per call,
    /// and iced re-uploads per id, so a handle built in `view` would upload
    /// on every frame.
    pub handle: Handle,
    /// The render had edits applied. Only edited proxies replace the
    /// camera's thumbnail — but an unedited one is still the best placeholder
    /// while the image reloads, being exactly what Develop is about to show.
    pub edited: bool,
    /// The edit rendered (`proxy::edit_key`), to tell when it has fallen
    /// behind. `None` for a proxy read from disk until its image is selected.
    pub params: Option<crate::core::types::EditParams>,
    /// Orders captures, so a save that completes after a newer render cannot
    /// mark that newer render as saved.
    pub generation: u64,
    /// The file on disk holds this capture.
    pub persisted: bool,
}

/// Edited proxies kept in memory. ~400 KB each (384px RGBA).
pub const EDITED_PROXY_CAPACITY: usize = 128;

/// Main application state
#[derive(Debug)]
pub struct RawEditor {
    // Phase 84: Active modal overlay
    pub active_modal: Modal,
    /// A modal that has just closed and is still playing its exit (see
    /// `ui::widgets::modal`). Set and cleared centrally in `update`, so no
    /// close path has to remember it.
    pub modal_exit: Option<ModalExit>,
    /// Source of `ModalExit::token`s; never reused, so a stale timer can't
    /// match a later exit.
    pub modal_exit_seq: u64,
    /// Preferences › Reduce motion: every animation jumps to its end state.
    /// Mirrored into `ui::motion::set_reduced`, which the widgets read.
    pub reduce_motion: bool,
    /// Per-camera default profile (`AppSettings::camera_profile_defaults`).
    pub camera_profile_defaults: std::collections::HashMap<String, String>,
    /// The page Preferences shows. Kept while the app runs, so reopening the
    /// dialog lands where the user left it; not persisted.
    pub preferences_section: PreferencesSection,
    /// An in-flight smooth scroll that brings the selection into view.
    pub scroll_glide: Option<ScrollGlide>,
    /// Bumped whenever a title-bar dropdown entry is chosen, which remounts
    /// (and so closes) the menu bar — see `views::layout::view_title_bar`.
    pub menu_generation: u64,

    /// Image IDs the open Delete modal acts on — a stable snapshot taken
    /// when the modal opens, not re-read live from multi_selection.
    pub pending_delete_ids: Vec<i64>,
    /// Guards against a double Enter-press firing two overlapping delete
    /// batches while a hard-delete is already in flight.
    pub is_deleting: bool,

    // Phase 85: User-configurable cache capacity
    pub cache_capacity: usize,

    // Phase 88: Responsive Grid
    pub thumbnail_size: f32,

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
    /// flight, with that edit: `proxy::resolve_awaiting` saves the proxy when
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
    pub dcp_memo: std::cell::RefCell<
        Option<(
            f32,
            crate::core::tonemap::ToneSettings,
            Arc<crate::raw::dcp::InterpolatedProfile>,
        )>,
    >,
    /// As-shot white balance of the current image as (Kelvin, Adobe tint),
    /// solved from the camera's WB multipliers via the color matrix (Robertson
    /// CCT). Anchor for the Temp/Tint sliders; None until a raw is loaded.
    pub as_shot_wb: Option<(f32, f32)>,

    // Phase 95: Unified GPU Pipeline Architecture
    /// Shared GPU context (created once, reused for all images)
    pub gpu_context: Option<Arc<gpu::shared::SharedContext>>,
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
    /// Phase 21: Histogram canvas cache
    pub histogram_cache: iced::widget::canvas::Cache,
    /// Phase 22: Histogram toggle (keep for user control)
    pub histogram_enabled: bool,
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
    /// Phase 25: Canvas cache for main image rendering
    pub canvas_cache: iced::widget::canvas::Cache,
    /// Phase 25: Drag state for panning
    pub is_dragging: bool,
    pub last_cursor_position: Option<Point>,
    /// Phase 26: Double-click detection
    pub last_click_time: Option<std::time::Instant>,
    /// Double-click detection for the title bar drag surface (toggles maximize).
    /// Separate from `last_click_time`, which is scoped to the preview canvas.
    pub last_titlebar_click: Option<std::time::Instant>,
    /// Phase 26: Viewport size for zoom-to-cursor calculations (actual displayed size)
    pub viewport_size: (f32, f32), // (width, height) in screen pixels
    /// Set by the per-tick edit path, cleared by `flush_edits` at commit
    /// points. Keeps SQLite writes off the slider-drag hot path.
    pub edits_dirty: bool,
    pub scale_factor: f32,
    /// Phase 29: Instant preview handle (displayed while RAW loads)
    pub working_preview: Option<Handle>,
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
    /// Phase 55: Track modifier keys for Ctrl/Cmd+Click
    pub last_modifiers: iced::keyboard::Modifiers,
    /// Pivot for Shift+click range selection: the last image picked WITHOUT
    /// shift. Held separately from `selected_image_id` because a range drag
    /// moves the current selection while the pivot must stay put.
    pub selection_anchor: Option<i64>,
    /// Phase 83: Auto-advance to next image after rating/flagging
    pub auto_advance: bool,
    /// Phase 59: Minimum rating filter (0 = show all, 1-5 = show rating or higher)
    pub min_filter_rating: u8,
    /// Library folder filter (path prefix)
    pub library_folder_filter: Option<String>,
    /// Folder the open RemoveFolder dialog acts on — a snapshot taken when the
    /// dialog opens, mirroring `pending_delete_ids`.
    pub pending_remove_folder: Option<String>,
    /// Phase 79: Modular Info Overlay state
    pub info_overlay: InfoOverlayState,
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
    /// The Develop slider whose value is being typed, if any.
    pub slider_entry: Option<SliderEntry>,
    /// Local adjustment mask tool (placement mode). Mutually exclusive with
    /// crop and WB picker.
    pub mask_tool: MaskTool,
    /// Index into current_edit_params.masks of the mask being edited
    /// (overlay handles + sidebar sliders). None = no mask selected.
    pub selected_mask: Option<usize>,
    /// Brush tool settings for painting masks (UI state).
    pub brush_settings: BrushSettings,
    /// Painting into the selected mask is armed (always on for brush masks).
    pub brush_armed: bool,
    /// A stroke is in progress.
    pub is_painting: bool,
    /// Tint the selected mask's coverage over the render.
    pub show_mask_overlay: bool,
    /// The colour-range eyedropper is armed for this mask.
    pub color_picking_mask: Option<usize>,
    /// Collapsed Develop sidebar sections (UI state).
    pub collapsed_sections: HashSet<crate::app::message::SidebarSection>,
    /// Which point curve the curve editor is showing (UI state, not an edit).
    pub curve_channel: crate::core::curves::CurveChannel,
    /// Phase 67: Drag mode for interaction
    pub drag_mode: DragMode,
    /// Phase 78: Async Task Deduplication (track pending preview loads)
    pub pending_loads: HashSet<i64>,
    /// Phase 81: Throttled preview loader queue
    pub queued_loads: Vec<(i64, String)>,
    /// Pending RAW preload jobs
    pub pending_raw_loads: HashSet<i64>,
    /// Throttled RAW preload queue
    pub queued_raw_loads: Vec<(i64, String)>,

    /// Phase 104: Performance Profiler
    pub profiler: crate::core::profiler::Profiler,
    pub show_profiler: bool,
    /// Phase 105: Cache for profiler canvas (cleared on each new frame push)
    pub profiler_cache: iced::widget::canvas::Cache,

    /// Phase 106: Render Throttling
    pub is_rendering: bool,
    pub pending_render: bool,
    /// True while a full-resolution resource rebuild is in flight. One at a
    /// time, across images: each holds a full-size working set on the GPU,
    /// and navigating quickly must not stack them. Cleared only by that
    /// rebuild's own result (`Message::FullResReady`).
    pub full_res_upgrading: bool,
    /// The image whose full-resolution rebuild last failed (out of GPU
    /// memory, say): not retried on every render while it stays selected.
    pub full_res_failed: Option<i64>,
    /// A noise-reduction slider is being dragged (set by its change handlers,
    /// cleared by `commit_current_state` — the slider's release). While set,
    /// a zoomed-in view renders from `preview_resources` so the drag stays
    /// interactive; the release re-renders at full resolution.
    pub stage_drag: bool,
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
    pub camera_profiles: CameraProfileState,

    // ── Scroll boost + filmstrip momentum ──────────────────────────────────
    /// True while the cursor is over the library grid, from the grid's own
    /// `mouse_area` enter/exit. Routes the wheel boost. This replaced a global
    /// CursorMoved subscription, which sent a message — and so rebuilt the
    /// entire view, every thumbnail included — on every single mouse move.
    pub cursor_over_grid: bool,
    /// Library grid scroll offset (px), from the scrollable's `on_scroll`.
    /// The grid is virtualised: only rows near this offset are built.
    pub grid_scroll_y: f32,
    /// Visible size of the grid's scroll viewport (px), also from
    /// `on_scroll`. `(0, 0)` until the first report — see `window_size`.
    pub grid_viewport: (f32, f32),
    /// Window size, from Opened/Resized events (rare, so cheap). Only used to
    /// estimate the grid viewport for the first frame, before it reports in.
    pub window_size: iced::Size,
    /// Filmstrip scroll offset and visible width (px), from `on_scroll`.
    pub filmstrip_scroll_x: f32,
    pub filmstrip_view_w: f32,
    /// Filmstrip momentum velocity in px/s along the scroll axis; 0.0 = idle
    /// (the kinetic `window::frames()` subscription only runs while nonzero).
    pub filmstrip_velocity: f32,
    pub last_kinetic_tick: Option<std::time::Instant>,
    /// Timestamp of the last wheel event over the filmstrip, used to measure
    /// how fast the user is actually spinning the wheel (deltaT-based).
    pub last_wheel_time: Option<std::time::Instant>,
    /// Smoothed estimate of wheel input speed, px/s. Drives both the size of
    /// the velocity impulse added per tick and the glide's "weight" (decay).
    pub filmstrip_input_speed: f32,
    /// Decay rate (1/s) for the current glide, chosen from input speed at the
    /// moment of the last wheel tick: slow input -> low decay (heavy, glides
    /// a long time); fast input -> high decay (light, stops quickly so it
    /// stays controllable).
    pub filmstrip_decay: f32,
}

/// Phase 79: Modular Info Overlay States
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InfoOverlayState {
    #[default]
    Hidden,
    Metadata,
    CacheDebug,
}

impl InfoOverlayState {
    pub fn next(&self) -> Self {
        match self {
            Self::Hidden => Self::Metadata,
            Self::Metadata => Self::CacheDebug,
            Self::CacheDebug => Self::Hidden,
        }
    }
}

/// Pixel size of the `crop` sub-rect (normalised `[x, y, w, h]`) of a `w`×`h`
/// image, never collapsing to zero — `fitted_size` divides by it.
pub fn crop_dims(w: u32, h: u32, crop: [f32; 4]) -> (u32, u32) {
    (
        ((w as f32 * crop[2]).round() as u32).max(1),
        ((h as f32 * crop[3]).round() as u32).max(1),
    )
}

/// True when `path` lies inside `folder` or any of its subfolders.
///
/// Component-wise, not a string prefix: `/photos/2024` must not match
/// `/photos/2024-old/a.NEF`, which a bare `str::starts_with` happily did —
/// clicking one folder in the sidebar also showed its similarly-named siblings.
pub fn path_is_within(path: &str, folder: &str) -> bool {
    std::path::Path::new(path).starts_with(folder)
}

pub const RAW_CACHE_ENTRY_CAPACITY: usize = 4096;
pub const PREVIEW_PRELOAD_BEHIND_MAX: usize = 30;
pub const PREVIEW_PRELOAD_AHEAD_MAX: usize = 100;
pub const RAW_PRELOAD_BEHIND_MAX: usize = 10;
pub const RAW_PRELOAD_AHEAD_MAX: usize = 20;
/// Upper bound for the free-RAM headroom slider. Above this the setting would
/// pause exports permanently on any ordinary machine.
pub const MIN_FREE_RAM_MB_MAX: u32 = 16384;
/// Shortest time the startup splash stays on screen.
pub const SPLASH_MIN: std::time::Duration = std::time::Duration::from_secs(1);

impl RawEditor {
    pub fn title(&self) -> String {
        String::from("RAW Editor")
    }

    /// Phase 23: Create a new instance of the application (INSTANT!)
    /// The database now loads in the background to show splash screen immediately
    pub fn new() -> (Self, Task<Message>) {
        tracing::info!("RAW Editor starting (instant splash screen)...");

        let settings = crate::core::settings::AppSettings::load_or_default();
        // Not under test: the flag is process-global, tests build editors in
        // parallel, and this reads the developer's real settings file.
        if !cfg!(test) {
            crate::ui::motion::set_reduced(settings.reduce_motion);
        }
        let cache_capacity = settings.cache_capacity.clamp(20, 500);
        let raw_preload_budget_mb = settings.raw_preload_budget_mb.min(4096);
        let thumbnail_size = settings.thumbnail_size.clamp(100.0, 400.0);
        let preview_preload_behind = settings
            .preview_preload_behind
            .min(PREVIEW_PRELOAD_BEHIND_MAX);
        let preview_preload_ahead = settings
            .preview_preload_ahead
            .min(PREVIEW_PRELOAD_AHEAD_MAX);
        let raw_preload_behind = settings.raw_preload_behind.min(RAW_PRELOAD_BEHIND_MAX);
        let raw_preload_ahead = settings.raw_preload_ahead.min(RAW_PRELOAD_AHEAD_MAX);

        // Determine the database path (e.g., in the application's data directory)
        let _db_path = database::library::Library::get_db_path();

        (
            RawEditor {
                library: None, // Phase 23: Database loads in background
                status: "Initializing...".to_string(),
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
                image_resources: None,
                preview_resources: None,
                editor_readiness: EditorReadiness::NoSelection,

                histogram_data: std::cell::RefCell::new([[0; 256]; 3]),
                histogram_cache: iced::widget::canvas::Cache::default(),
                histogram_enabled: settings.histogram_enabled,
                before_peek: false,
                zoom: 1.0,
                pan_offset: cgmath::Vector2::new(0.0, 0.0),
                canvas_cache: iced::widget::canvas::Cache::default(),
                is_dragging: false,
                last_cursor_position: None,
                last_click_time: None,
                last_titlebar_click: None,
                viewport_size: (800.0, 400.0),
                edits_dirty: false,
                scale_factor: 1.0,
                working_preview: None,
                working_preview_bytes: None,
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
                last_modifiers: iced::keyboard::Modifiers::default(),
                selection_anchor: None,
                auto_advance: settings.auto_advance,
                min_filter_rating: 0,
                library_folder_filter: None,
                pending_remove_folder: None,
                // Must be the enum's own default (Hidden). Starting at
                // Metadata booted the app with the overlay already on, at
                // phase 2 of 3, so the first `I` press advanced the cycle
                // instead of opening it.
                info_overlay: crate::app::state::InfoOverlayState::default(),
                // Phase 84
                active_modal: Modal::None,
                pending_delete_ids: Vec::new(),
                is_deleting: false,
                // Phase 85
                cache_capacity,
                // Phase 88
                thumbnail_size,
                // Phase 89
                export_settings: ExportSettings::default(),
                export_queue: Vec::new(),
                export_jobs: Vec::new(),
                is_exporting: false,

                // Phase 104/105: Profiler
                profiler: crate::core::profiler::Profiler::new(),
                show_profiler: false,
                profiler_cache: iced::widget::canvas::Cache::default(),
                is_rendering: false,
                pending_render: false,
                full_res_upgrading: false,
                full_res_failed: None,
                stage_drag: false,
                display_from_drag_preview: false,
                held_noise: None,
                noise_settle_token: 0,
                loupe: LoupeState::default(),
                lensfun: LensfunState::default(),
                camera_profiles: CameraProfileState::default(),
                cursor_over_grid: false,
                grid_scroll_y: 0.0,
                grid_viewport: (0.0, 0.0),
                window_size: iced::Size::new(1920.0, 1080.0),
                filmstrip_scroll_x: 0.0,
                filmstrip_view_w: 0.0,
                filmstrip_velocity: 0.0,
                last_kinetic_tick: None,
                modal_exit: None,
                modal_exit_seq: 0,
                reduce_motion: settings.reduce_motion,
                camera_profile_defaults: settings.camera_profile_defaults.clone(),
                preferences_section: PreferencesSection::default(),
                scroll_glide: None,
                menu_generation: 0,
                last_wheel_time: None,
                filmstrip_input_speed: 0.0,
                filmstrip_decay: 4.0,

                // Background task
                // background_task: None, // This field is not defined in the struct
                history_map: HashMap::new(),
                is_cropping: false,
                is_wb_picking: false,
                is_leveling: false,
                slider_entry: None,
                mask_tool: MaskTool::Inactive,
                selected_mask: None,
                curve_channel: crate::core::curves::CurveChannel::Master,
                brush_settings: BrushSettings::default(),
                brush_armed: false,
                is_painting: false,
                show_mask_overlay: false,
                color_picking_mask: None,
                // Lens and defringe start folded: they are set-once tools, and
                // the sidebar is long enough without them open.
                collapsed_sections: [
                    crate::app::message::SidebarSection::Lens,
                    crate::app::message::SidebarSection::Defringe,
                ]
                .into_iter()
                .collect(),
                drag_mode: DragMode::None,
                pending_loads: HashSet::new(),
                queued_loads: Vec::new(),
                pending_raw_loads: HashSet::new(),
                queued_raw_loads: Vec::new(),
            },
            // The splash stays up for at least SPLASH_MIN, however fast the
            // database opens: on a small catalogue it otherwise flashed for a
            // frame. The load runs meanwhile, so a slow one costs nothing extra.
            Task::perform(
                async {
                    let shown_until = tokio::time::Instant::now() + SPLASH_MIN;
                    let loaded = database::library::load_database().await;
                    tokio::time::sleep_until(shown_until).await;
                    loaded
                },
                Message::DatabaseLoaded,
            ),
        )
    }

    // Phase 65: Undo/Redo Helper
    // Returns a mutable reference to the (Stack, Index) tuple for the current image.
    // Initializes it if missing.
    pub fn get_current_history(
        &mut self,
    ) -> Option<&mut (Vec<crate::core::types::EditParams>, usize)> {
        if let Some(image_id) = self.selected_image_id {
            // Ensure entry exists
            self.history_map.entry(image_id).or_insert_with(|| {
                // Initial state is the current params
                (vec![self.current_edit_params], 0)
            });

            self.history_map.get_mut(&image_id)
        } else {
            None
        }
    }

    /// The params the main view renders: the user's edit, except that noise
    /// reduction stays where it was while a drag on its sliders is held back
    /// (`held_noise`). Everything that pushes the edit to the GPU uses this;
    /// what is saved, exported and undone is `current_edit_params`.
    pub fn view_params(&self) -> crate::core::types::EditParams {
        let mut p = self.current_edit_params;
        if let Some(h) = self.held_noise.filter(|h| h.image_id == self.selected_image_id) {
            p.luma_noise = h.luma;
            p.luma_detail = h.detail;
            p.color_noise = h.color;
        }
        p
    }

    /// Helper to push current state to history.
    ///
    /// Also flushes pending edits to SQLite: a history commit marks the end of
    /// a gesture, which is exactly when the params are worth persisting. Tying
    /// the two together means any future commit site gets persistence for free
    /// and the two can't drift apart.
    pub fn commit_current_state(&mut self) {
        self.stage_drag = false;
        self.flush_edits();
        let params_to_push = self.current_edit_params;
        if let Some((stack, index)) = self.get_current_history() {
            // A release that changed nothing — clicking a slider without
            // dragging, grabbing a crop handle and letting go — must not add a
            // step, or the next Undo visibly does nothing. `is_cropping` is
            // tool state, so entering crop mode alone isn't an edit either.
            let unchanged = stack
                .get(*index)
                .is_some_and(|top| top.without_tool_state() == params_to_push.without_tool_state());
            if unchanged {
                return;
            }

            // Truncate any redo history
            stack.truncate(*index + 1);

            // Push new state
            stack.push(params_to_push);
            *index += 1;

            tracing::debug!(
                "History Commit: Stack size {}, Index {}",
                stack.len(),
                *index
            );
        }
    }

    /// Mirror the crop tool's on/off state into the params the shader reads.
    ///
    /// `is_cropping` rides in `EditParams` only because that is what reaches
    /// the GPU. Anything that swaps the params wholesale — undo/redo, reset,
    /// loading another image's edits — would otherwise leave the shader and
    /// the crop overlay disagreeing about whether crop mode is on.
    pub fn sync_tool_flags(&mut self) {
        self.current_edit_params.is_cropping = u32::from(self.is_cropping);
        // The mask overlay shows while the user asks for it, or while a
        // stroke is being painted (you need to see what you are painting).
        self.current_edit_params.mask_overlay = match self.selected_mask {
            Some(i) if (self.show_mask_overlay || self.is_painting)
                && i < self.current_edit_params.mask_count as usize =>
            {
                i as u32 + 1
            }
            _ => 0,
        };
    }

    /// True while the mask tool wants the preview canvas (placement mode or a
    /// selected mask with visible handles) — suppresses panning, like crop.
    pub fn mask_overlay_active(&self) -> bool {
        self.mask_tool != MaskTool::Inactive || self.selected_mask.is_some()
    }

    /// Size of the pixel buffer the develop viewport currently holds.
    ///
    /// This is the RENDER TARGET size, which since the windowed render is no
    /// longer the image's shape: at zoom it covers only the visible slice.
    /// Use it to size textures and nothing else — for anything that reasons
    /// about where the image sits on screen, use `image_display_dims`.
    /// MUST mirror the fallback priority in `views/develop.rs`
    /// (rendered > working > placeholder).
    pub fn buffer_dims(&self) -> (u32, u32) {
        if self.rendered_preview_bytes.is_some() {
            self.rendered_preview_dims
        } else if self.working_preview_bytes.is_some() {
            self.working_preview_dims
        } else {
            (1280, 853)
        }
    }

    /// The images the CURRENT TAB is showing, in display order.
    ///
    /// The one place the per-tab filter predicates live. They differ, and the
    /// differences are deliberate:
    ///
    /// | Tab | rating filter | folder filter | marked for removal |
    /// |-----|---------------|---------------|--------------------|
    /// | Library | yes | yes | visible |
    /// | Cull | yes | no | visible |
    /// | Develop | yes | no | **excluded** |
    ///
    /// `MARKED_FOR_REMOVAL_RATING` is 255, which satisfies `>=` against any
    /// real 0-5 threshold, so marked images survive the rating filter — that
    /// is intentional, and why Develop needs its own explicit exclusion.
    ///
    /// Anything that reasons about "the images on screen" must come through
    /// here: the three views that render lists, range selection, select-all,
    /// and arrow-key navigation. Re-inlining the predicate is how the four
    /// drift apart.
    pub fn displayed_images(&self) -> Vec<&ImageData> {
        let marked = crate::database::models::MARKED_FOR_REMOVAL_RATING;
        self.images
            .iter()
            .filter(|img| self.min_filter_rating == 0 || img.rating >= self.min_filter_rating)
            .filter(|img| {
                if self.current_tab != AppTab::Library {
                    return true;
                }
                self.library_folder_filter
                    .as_ref()
                    .map(|folder| path_is_within(&img.path, folder))
                    .unwrap_or(true)
            })
            // Marked-for-removal is hidden where acting on it would be wrong:
            // Develop refuses to edit them, and Export refuses to write out
            // images the user has flagged for deletion.
            .filter(|img| {
                !matches!(self.current_tab, AppTab::Develop | AppTab::Export)
                    || img.rating != marked
            })
            .collect()
    }

    /// RAW metadata for the selected image, or `None` when what is held
    /// belongs to some other image (or nothing has been loaded yet).
    pub fn selected_metadata(&self) -> Option<&raw::loader::RawDataResult> {
        if self.current_metadata_id.is_some() && self.current_metadata_id == self.selected_image_id
        {
            self.current_metadata.as_ref()
        } else {
            None
        }
    }

    /// Ids of [`Self::displayed_images`], in the same order.
    pub fn displayed_ids(&self) -> Vec<i64> {
        self.displayed_images().iter().map(|i| i.id).collect()
    }

    /// The images a rate / flag / delete action applies to: the whole
    /// multi-selection when there is one, else the current image.
    pub fn action_target_ids(&self) -> Vec<i64> {
        if !self.multi_selection.is_empty() {
            self.multi_selection.iter().copied().collect()
        } else {
            self.selected_image_id.into_iter().collect()
        }
    }

    /// Apply `f` to the in-memory catalogue rows for `ids`.
    ///
    /// One pass over the catalogue with a set lookup — the per-id
    /// `iter().find()` this replaces was O(selection × catalogue) on every
    /// select-all rating.
    pub fn apply_to_images(&mut self, ids: &[i64], mut f: impl FnMut(&mut ImageData)) {
        let ids: HashSet<i64> = ids.iter().copied().collect();
        for img in self.images.iter_mut().filter(|img| ids.contains(&img.id)) {
            f(img);
        }
    }

    /// The thumbnail to show for `img`: its edited proxy when it has edits,
    /// the camera's embedded preview otherwise.
    ///
    /// An in-memory proxy is authoritative — it is the newest render. When it
    /// says "unedited" (the edit was just reset) the camera thumbnail wins
    /// even over a saved edited one that has not been cleared yet.
    pub fn thumbnail_handle(&self, img: &ImageData) -> Option<Handle> {
        if let Some(proxy) = self.edited_proxies.peek(&img.id) {
            if proxy.edited {
                return Some(proxy.handle.clone());
            }
            return img.cache_path_thumb.as_ref().map(Handle::from_path);
        }
        img.cache_path_edited
            .as_ref()
            .or(img.cache_path_thumb.as_ref())
            .map(Handle::from_path)
    }

    /// Forget the edited proxies of `ids`, whose saved edits just changed
    /// without being rendered — see `handlers::proxy`. Their thumbnails fall
    /// back to the camera's until they are next developed. The stale files
    /// are swept at the next launch (`Library::sweep_edited_proxies`).
    pub fn invalidate_proxies(&mut self, ids: &[i64]) {
        for id in ids {
            self.edited_proxies.pop(id);
        }
        let had_saved: Vec<i64> = self
            .images
            .iter()
            .filter(|img| img.cache_path_edited.is_some() && ids.contains(&img.id))
            .map(|img| img.id)
            .collect();
        if had_saved.is_empty() {
            return;
        }
        if let Some(lib) = &self.library {
            if let Err(e) = lib.clear_edited_proxies(&had_saved) {
                tracing::warn!("Could not clear stale edited thumbnails: {e}");
            }
        }
        self.apply_to_images(&had_saved, |img| img.cache_path_edited = None);
    }

    /// True when the selected image's edits are being rendered, so its proxy
    /// follows them on its own.
    fn renders_selected_image(&self) -> bool {
        self.current_tab == AppTab::Develop
            && self.selected_image_id.is_some()
            && self.image_resources.as_ref().map(|r| r.image_id) == self.selected_image_id
    }

    /// True when this image should render as selected.
    ///
    /// The single predicate for both the library grid and the filmstrip. They
    /// used to disagree — the grid ORed `selected_image_id` with
    /// `multi_selection`, the filmstrip read only `multi_selection` — which
    /// happened to look consistent only because a plain click kept the two in
    /// sync. Range selection breaks that assumption.
    pub fn is_selected(&self, image_id: i64) -> bool {
        self.selected_image_id == Some(image_id) || self.multi_selection.contains(&image_id)
    }

    /// Display-space (EXIF-oriented) dimensions of what is actually ON SCREEN.
    ///
    /// The single dims source for all on-screen geometry: letterbox fit,
    /// zoom-to-cursor, crop and mask drag math, and the canvas overlay. Only
    /// the ASPECT actually matters to `core::viewport::image_rect`, which is
    /// why taking it from the GPU resources is safe even while a preview of a
    /// different resolution is on screen — and why it must not come from the
    /// render buffer, whose aspect now tracks the viewport rather than the
    /// image.
    ///
    /// **Crop-aware.** Outside crop mode the viewport shows the crop sub-rect,
    /// so these are the CROPPED dimensions. Returning the full frame's here
    /// made the letterbox reserve the uncropped aspect and the shader stretch
    /// the crop to fill it — a 16:9 crop of a 3:2 frame rendered vertically
    /// squashed. (Export was unaffected: it renders the crop to its own
    /// correctly-sized target.) In crop mode the shader deliberately reveals
    /// the whole sensor so the handles can be dragged over it, so the full
    /// frame genuinely is what's displayed and the crop must NOT be applied.
    ///
    /// **Before the GPU resources exist** (the RAW is still loading) what is on
    /// screen is the placeholder, shown in its own shape: an edited proxy is
    /// already cropped, and the camera JPEG never is. Applying the crop here
    /// squeezed the full camera frame into the crop's aspect for as long as
    /// the load took — and would crop an already-cropped proxy a second time.
    pub fn image_display_dims(&self) -> (u32, u32) {
        let Some(res) = &self.image_resources else {
            return if self.working_preview_bytes.is_some() {
                self.working_preview_dims
            } else {
                (1280, 853)
            };
        };
        let (w, h) = res.oriented_dims(self.current_edit_params.quarter_turns);
        if self.is_cropping {
            return (w, h);
        }
        crop_dims(w, h, self.current_edit_params.crop)
    }

    /// Magnification as a photographer reads it: 100% means one full-resolution
    /// sensor pixel per physical screen pixel.
    ///
    /// Distinct from `self.zoom`, which is relative to fit — at fit a 6000px
    /// image on a 1900px viewport is `zoom == 1.0` but only ~32%.
    pub fn zoom_percent(&self) -> f32 {
        let (dw, dh) = self.image_display_dims();
        let (vw, vh) = self.viewport_size;
        let (fw, _) = crate::core::viewport::fitted_size(dw, dh, vw, vh);
        // Against the FULL-resolution width: the develop texture may be a
        // subsampled preview, and reporting against that would claim 100%
        // while showing half the detail.
        let full_w = self
            .image_resources
            .as_ref()
            .map(|r| r.oriented_original_dims(self.current_edit_params.quarter_turns).0)
            .unwrap_or(dw)
            .max(1) as f32;
        // `fw` spans the CROPPED region (image_display_dims is crop-aware), so
        // the sensor pixels it covers are only that fraction of the full width.
        let visible_w = if self.is_cropping {
            full_w
        } else {
            (full_w * self.current_edit_params.crop[2]).max(1.0)
        };
        fw * self.zoom * self.scale_factor / visible_w * 100.0
    }

    /// Helper to save current edit parameters to database
    /// Signal that the preview pixel buffer's contents changed, so the viewport
    /// shader re-uploads its texture on the next redraw. Must be called
    /// wherever `working_preview_bytes` or `rendered_preview_bytes` is
    /// replaced — a missed call shows a stale image.
    pub fn bump_preview_generation(&mut self) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        // 0 is reserved for the shader's 1x1 placeholder.
        if self.preview_generation == 0 {
            self.preview_generation = 1;
        }
    }

    /// Mark the current image's edits as needing persistence.
    ///
    /// Called from the per-slider-tick path (`update_pipeline`). Writing to
    /// SQLite there directly meant a JSON serialize + SELECT + UPDATE + fsync
    /// per mouse-move event, on the UI thread; the flag defers that to the
    /// commit points below.
    pub fn mark_edits_dirty(&mut self) {
        self.edits_dirty = true;
    }

    /// Persist the current image's edits if anything changed since the last
    /// flush. Cheap and safe to call unconditionally at any commit point.
    ///
    /// Flush points must cover every way the current params can stop being
    /// current: slider release (`CommitEdit`), drag release, image switch, tab
    /// switch, and window close. Missing one loses the last gesture's edits.
    pub fn flush_edits(&mut self) {
        if !self.edits_dirty {
            return;
        }
        // Clear first: a failed write should not spin on every later flush.
        self.edits_dirty = false;
        self.save_current_edits();
        // Edits saved with no render behind them (undo, reset or paste in
        // Library or Cull): the image's proxy now shows something else.
        if !self.renders_selected_image() {
            if let Some(id) = self.selected_image_id {
                self.invalidate_proxies(&[id]);
            }
        }
    }

    /// Unconditional write, bypassing the dirty flag. Prefer `flush_edits`;
    /// this exists for callers that write params for an image other than the
    /// selected one, or that must persist regardless of flag state.
    pub fn save_current_edits(&self) {
        // Phase 23: Only save if database is loaded
        if let Some(library) = &self.library {
            if let Some(image_id) = self.selected_image_id {
                if let Err(e) = library.save_edit_params(image_id, &self.current_edit_params) {
                    tracing::error!("Failed to save edits for image {}: {:?}", image_id, e);
                }
            }
        }
    }

    /// Phase 66: Helper to calculate a center crop for a target aspect ratio
    /// Returns [x, y, w, h] in normalized coordinates (0.0 to 1.0)
    pub fn calculate_center_crop(target_ratio: f32, image_w: u32, image_h: u32) -> [f32; 4] {
        let image_ratio = image_w as f32 / image_h as f32;

        if image_ratio > target_ratio {
            // Image is wider than target: Crop width (sides)
            // h = 1.0, w = target / image
            let w = target_ratio / image_ratio;
            let x = (1.0 - w) / 2.0;
            [x, 0.0, w, 1.0]
        } else {
            // Image is taller than target: Crop height (top/bottom)
            // w = 1.0, h = image / target
            let h = image_ratio / target_ratio;
            let y = (1.0 - h) / 2.0;
            [0.0, y, 1.0, h]
        }
    }

    fn raw_cache_entry_bytes(raw: &raw::loader::RawDataResult) -> usize {
        raw.data.len()
            .saturating_mul(std::mem::size_of::<u16>())
            .saturating_add(1024)
    }

    pub fn raw_cache_budget_bytes(&self) -> usize {
        (self.raw_preload_budget_mb as usize).saturating_mul(1024 * 1024)
    }

    /// Headroom to keep free system-wide, in bytes. `0` = check disabled.
    /// Mirrors `raw_cache_budget_bytes`'s shape deliberately.
    pub fn min_free_bytes(&self) -> u64 {
        (self.min_free_ram_mb as u64).saturating_mul(1024 * 1024)
    }

    /// Drop everything regenerable the app is holding, and let wgpu hand back
    /// what has already been released.
    ///
    /// Called under memory pressure. These caches exist purely to make
    /// navigation fast; rebuilding them costs time, whereas running the machine
    /// out of RAM costs the user their session.
    pub fn release_discretionary_memory(&mut self) {
        self.preview_cache.clear();
        self.raw_cache.clear();
        self.raw_cache_bytes = 0;
        if let Some(ctx) = &self.gpu_context {
            ctx.reclaim();
        }
    }

    pub fn evict_raw_cache_to_budget(&mut self) {
        let budget = self.raw_cache_budget_bytes();
        while self.raw_cache_bytes > budget {
            if let Some((_id, raw)) = self.raw_cache.pop_lru() {
                self.raw_cache_bytes = self
                    .raw_cache_bytes
                    .saturating_sub(Self::raw_cache_entry_bytes(&raw));
            } else {
                self.raw_cache_bytes = 0;
                break;
            }
        }
    }

    pub fn insert_raw_cache(&mut self, image_id: i64, raw: Arc<raw::loader::RawDataResult>) {
        if self.raw_preload_budget_mb == 0 {
            return;
        }

        if let Some(old) = self.raw_cache.get(&image_id) {
            self.raw_cache_bytes = self
                .raw_cache_bytes
                .saturating_sub(Self::raw_cache_entry_bytes(old));
        }

        self.raw_cache_bytes = self
            .raw_cache_bytes
            .saturating_add(Self::raw_cache_entry_bytes(&raw));
        self.raw_cache.put(image_id, raw);
        self.evict_raw_cache_to_budget();
    }

    /// Drop a deleted image's entry from the raw cache, keeping
    /// `raw_cache_bytes` accounting correct.
    pub fn remove_from_raw_cache(&mut self, image_id: i64) {
        if let Some(old) = self.raw_cache.pop(&image_id) {
            self.raw_cache_bytes = self
                .raw_cache_bytes
                .saturating_sub(Self::raw_cache_entry_bytes(&old));
        }
    }

    pub fn save_preferences(&self) {
        let settings = crate::core::settings::AppSettings {
            cache_capacity: self.cache_capacity,
            raw_preload_budget_mb: self.raw_preload_budget_mb,
            thumbnail_size: self.thumbnail_size,
            auto_advance: self.auto_advance,
            histogram_enabled: self.histogram_enabled,
            preview_preload_behind: self.preview_preload_behind,
            preview_preload_ahead: self.preview_preload_ahead,
            raw_preload_behind: self.raw_preload_behind,
            raw_preload_ahead: self.raw_preload_ahead,
            copy_categories: self.copy_categories,
            min_free_ram_mb: self.min_free_ram_mb,
            reduce_motion: self.reduce_motion,
            camera_profile_defaults: self.camera_profile_defaults.clone(),
        };

        if let Err(e) = settings.save() {
            tracing::warn!("Failed to save app settings: {}", e);
        }
    }

    /// Phase 24: Keyboard shortcuts subscription
    /// Phase 81: Throttled Image Loader subscription
    pub fn subscription(&self) -> iced::Subscription<Message> {
        use iced::keyboard;
        use iced::keyboard::key::Named;

        let keyboard_subscription = iced::event::listen_with(|event, status, _window| {
            // Phase 55: Track modifier key changes for Ctrl/Cmd+Click
            if let iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
                return Some(Message::ModifiersChanged(modifiers));
            }

            // A focused widget already consumed this key — typing into the
            // export subfolder field, or a hovered slider's arrow keys. Without
            // this, Backspace in a text field opened the Delete dialog, `r`
            // reset the open image's edits and digits re-rated the selection.
            if status == iced::event::Status::Captured {
                if let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) = &event {
                    // A text field swallows Escape (it just unfocuses), but in a
                    // slider's typed value it should also give the number back.
                    // A no-op when no slider is being typed into.
                    if matches!(key, keyboard::Key::Named(Named::Escape)) {
                        return Some(Message::SliderEntryCancel);
                    }
                    return None;
                }
            }

            // Scroll boost + filmstrip momentum: routed by cursor region in
            // handlers::scroll, so we just forward these globally.
            if let iced::Event::Mouse(iced::mouse::Event::WheelScrolled { delta }) = event {
                return Some(Message::WheelScrolled(delta));
            }
            if let iced::Event::Window(
                iced::window::Event::Resized(size) | iced::window::Event::Opened { size, .. },
            ) = event
            {
                return Some(Message::WindowResized(size));
            }
            // Alt-tabbing away while Space is held sends the KeyReleased to
            // the other window, which would otherwise strand the peek on.
            if let iced::Event::Window(iced::window::Event::Unfocused) = event {
                return Some(Message::SetBeforePeek(false));
            }
            // Hold-to-peek and the arrow-key follow scroll need the release
            // edge; every other shortcut here is press-only.
            if let iced::Event::Keyboard(keyboard::Event::KeyReleased { key, .. }) = &event {
                if matches!(key, keyboard::Key::Named(Named::Space)) {
                    return Some(Message::SetBeforePeek(false));
                }
                // The end of a held arrow key: the follow scroll coasts to rest.
                if matches!(
                    key,
                    keyboard::Key::Named(Named::ArrowLeft | Named::ArrowRight)
                ) {
                    return Some(Message::NavigationKeyReleased);
                }
                return None;
            }

            if let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event
            {
                // Phase 54: Settings Clipboard (Ctrl/Cmd+C/V)
                if modifiers.command() {
                    match key {
                        // Shift+C skips the picker and copies everything, so
                        // the common case keeps zero friction.
                        keyboard::Key::Character(c) if c == "c" || c == "C" => {
                            return Some(if modifiers.shift() {
                                Message::CopyAllSettings
                            } else {
                                Message::CopySettings
                            });
                        }
                        keyboard::Key::Character(c) if c == "v" || c == "V" => {
                            return Some(Message::PasteSettings);
                        }
                        // Must live in the command block with an early return:
                        // the fall-through match below would make a bare `a`
                        // select everything.
                        keyboard::Key::Character(c) if c == "a" || c == "A" => {
                            return Some(Message::SelectAll);
                        }
                        _ => {}
                    }
                }

                match key {
                    keyboard::Key::Named(Named::F3) => Some(Message::ToggleProfiler),
                    // Nothing opened the shortcut list from the keyboard,
                    // which is a poor joke for a shortcut list.
                    keyboard::Key::Named(Named::F1) => {
                        Some(Message::OpenModal(Modal::Help))
                    }
                    keyboard::Key::Character(c) if c == "?" || c == "/" && modifiers.shift() => {
                        Some(Message::OpenModal(Modal::Help))
                    }
                    keyboard::Key::Named(Named::Escape) => Some(Message::Escape),
                    // Hold to peek at the original; the KeyReleased arm above
                    // ends it. The handler ignores repeats and gates on tab /
                    // modal / readiness.
                    keyboard::Key::Named(Named::Space) => Some(Message::SetBeforePeek(true)),
                    keyboard::Key::Named(Named::ArrowRight) => Some(Message::SelectNextImage),
                    keyboard::Key::Named(Named::ArrowLeft) => Some(Message::SelectPreviousImage),
                    keyboard::Key::Named(Named::Delete)
                    | keyboard::Key::Named(Named::Backspace) => Some(Message::DeleteImage),
                    // Enter confirms whichever modal is open. One key event can
                    // only produce one message, so it routes on `active_modal`
                    // in handle_modal_confirm rather than each modal trying to
                    // claim the key. Inert when no modal is open.
                    keyboard::Key::Named(Named::Enter) => Some(Message::ModalConfirm),
                    // D confirms "Mark for Removal" in the Delete modal. Safe
                    // everywhere else — the handler no-ops unless Modal::Delete
                    // is actually open.
                    keyboard::Key::Character(c) if c == "d" || c == "D" => {
                        Some(Message::MarkForRemovalConfirmed)
                    }
                    // Phase 56: Rating Shortcuts (1-5)
                    keyboard::Key::Character(c) if c == "0" => Some(Message::SetRating(0)),
                    keyboard::Key::Character(c) if c == "1" => Some(Message::SetRating(1)),
                    keyboard::Key::Character(c) if c == "2" => Some(Message::SetRating(2)),
                    keyboard::Key::Character(c) if c == "3" => Some(Message::SetRating(3)),
                    keyboard::Key::Character(c) if c == "4" => Some(Message::SetRating(4)),
                    keyboard::Key::Character(c) if c == "5" => Some(Message::SetRating(5)),
                    keyboard::Key::Character(c) if c == "r" || c == "R" => {
                        Some(Message::ResetEdits)
                    }
                    // Phase 83: Flag Shortcuts
                    keyboard::Key::Character(c) if c == "p" || c == "P" => {
                        Some(Message::SetFlag(1))
                    }
                    keyboard::Key::Character(c) if c == "x" || c == "X" => {
                        Some(Message::SetFlag(-1))
                    }
                    keyboard::Key::Character(c) if c == "u" || c == "U" => {
                        Some(Message::SetFlag(0))
                    }
                    // Phase 60: HUD Toggle
                    keyboard::Key::Character(c) if c == "i" || c == "I" => {
                        Some(Message::ToggleInfoHud)
                    }
                    // Phase 65: Undo/Redo shortcuts
                    keyboard::Key::Character(c)
                        if c == "z" && (modifiers.command() || modifiers.control()) =>
                    {
                        if modifiers.shift() {
                            Some(Message::Redo)
                        } else {
                            Some(Message::Undo)
                        }
                    }
                    keyboard::Key::Character(c)
                        if c == "y" && (modifiers.command() || modifiers.control()) =>
                    {
                        Some(Message::Redo)
                    }
                    // Phase 67: Crop shortcut
                    keyboard::Key::Character(c) if c == "c" => Some(Message::ToggleCrop),
                    // WB eyedropper shortcut
                    keyboard::Key::Character(c) if c == "w" || c == "W" => {
                        Some(Message::ToggleWbPicker)
                    }
                    _ => None,
                }
            } else {
                None
            }
        });

        // Borrowed, not cloned: subscription() is rebuilt after every message,
        // and only the head of each queue is ever read.
        let loader_subscription = crate::app::loader::subscription(
            &self.queued_loads,
            &self.queued_raw_loads,
            &self.queued_proxy_loads,
        );

        let mut subs = vec![keyboard_subscription, loader_subscription];
        // Only tick while a filmstrip glide or a reveal scroll is in progress
        // — avoids driving a redraw-linked subscription at all times.
        if self.filmstrip_velocity != 0.0 || self.scroll_glide.is_some() {
            subs.push(iced::window::frames().map(Message::KineticTick));
        }
        iced::Subscription::batch(subs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> RawEditor {
        RawEditor::new().0
    }

    #[test]
    fn crop_dims_of_the_full_frame_is_the_frame() {
        assert_eq!(crop_dims(1280, 853, [0.0, 0.0, 1.0, 1.0]), (1280, 853));
    }

    /// The bug crop-aware display dims fixed: the viewport letterboxed using
    /// the FULL frame's aspect while the shader drew the crop stretched to
    /// fill it, so a half-width crop rendered horizontally stretched.
    #[test]
    fn crop_dims_follow_the_crop() {
        assert_eq!(crop_dims(1280, 853, [0.0, 0.0, 0.5, 1.0]), (640, 853));

        // A 16:9 crop of a 3:2 frame must report ~16:9.
        let (w, h) = crop_dims(1280, 853, [0.0, 0.078, 1.0, 0.84375]);
        let aspect = w as f32 / h as f32;
        assert!((aspect - 16.0 / 9.0).abs() < 0.02, "aspect was {aspect}");
    }

    /// A degenerate crop must not produce a zero-sized rect and divide by zero
    /// downstream in `fitted_size`.
    #[test]
    fn crop_dims_never_collapse_to_zero() {
        let (w, h) = crop_dims(1280, 853, [0.0, 0.0, 0.0001, 0.0001]);
        assert!(w >= 1 && h >= 1, "got {w}x{h}");
    }

    /// While the RAW loads, the placeholder is shown in its own shape: an
    /// edited proxy is already cropped, and must not be cropped again.
    #[test]
    fn a_loading_placeholder_is_not_cropped_again() {
        let mut e = editor();
        e.current_edit_params.crop = [0.0, 0.0, 0.5, 1.0];
        e.working_preview_bytes = Some(std::sync::Arc::from(vec![0u8; 4]));
        e.working_preview_dims = (384, 216);
        assert_eq!(e.image_display_dims(), (384, 216));
        // Nothing at all yet: the stock placeholder, also uncropped.
        e.working_preview_bytes = None;
        assert_eq!(e.image_display_dims(), (1280, 853));
    }

    // ── displayed_images / selection ─────────────────────────────────────────

    fn img(id: i64, path: &str, rating: u8) -> ImageData {
        ImageData {
            id,
            filename: format!("{id}.NEF"),
            path: path.to_string(),
            cache_path_thumb: None,
            cache_path_instant: None,
            cache_path_working: None,
            cache_path_edited: None,
            file_status: "exists".to_string(),
            rating,
            flag: 0,
        }
    }

    fn catalog() -> RawEditor {
        let mut e = editor();
        let marked = crate::database::models::MARKED_FOR_REMOVAL_RATING;
        e.images = vec![
            img(1, "/photos/2024/a.NEF", 0),
            img(2, "/photos/2024/b.NEF", 3),
            img(3, "/photos/2024-old/c.NEF", 5),
            img(4, "/photos/2025/d.NEF", 0),
            img(5, "/photos/2024/e.NEF", marked),
        ];
        e
    }

    #[test]
    fn displayed_images_library_shows_marked_and_honours_the_folder_filter() {
        let mut e = catalog();
        e.current_tab = AppTab::Library;
        assert_eq!(e.displayed_ids(), vec![1, 2, 3, 4, 5]);

        e.library_folder_filter = Some("/photos/2024".to_string());
        // Component match: 2024-old is a different folder and stays hidden.
        assert_eq!(e.displayed_ids(), vec![1, 2, 5]);
    }

    #[test]
    fn folder_filter_includes_subfolders_but_not_similarly_named_siblings() {
        assert!(path_is_within("/photos/2024/a.NEF", "/photos/2024"));
        assert!(path_is_within("/photos/2024/nested/b.NEF", "/photos/2024"));
        assert!(path_is_within("/photos/2024/a.NEF", "/photos/2024/"));
        assert!(!path_is_within("/photos/2024-old/c.NEF", "/photos/2024"));
        assert!(!path_is_within("/photos/20/c.NEF", "/photos/2024"));
    }

    #[test]
    fn displayed_images_develop_excludes_marked_for_removal() {
        let mut e = catalog();
        e.current_tab = AppTab::Develop;
        assert_eq!(e.displayed_ids(), vec![1, 2, 3, 4]);
    }

    /// The folder filter is Library-only; applying it in Cull or Develop would
    /// silently shrink the filmstrip.
    #[test]
    fn displayed_images_ignores_the_folder_filter_outside_library() {
        let mut e = catalog();
        e.library_folder_filter = Some("/photos/2025".to_string());
        e.current_tab = AppTab::Cull;
        assert_eq!(e.displayed_ids(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn displayed_images_applies_the_rating_filter_in_every_tab() {
        let mut e = catalog();
        e.min_filter_rating = 3;
        e.current_tab = AppTab::Cull;
        // 2 (3 stars), 3 (5 stars), and 5 — marked, whose sentinel 255 clears
        // any real threshold. That is intentional; Develop is where it's cut.
        assert_eq!(e.displayed_ids(), vec![2, 3, 5]);
        e.current_tab = AppTab::Develop;
        assert_eq!(e.displayed_ids(), vec![2, 3]);
    }

    #[test]
    fn is_selected_covers_both_the_current_image_and_the_multi_selection() {
        let mut e = catalog();
        e.selected_image_id = Some(2);
        e.multi_selection.insert(4);
        assert!(e.is_selected(2));
        assert!(e.is_selected(4));
        assert!(!e.is_selected(1));
    }

    /// The peek must not change the framing — that is the whole reason
    /// `before_reference` preserves crop.
    #[test]
    fn before_peek_does_not_change_the_displayed_aspect() {
        let mut e = editor();
        e.current_edit_params.crop = [0.0, 0.0, 0.5, 1.0];
        let normal = e.image_display_dims();

        let before = e.current_edit_params.before_reference();
        e.current_edit_params = before;
        assert_eq!(e.image_display_dims(), normal);
    }

    fn history_len(e: &mut RawEditor) -> usize {
        e.get_current_history().map(|(stack, _)| stack.len()).unwrap_or(0)
    }

    /// Clicking a slider without moving it still fires CommitEdit; that must
    /// not leave an Undo step that visibly does nothing.
    #[test]
    fn a_commit_that_changed_nothing_adds_no_undo_step() {
        let mut e = editor();
        e.selected_image_id = Some(1);
        let base = history_len(&mut e);

        e.commit_current_state();
        assert_eq!(history_len(&mut e), base);

        e.current_edit_params.exposure = 1.0;
        e.commit_current_state();
        assert_eq!(history_len(&mut e), base + 1);

        // Toggling the crop tool alone is not an edit either.
        e.current_edit_params.is_cropping = 1;
        e.commit_current_state();
        assert_eq!(history_len(&mut e), base + 1);
    }

    /// History entries recorded in crop mode carry the shader flag; undoing
    /// after leaving crop mode must not switch the crop visualisation back on.
    #[test]
    fn undo_keeps_the_shader_crop_flag_in_step_with_the_tool() {
        let mut e = editor();
        e.selected_image_id = Some(1);
        e.current_tab = AppTab::Develop;
        e.is_cropping = true;
        e.sync_tool_flags();
        let _ = history_len(&mut e);
        e.current_edit_params.crop = [0.1, 0.1, 0.5, 0.5];
        e.commit_current_state();

        e.is_cropping = false; // "Done"
        let _ = crate::app::handlers::develop::handle_undo(&mut e);
        assert_eq!(e.current_edit_params.crop, [0.0, 0.0, 1.0, 1.0], "undo restored the old crop");
        assert_eq!(e.current_edit_params.is_cropping, 0, "but not the old tool state");
    }

    // ── edited proxies ────────────────────────────────────────────────────────

    fn proxy(edited: bool) -> EditedProxy {
        let preview = CachedPreview { width: 1, height: 1, bytes: Arc::from(vec![0u8; 4]) };
        EditedProxy {
            handle: preview.to_handle(),
            preview,
            edited,
            params: None,
            generation: 1,
            persisted: false,
        }
    }

    #[test]
    fn thumbnails_prefer_the_edit_and_fall_back_to_the_camera() {
        let mut e = catalog();
        let mut image = e.images[0].clone();
        image.cache_path_thumb = Some("/c/thumb.jpg".into());

        // Never developed: the camera's own thumbnail.
        assert_eq!(e.thumbnail_handle(&image), Some(Handle::from_path("/c/thumb.jpg")));

        // A saved edited proxy from an earlier session.
        image.cache_path_edited = Some("/c/edited.jpg".into());
        assert_eq!(e.thumbnail_handle(&image), Some(Handle::from_path("/c/edited.jpg")));

        // A live render outranks the file.
        let live = proxy(true);
        let live_handle = live.handle.clone();
        e.edited_proxies.put(image.id, live);
        assert_eq!(e.thumbnail_handle(&image), Some(live_handle));

        // ...including when it says the edit was just reset to defaults.
        e.edited_proxies.put(image.id, proxy(false));
        assert_eq!(e.thumbnail_handle(&image), Some(Handle::from_path("/c/thumb.jpg")));
    }

    /// Undo / reset / paste in Library change an image's saved edits without
    /// rendering it, so its proxy must go rather than keep showing the old edit.
    #[test]
    fn edits_saved_without_a_render_invalidate_the_proxy() {
        let mut e = catalog();
        e.current_tab = AppTab::Library;
        e.selected_image_id = Some(1);
        e.images[0].cache_path_edited = Some("/c/edited.jpg".into());
        e.edited_proxies.put(1, proxy(true));

        e.mark_edits_dirty();
        e.flush_edits();
        assert!(e.edited_proxies.peek(&1).is_none());
        assert_eq!(e.images[0].cache_path_edited, None);
    }
}
