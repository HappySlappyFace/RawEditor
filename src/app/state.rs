use crate::app::message::{AppTab, Message};
use crate::database;
use crate::database::models::Image as ImageData;
use crate::gpu;
use crate::raw;
use crate::ui::preview_renderer::{CropHandle, MaskHandle};
use iced::widget::image::Handle;
use iced::{Point, Rectangle, Task};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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

/// Local adjustment mask tool mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskTool {
    Inactive,
    /// Next click on the preview places a linear gradient
    PlacingLinear,
    /// Next click on the preview places a radial gradient
    PlacingRadial,
}

/// Phase 83: Pick/Reject flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Unflagged = 0,
    Pick = 1,
    Reject = -1,
}

use lru::LruCache;
use std::num::NonZeroUsize;

/// Phase 84: Modal system
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Main application state
#[derive(Debug)]
pub struct RawEditor {
    // Phase 84: Active modal overlay
    pub active_modal: Modal,

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
    /// Cache directory for full-size previews
    pub preview_cache_dir: PathBuf,
    /// Phase 73: RAM cache for 1280px previews (Look-Ahead)
    pub preview_cache: LruCache<i64, CachedPreview>,
    /// RAW preload RAM budget in MB (0 = disabled)
    pub raw_preload_budget_mb: u32,
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
    /// Local adjustment mask tool (placement mode). Mutually exclusive with
    /// crop and WB picker.
    pub mask_tool: MaskTool,
    /// Index into current_edit_params.masks of the mask being edited
    /// (overlay handles + sidebar sliders). None = no mask selected.
    pub selected_mask: Option<usize>,
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
    /// True while a full-resolution resource rebuild is in flight (zoom-in upgrade).
    pub full_res_upgrading: bool,

    // ── Scroll boost + filmstrip momentum ──────────────────────────────────
    /// Window-space cursor position, updated by the global CursorMoved
    /// subscription. Used to route wheel events to the region under the cursor.
    pub global_cursor: Point,
    /// Current window size, updated by the global Resized subscription.
    pub window_size: iced::Size,
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

/// Phase 80: Configurable Cache Size
pub const PRELOAD_BEHIND: usize = 10;
pub const PRELOAD_AHEAD: usize = 50;
// Phase 81: Increased cache capacity
pub const CACHE_CAPACITY: usize = 200;
// RAW preload window for develop mode
pub const RAW_PRELOAD_BEHIND: usize = 1;
pub const RAW_PRELOAD_AHEAD: usize = 4;
pub const RAW_PRELOAD_BUDGET_MB_DEFAULT: u32 = 1024;
pub const RAW_CACHE_ENTRY_CAPACITY: usize = 4096;
pub const PREVIEW_PRELOAD_BEHIND_MAX: usize = 30;
pub const PREVIEW_PRELOAD_AHEAD_MAX: usize = 100;
pub const RAW_PRELOAD_BEHIND_MAX: usize = 10;
pub const RAW_PRELOAD_AHEAD_MAX: usize = 20;

impl RawEditor {
    pub fn title(&self) -> String {
        String::from("RAW Editor")
    }

    /// Phase 23: Create a new instance of the application (INSTANT!)
    /// The database now loads in the background to show splash screen immediately
    pub fn new() -> (Self, Task<Message>) {
        tracing::info!("RAW Editor starting (instant splash screen)...");

        // Initialize preview cache directory (fast)
        let preview_cache_dir = raw::preview::get_preview_cache_dir();
        let settings = crate::core::settings::AppSettings::load_or_default();
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
                preview_cache_dir,
                preview_cache: LruCache::new(NonZeroUsize::new(cache_capacity).unwrap()),
                raw_preload_budget_mb,
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
                global_cursor: Point::ORIGIN,
                window_size: iced::Size::new(1920.0, 1080.0),
                filmstrip_velocity: 0.0,
                last_kinetic_tick: None,
                last_wheel_time: None,
                filmstrip_input_speed: 0.0,
                filmstrip_decay: 4.0,

                // Background task
                // background_task: None, // This field is not defined in the struct
                history_map: HashMap::new(),
                is_cropping: false,
                is_wb_picking: false,
                mask_tool: MaskTool::Inactive,
                selected_mask: None,
                drag_mode: DragMode::None,
                pending_loads: HashSet::new(),
                queued_loads: Vec::new(),
                pending_raw_loads: HashSet::new(),
                queued_raw_loads: Vec::new(),
            },
            Task::perform(
                database::library::load_database("".to_string()),
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

    /// Helper to push current state to history.
    ///
    /// Also flushes pending edits to SQLite: a history commit marks the end of
    /// a gesture, which is exactly when the params are worth persisting. Tying
    /// the two together means any future commit site gets persistence for free
    /// and the two can't drift apart.
    pub fn commit_current_state(&mut self) {
        self.flush_edits();
        let params_to_push = self.current_edit_params;
        if let Some((stack, index)) = self.get_current_history() {
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
                    .map(|folder| img.path.starts_with(folder))
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

    /// Ids of [`Self::displayed_images`], in the same order.
    pub fn displayed_ids(&self) -> Vec<i64> {
        self.displayed_images().iter().map(|i| i.id).collect()
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
    pub fn image_display_dims(&self) -> (u32, u32) {
        let (w, h) = if let Some(res) = &self.image_resources {
            res.oriented_dims()
        } else if self.working_preview_bytes.is_some() {
            self.working_preview_dims
        } else {
            (1280, 853)
        };

        if self.is_cropping {
            return (w, h);
        }
        let crop = self.current_edit_params.crop;
        (
            ((w as f32 * crop[2]).round() as u32).max(1),
            ((h as f32 * crop[3]).round() as u32).max(1),
        )
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
            .map(|r| r.oriented_original_dims().0)
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

    // Phase 67: Calculate image screen bounds for interaction
    pub fn get_image_screen_bounds(&self) -> Rectangle {
        let (vw, vh) = self.viewport_size;
        let (dw, dh) = self.image_display_dims();
        crate::core::viewport::image_rect(dw, dh, vw, vh, self.zoom, self.pan_offset)
    }

    // Phase 67: Detect if cursor is over a crop handle
    pub fn detect_crop_handle(&self, cursor_pos: Point) -> Option<CropHandle> {
        if self.image_resources.is_some() {
            let bounds = self.get_image_screen_bounds();
            let crop = self.current_edit_params.crop;

            let crop_x = bounds.x + (crop[0] * bounds.width);
            let crop_y = bounds.y + (crop[1] * bounds.height);
            let crop_w = crop[2] * bounds.width;
            let crop_h = crop[3] * bounds.height;

            let handle_radius = 15.0; // Hitbox radius (generous)

            let check_handle = |x, y| {
                let dx = cursor_pos.x - x;
                let dy = cursor_pos.y - y;
                (dx * dx + dy * dy) < handle_radius * handle_radius
            };

            if check_handle(crop_x, crop_y) {
                return Some(CropHandle::TopLeft);
            }
            if check_handle(crop_x + crop_w, crop_y) {
                return Some(CropHandle::TopRight);
            }
            if check_handle(crop_x, crop_y + crop_h) {
                return Some(CropHandle::BottomLeft);
            }
            if check_handle(crop_x + crop_w, crop_y + crop_h) {
                return Some(CropHandle::BottomRight);
            }

            // Phase 70: Check if inside body
            if cursor_pos.x >= crop_x
                && cursor_pos.x <= crop_x + crop_w
                && cursor_pos.y >= crop_y
                && cursor_pos.y <= crop_y + crop_h
            {
                return Some(CropHandle::Body);
            }
        }
        None
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

        let keyboard_subscription = iced::event::listen_with(|event, _status, _window| {
            // Phase 55: Track modifier key changes for Ctrl/Cmd+Click
            if let iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
                return Some(Message::ModifiersChanged(modifiers));
            }

            // Scroll boost + filmstrip momentum: routed by cursor region in
            // handlers::scroll, so we just forward these globally.
            if let iced::Event::Mouse(iced::mouse::Event::WheelScrolled { delta }) = event {
                return Some(Message::WheelScrolled(delta));
            }
            if let iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) = event {
                return Some(Message::GlobalCursorMoved(position));
            }
            if let iced::Event::Window(iced::window::Event::Resized(size)) = event {
                return Some(Message::WindowResized(size));
            }
            // Alt-tabbing away while Space is held sends the KeyReleased to
            // the other window, which would otherwise strand the peek on.
            if let iced::Event::Window(iced::window::Event::Unfocused) = event {
                return Some(Message::SetBeforePeek(false));
            }
            // Hold-to-peek needs the release edge; every other shortcut here
            // is press-only.
            if let iced::Event::Keyboard(keyboard::Event::KeyReleased { key, .. }) = &event {
                if matches!(key, keyboard::Key::Named(Named::Space)) {
                    return Some(Message::SetBeforePeek(false));
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
        let loader_subscription =
            crate::app::loader::subscription(&self.queued_loads, &self.queued_raw_loads);

        let mut subs = vec![keyboard_subscription, loader_subscription];
        // Only tick while a filmstrip glide is in progress — avoids driving a
        // redraw-linked subscription at all times.
        if self.filmstrip_velocity != 0.0 {
            subs.push(iced::window::frames().map(Message::KineticTick));
        }
        iced::Subscription::batch(subs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No GPU or image needed: with no resources loaded `image_display_dims`
    /// falls back to the 1280x853 placeholder, which is enough to exercise the
    /// crop arithmetic.
    fn editor() -> RawEditor {
        RawEditor::new().0
    }

    #[test]
    fn image_display_dims_uncropped_is_the_whole_frame() {
        let e = editor();
        assert_eq!(e.current_edit_params.crop, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(e.image_display_dims(), (1280, 853));
    }

    /// The bug this fixes: the viewport letterboxed using the FULL frame's
    /// aspect while the shader drew the crop stretched to fill it, so a
    /// half-width crop rendered horizontally stretched.
    #[test]
    fn image_display_dims_applies_the_crop_outside_crop_mode() {
        let mut e = editor();
        e.current_edit_params.crop = [0.0, 0.0, 0.5, 1.0];
        assert_eq!(e.image_display_dims(), (640, 853));

        // A 16:9 crop of the 3:2 placeholder must report ~16:9.
        e.current_edit_params.crop = [0.0, 0.078, 1.0, 0.84375];
        let (w, h) = e.image_display_dims();
        let aspect = w as f32 / h as f32;
        assert!((aspect - 16.0 / 9.0).abs() < 0.02, "aspect was {aspect}");
    }

    /// In crop mode the shader deliberately reveals the whole sensor so the
    /// handles can be dragged over it — applying the crop here would make the
    /// handles drift away from the cursor.
    #[test]
    fn image_display_dims_ignores_the_crop_while_cropping() {
        let mut e = editor();
        e.current_edit_params.crop = [0.0, 0.0, 0.5, 1.0];
        e.is_cropping = true;
        assert_eq!(e.image_display_dims(), (1280, 853));
    }

    /// A degenerate crop must not produce a zero-sized rect and divide by zero
    /// downstream in `fitted_size`.
    #[test]
    fn image_display_dims_never_collapses_to_zero() {
        let mut e = editor();
        e.current_edit_params.crop = [0.0, 0.0, 0.0001, 0.0001];
        let (w, h) = e.image_display_dims();
        assert!(w >= 1 && h >= 1, "got {w}x{h}");
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
        // Prefix match, so 2024-old is included — this mirrors the existing
        // filter behaviour, which the sidebar's remove button deliberately
        // does NOT share.
        assert_eq!(e.displayed_ids(), vec![1, 2, 3, 5]);
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
}
