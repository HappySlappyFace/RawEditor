use crate::app::message::Message;
use crate::database;
use crate::database::models::Image as ImageData;
use crate::ui::preview_renderer::{CropHandle, MaskHandle};
use iced::widget::image::Handle;
use iced::{Point, Task};
use std::collections::HashSet;
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
    /// Folders dropped onto the window, waiting for the user to import them
    /// (`RawEditor::drop_import`).
    ImportDropped,
    /// Confirm downloading an AI model a feature needs
    /// (`RawEditor::model_offer`).
    AiModel,
    /// AI denoise: choose the amount on a 1:1 preview, then apply
    /// (`RawEditor::ai_denoise`).
    AiDenoise,
    /// First run: "Check for updates at startup?" (`RawEditor::updates`).
    UpdatePrompt,
    /// A check's result: a newer version, up to date, or an error.
    Update,
}

/// Folders dropped onto the window (`handlers::library::handle_file_dropped`).
///
/// Dropping fifty RAW files from one folder delivers fifty events, so each
/// folder is scanned once (`scanning`) and offered once (`folders`).
#[derive(Debug, Default)]
pub struct DropImport {
    /// Dropped items whose scan hasn't come back yet.
    pub scanning: HashSet<std::path::PathBuf>,
    /// Folders the dialog offers, each with the number of new photos it adds.
    pub folders: Vec<(std::path::PathBuf, usize)>,
    /// Every new file across `folders`, so nested or repeated folders aren't
    /// counted twice.
    pub files: HashSet<std::path::PathBuf>,
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

/// The pages of the Preferences side menu, in menu order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreferencesSection {
    #[default]
    General,
    Performance,
    Memory,
    CameraProfiles,
    LensProfiles,
    AiModels,
    Folders,
    Licenses,
}

impl PreferencesSection {
    pub const ALL: [PreferencesSection; 8] = [
        PreferencesSection::General,
        PreferencesSection::Performance,
        PreferencesSection::Memory,
        PreferencesSection::CameraProfiles,
        PreferencesSection::LensProfiles,
        PreferencesSection::AiModels,
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
    /// Colour space the pixels are encoded in and tagged with (ICC profile).
    pub color_space: crate::core::output_space::OutputSpace,
    pub quality: u8,
    pub resize: bool,
    pub max_width: u32,
    /// The "Max long edge" field as typed. `max_width` is its value when it is
    /// a positive whole number, else 0 (see [`ExportSettings::size_invalid`]).
    pub max_width_text: String,
    pub subfolder: String,
    pub base_path: std::path::PathBuf,
}

impl ExportSettings {
    /// Resize is on but the long edge is not a positive number: exporting
    /// would silently write full-size files.
    pub fn size_invalid(&self) -> bool {
        self.resize && self.max_width == 0
    }
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jpeg,
            color_space: crate::core::output_space::OutputSpace::Srgb,
            quality: 80,
            resize: false,
            max_width: 2048,
            max_width_text: "2048".to_string(),
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

/// `(kelvin, tone, interpolation, the profile it came from)`: the memo is
/// valid only for the same profile (pointer-compared; kept alive here).
pub type DcpMemo = (
    f32,
    crate::core::tonemap::ToneSettings,
    Arc<crate::raw::dcp::InterpolatedProfile>,
    Arc<crate::raw::dcp::DcpProfile>,
);

/// Shape assumed for the viewport before any preview or render exists.
pub const PLACEHOLDER_DIMS: (u32, u32) = (1280, 853);

/// How soon after a slider's commit a double-click reset still counts as
/// the second half of the same gesture.
pub const SLIDER_AMEND_WINDOW: std::time::Duration = std::time::Duration::from_millis(700);

/// Undo steps kept per image.
pub const HISTORY_LIMIT: usize = 100;

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
    /// Dark or light UI (`ui::theme::set_appearance` holds the live value).
    pub appearance: crate::ui::theme::Appearance,
    /// The editing session: the app's logic, toolkit-free (`crate::session`).
    pub session: crate::session::Session,
    /// The first-run update prompt's checkbox, which starts ticked.
    pub update_prompt_choice: bool,
    /// The model the AI-model confirm dialog offers (`handlers::models`).
    pub model_offer: Option<crate::ml::models::ModelId>,
    /// The page Preferences shows. Kept while the app runs, so reopening the
    /// dialog lands where the user left it; not persisted.
    pub preferences_section: PreferencesSection,
    /// An in-flight smooth scroll that brings the selection into view.
    pub scroll_glide: Option<ScrollGlide>,
    /// Bumped whenever a title-bar dropdown entry is chosen, which remounts
    /// (and so closes) the menu bar — see `views::layout::view_title_bar`.
    pub menu_generation: u64,

    // Phase 88: Responsive Grid
    pub thumbnail_size: f32,
    /// Phase 21: Histogram canvas cache
    pub histogram_cache: iced::widget::canvas::Cache,
    /// Phase 22: Histogram toggle (keep for user control)
    pub histogram_enabled: bool,
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
    /// Phase 29: Instant preview handle (displayed while RAW loads)
    pub working_preview: Option<Handle>,
    /// Phase 55: Track modifier keys for Ctrl/Cmd+Click
    pub last_modifiers: iced::keyboard::Modifiers,
    /// Character shortcuts whose key is currently held (see `key_is_fresh`).
    pub keys_down: std::collections::HashMap<String, std::time::Instant>,
    /// Pivot for Shift+click range selection: the last image picked WITHOUT
    /// shift. Held separately from `selected_image_id` because a range drag
    /// moves the current selection while the pivot must stay put.
    pub selection_anchor: Option<i64>,
    /// Folder the open RemoveFolder dialog acts on — a snapshot taken when the
    /// dialog opens, mirroring `pending_delete_ids`.
    pub pending_remove_folder: Option<String>,
    /// Folders dropped onto the window: being scanned, and scanned ones the
    /// ImportDropped dialog offers.
    pub drop_import: DropImport,
    /// Phase 79: Modular Info Overlay state
    pub info_overlay: InfoOverlayState,
    /// The Develop slider whose value is being typed, if any.
    pub slider_entry: Option<SliderEntry>,
    /// Painting into the selected mask is armed (always on for brush masks).
    pub brush_armed: bool,
    /// Collapsed Develop sidebar sections (UI state).
    pub collapsed_sections: HashSet<crate::app::message::SidebarSection>,
    /// Which point curve the curve editor is showing (UI state, not an edit).
    pub curve_channel: crate::core::curves::CurveChannel,
    /// Phase 67: Drag mode for interaction
    pub drag_mode: DragMode,
    pub show_profiler: bool,
    /// Phase 105: Cache for profiler canvas (cleared on each new frame push)
    pub profiler_cache: iced::widget::canvas::Cache,

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
        String::from("RawEditor")
    }

    /// Phase 23: Create a new instance of the application (INSTANT!)
    /// The database now loads in the background to show splash screen immediately
    pub fn new() -> (Self, Task<Message>) {
        tracing::info!("RAW Editor starting (instant splash screen)...");

        let settings = crate::core::settings::AppSettings::load_or_default();
        // Not under test: the flag is process-global and tests build editors
        // in parallel. (Under test `settings` are always the defaults —
        // `AppSettings::load_or_default`.)
        let appearance = settings
            .appearance
            .as_deref()
            .and_then(crate::ui::theme::Appearance::from_name)
            .unwrap_or_default();
        if !cfg!(test) {
            crate::ui::motion::set_reduced(settings.reduce_motion);
            crate::ui::theme::set_appearance(appearance);
        }
        let thumbnail_size = settings.thumbnail_size.clamp(100.0, 400.0);

        // Determine the database path (e.g., in the application's data directory)
        let _db_path = database::library::Library::get_db_path();

        (
            RawEditor {
                histogram_cache: iced::widget::canvas::Cache::default(),
                histogram_enabled: settings.histogram_enabled,
                canvas_cache: iced::widget::canvas::Cache::default(),
                is_dragging: false,
                last_cursor_position: None,
                last_click_time: None,
                last_titlebar_click: None,
                working_preview: None,
                last_modifiers: iced::keyboard::Modifiers::default(),
                keys_down: Default::default(),
                selection_anchor: None,
                pending_remove_folder: None,
                drop_import: DropImport::default(),
                // Must be the enum's own default (Hidden). Starting at
                // Metadata booted the app with the overlay already on, at
                // phase 2 of 3, so the first `I` press advanced the cycle
                // instead of opening it.
                info_overlay: crate::app::state::InfoOverlayState::default(),
                // Phase 84
                active_modal: Modal::None,
                // Phase 88
                thumbnail_size,
                show_profiler: false,
                profiler_cache: iced::widget::canvas::Cache::default(),
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
                appearance,
                session: crate::session::Session::new(&settings),
                update_prompt_choice: true,
                model_offer: None,
                preferences_section: PreferencesSection::default(),
                scroll_glide: None,
                menu_generation: 0,
                last_wheel_time: None,
                filmstrip_input_speed: 0.0,
                filmstrip_decay: 4.0,
                slider_entry: None,
                curve_channel: crate::core::curves::CurveChannel::Master,
                brush_armed: false,
                collapsed_sections: match &settings.develop_folded {
                    Some(names) => names
                        .iter()
                        .filter_map(|n| crate::app::message::SidebarSection::from_name(n))
                        .collect(),
                    None => crate::app::message::SidebarSection::FOLDED_BY_DEFAULT.into_iter().collect(),
                },
                drag_mode: DragMode::None,
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

    /// The thumbnail to show for `img`: its edited proxy when it has edits,
    /// the camera's embedded preview otherwise.
    ///
    /// An in-memory proxy is authoritative — it is the newest render. When it
    /// says "unedited" (the edit was just reset) the camera thumbnail wins
    /// even over a saved edited one that has not been cleared yet — unless
    /// the camera has none (its preview could not be read, so the cache job
    /// failed): then the render is the only picture there is, edited or not.
    pub fn thumbnail_handle(&self, img: &ImageData) -> Option<Handle> {
        if let Some(proxy) = self.session.edited_proxies.peek(&img.id) {
            if proxy.edited || img.cache_path_thumb.is_none() {
                return Some(proxy.handle.clone());
            }
            return img.cache_path_thumb.as_ref().map(Handle::from_path);
        }
        img.cache_path_edited
            .as_ref()
            .or(img.cache_path_thumb.as_ref())
            .map(Handle::from_path)
    }

    pub fn save_preferences(&self) {
        let settings = crate::core::settings::AppSettings {
            cache_capacity: self.session.cache_capacity,
            raw_preload_budget_mb: self.session.raw_preload_budget_mb,
            thumbnail_size: self.thumbnail_size,
            auto_advance: self.session.auto_advance,
            histogram_enabled: self.histogram_enabled,
            preview_preload_behind: self.session.preview_preload_behind,
            preview_preload_ahead: self.session.preview_preload_ahead,
            raw_preload_behind: self.session.raw_preload_behind,
            raw_preload_ahead: self.session.raw_preload_ahead,
            copy_categories: self.session.copy_categories,
            min_free_ram_mb: self.session.min_free_ram_mb,
            reduce_motion: self.reduce_motion,
            appearance: Some(self.appearance.name().to_string()),
            camera_profile_defaults: self.session.camera_profile_defaults.clone(),
            update_check: self.session.update_check,
            develop_folded: Some(
                crate::app::message::SidebarSection::ALL
                    .into_iter()
                    .filter(|s| self.collapsed_sections.contains(s))
                    .map(|s| s.name().to_string())
                    .collect(),
            ),
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
            // Alt+F4, a WM close binding, the taskbar and a session logout:
            // with `exit_on_close_request(false)` they land here instead of
            // exiting past the save points `handle_close_window` runs.
            if let iced::Event::Window(iced::window::Event::CloseRequested) = event {
                return Some(Message::CloseWindow);
            }
            // Only winit's X11, macOS and Windows backends deliver drops;
            // on native Wayland (winit 0.30) this never fires.
            if let iced::Event::Window(iced::window::Event::FileDropped(path)) = event {
                return Some(Message::FileDropped(path));
            }
            // Alt-tabbing away while Space is held sends the KeyReleased to
            // the other window, which would otherwise strand the peek on (and
            // a held arrow key's glide at speed).
            if let iced::Event::Window(iced::window::Event::Unfocused) = event {
                return Some(Message::WindowUnfocused);
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
                if let keyboard::Key::Character(c) = key {
                    return Some(Message::ShortcutKeyUp(c.to_lowercase()));
                }
                return None;
            }

            if let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event {
                key_shortcut(key, modifiers)
            } else {
                None
            }
        });

        // Borrowed, not cloned: subscription() is rebuilt after every message,
        // and only the head of each queue is ever read.
        let loader_subscription = crate::app::loader::subscription(
            &self.session.queued_loads,
            &self.session.queued_raw_loads,
            &self.session.queued_proxy_loads,
        );

        let mut subs = vec![keyboard_subscription, loader_subscription];
        // Only tick while a filmstrip glide or a reveal scroll is in progress
        // — avoids driving a redraw-linked subscription at all times.
        if self.filmstrip_velocity != 0.0 || self.scroll_glide.is_some() {
            subs.push(iced::window::frames().map(Message::KineticTick));
        }
        // AI progress bars: tick only while a download or an AI job runs.
        if self.session.models.job.is_some() || self.session.ai_denoise.busy() {
            subs.push(iced::time::every(std::time::Duration::from_millis(100)).map(|_| Message::AiTick));
        }
        iced::Subscription::batch(subs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::message::AppTab;
    fn editor() -> RawEditor {
        RawEditor::new().0
    }

    #[test]
    fn message_stays_small_and_history_is_capped() {
        // EditParams is ~21 KB; it must not be carried by value in a Message.
        assert!(std::mem::size_of::<Message>() < 1024, "{}", std::mem::size_of::<Message>());

        let mut e = editor();
        e.session.selected_image_id = Some(1);
        for i in 0..(HISTORY_LIMIT + 20) {
            e.session.current_edit_params.exposure = i as f32 * 0.01;
            e.session.commit_current_state();
        }
        let (stack, index) = e.session.get_current_history().unwrap();
        assert_eq!(stack.len(), HISTORY_LIMIT);
        assert_eq!(*index, HISTORY_LIMIT - 1);
        assert_eq!(stack[*index].exposure, (HISTORY_LIMIT + 19) as f32 * 0.01);
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
        e.session.current_edit_params.crop = [0.0, 0.0, 0.5, 1.0];
        e.session.working_preview_bytes = Some(std::sync::Arc::from(vec![0u8; 4]));
        e.session.working_preview_dims = (384, 216);
        assert_eq!(e.session.image_display_dims(), (384, 216));
        // Nothing at all yet: the stock placeholder, also uncropped.
        e.session.working_preview_bytes = None;
        assert_eq!(e.session.image_display_dims(), (1280, 853));
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
        let marked = crate::database::models::with_mark(0, true);
        e.session.images = vec![
            img(1, "/photos/2024/a.NEF", 0),
            img(2, "/photos/2024/b.NEF", 3),
            img(3, "/photos/2024-old/c.NEF", 5),
            img(4, "/photos/2025/d.NEF", 0),
            img(5, "/photos/2024/e.NEF", marked),
        ];
        e
    }

    /// Auto-advance after a mark used to walk the unfiltered list and could
    /// land on an image the rating filter hides (no filmstrip selection).
    #[test]
    fn next_unmarked_prefers_an_image_the_filter_shows() {
        let mut e = catalog();
        e.session.current_tab = AppTab::Develop;
        e.session.min_filter_rating = 5;
        assert_eq!(e.session.displayed_ids(), vec![3]);
        assert_eq!(crate::app::handlers::navigation::find_next_unmarked_image_id(&e, 1), Some(3));
    }

    #[test]
    fn displayed_images_library_shows_marked_and_honours_the_folder_filter() {
        let mut e = catalog();
        e.session.current_tab = AppTab::Library;
        assert_eq!(e.session.displayed_ids(), vec![1, 2, 3, 4, 5]);

        e.session.library_folder_filter = Some("/photos/2024".to_string());
        // Component match: 2024-old is a different folder and stays hidden.
        assert_eq!(e.session.displayed_ids(), vec![1, 2, 5]);
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
        e.session.current_tab = AppTab::Develop;
        assert_eq!(e.session.displayed_ids(), vec![1, 2, 3, 4]);
    }

    /// The folder filter is Library-only; applying it in Cull or Develop would
    /// silently shrink the filmstrip.
    #[test]
    fn displayed_images_ignores_the_folder_filter_outside_library() {
        let mut e = catalog();
        e.session.library_folder_filter = Some("/photos/2025".to_string());
        e.session.current_tab = AppTab::Cull;
        assert_eq!(e.session.displayed_ids(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn displayed_images_applies_the_rating_filter_in_every_tab() {
        let mut e = catalog();
        e.session.min_filter_rating = 3;
        e.session.current_tab = AppTab::Cull;
        // 2 (3 stars), 3 (5 stars), and 5 — marked, whose sentinel 255 clears
        // any real threshold. That is intentional; Develop is where it's cut.
        assert_eq!(e.session.displayed_ids(), vec![2, 3, 5]);
        e.session.current_tab = AppTab::Develop;
        assert_eq!(e.session.displayed_ids(), vec![2, 3]);
    }

    #[test]
    fn is_selected_covers_both_the_current_image_and_the_multi_selection() {
        let mut e = catalog();
        e.session.selected_image_id = Some(2);
        e.session.multi_selection.insert(4);
        assert!(e.session.is_selected(2));
        assert!(e.session.is_selected(4));
        assert!(!e.session.is_selected(1));
    }

    /// The peek must not change the framing — that is the whole reason
    /// `before_reference` preserves crop.
    #[test]
    fn before_peek_does_not_change_the_displayed_aspect() {
        let mut e = editor();
        e.session.current_edit_params.crop = [0.0, 0.0, 0.5, 1.0];
        let normal = e.session.image_display_dims();

        let before = e.session.current_edit_params.before_reference();
        e.session.current_edit_params = before;
        assert_eq!(e.session.image_display_dims(), normal);
    }

    fn history_len(e: &mut RawEditor) -> usize {
        e.session.get_current_history().map(|(stack, _)| stack.len()).unwrap_or(0)
    }

    /// Clicking a slider without moving it still fires CommitEdit; that must
    /// not leave an Undo step that visibly does nothing.
    #[test]
    fn a_commit_that_changed_nothing_adds_no_undo_step() {
        let mut e = editor();
        e.session.selected_image_id = Some(1);
        let base = history_len(&mut e);

        e.session.commit_current_state();
        assert_eq!(history_len(&mut e), base);

        e.session.current_edit_params.exposure = 1.0;
        e.session.commit_current_state();
        assert_eq!(history_len(&mut e), base + 1);

        // Toggling the crop tool alone is not an edit either.
        e.session.current_edit_params.is_cropping = 1;
        e.session.commit_current_state();
        assert_eq!(history_len(&mut e), base + 1);
    }

    /// History entries recorded in crop mode carry the shader flag; undoing
    /// after leaving crop mode must not switch the crop visualisation back on.
    #[test]
    fn undo_keeps_the_shader_crop_flag_in_step_with_the_tool() {
        let mut e = editor();
        e.session.selected_image_id = Some(1);
        e.session.current_tab = AppTab::Develop;
        e.session.is_cropping = true;
        e.session.sync_tool_flags();
        let _ = history_len(&mut e);
        e.session.current_edit_params.crop = [0.1, 0.1, 0.5, 0.5];
        e.session.commit_current_state();

        e.session.is_cropping = false; // "Done"
        let _ = crate::app::handlers::develop::handle_undo(&mut e);
        assert_eq!(e.session.current_edit_params.crop, [0.0, 0.0, 1.0, 1.0], "undo restored the old crop");
        assert_eq!(e.session.current_edit_params.is_cropping, 0, "but not the old tool state");
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
        let mut image = e.session.images[0].clone();
        image.cache_path_thumb = Some("/c/thumb.jpg".into());

        // Never developed: the camera's own thumbnail.
        assert_eq!(e.thumbnail_handle(&image), Some(Handle::from_path("/c/thumb.jpg")));

        // A saved edited proxy from an earlier session.
        image.cache_path_edited = Some("/c/edited.jpg".into());
        assert_eq!(e.thumbnail_handle(&image), Some(Handle::from_path("/c/edited.jpg")));

        // A live render outranks the file.
        let live = proxy(true);
        let live_handle = live.handle.clone();
        e.session.edited_proxies.put(image.id, live);
        assert_eq!(e.thumbnail_handle(&image), Some(live_handle));

        // ...including when it says the edit was just reset to defaults.
        e.session.edited_proxies.put(image.id, proxy(false));
        assert_eq!(e.thumbnail_handle(&image), Some(Handle::from_path("/c/thumb.jpg")));
    }

    /// A RAW whose embedded preview couldn't be read has no camera thumbnail;
    /// its unedited render must fill the slot rather than leave it blank
    /// until the user touches a slider.
    #[test]
    fn without_a_camera_thumbnail_an_unedited_render_is_shown() {
        let mut e = catalog();
        let mut image = e.session.images[0].clone();
        image.cache_path_thumb = None;
        assert_eq!(e.thumbnail_handle(&image), None);

        let unedited = proxy(false);
        let handle = unedited.handle.clone();
        e.session.edited_proxies.put(image.id, unedited);
        assert_eq!(e.thumbnail_handle(&image), Some(handle));
    }

    /// Undo / reset / paste in Library change an image's saved edits without
    /// rendering it, so its proxy must go rather than keep showing the old edit.
    #[test]
    fn edits_saved_without_a_render_invalidate_the_proxy() {
        let mut e = catalog();
        e.session.current_tab = AppTab::Library;
        e.session.selected_image_id = Some(1);
        e.session.images[0].cache_path_edited = Some("/c/edited.jpg".into());
        e.session.edited_proxies.put(1, proxy(true));

        e.session.mark_edits_dirty();
        e.session.flush_edits();
        assert!(e.session.edited_proxies.peek(&1).is_none());
        assert_eq!(e.session.images[0].cache_path_edited, None);
    }
}

/// The message a key press stands for, if any. Pure, so the keymap can be
/// tested.
pub(crate) fn key_shortcut(key: iced::keyboard::Key, modifiers: iced::keyboard::Modifiers) -> Option<Message> {
    use iced::keyboard;
    use iced::keyboard::key::Named;
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

        // Ctrl/Cmd+Z and Y first: they are the only letter shortcuts that
        // take a modifier.
        if let keyboard::Key::Character(c) = &key {
            if matches!(c.as_str(), "z" | "Z" | "y" | "Y") && (modifiers.command() || modifiers.control()) {
                return Some(if c.eq_ignore_ascii_case("y") || modifiers.shift() { Message::Redo } else { Message::Undo });
            }
            // Every other letter or digit shortcut is a bare key (Shift allowed:
            // "?" and capitals). With Ctrl/Cmd/Alt held it is someone else's
            // chord (Ctrl+R reload, Cmd+W close, Alt as the slider's fine-drag
            // modifier) and must not reset edits, rate or flag.
            if modifiers.command() || modifiers.control() || modifiers.alt() || modifiers.logo() {
                return None;
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
                once(&c, Message::MarkForRemovalConfirmed)
            }
            // Phase 56: Rating Shortcuts (1-5)
            keyboard::Key::Character(c) if c == "0" => once(&c, Message::SetRating(0)),
            keyboard::Key::Character(c) if c == "1" => once(&c, Message::SetRating(1)),
            keyboard::Key::Character(c) if c == "2" => once(&c, Message::SetRating(2)),
            keyboard::Key::Character(c) if c == "3" => once(&c, Message::SetRating(3)),
            keyboard::Key::Character(c) if c == "4" => once(&c, Message::SetRating(4)),
            keyboard::Key::Character(c) if c == "5" => once(&c, Message::SetRating(5)),
            keyboard::Key::Character(c) if c == "r" || c == "R" => {
                once(&c, Message::ResetEdits)
            }
            // Phase 83: Flag Shortcuts
            keyboard::Key::Character(c) if c == "p" || c == "P" => {
                once(&c, Message::SetFlag(1))
            }
            keyboard::Key::Character(c) if c == "x" || c == "X" => {
                once(&c, Message::SetFlag(-1))
            }
            keyboard::Key::Character(c) if c == "u" || c == "U" => {
                once(&c, Message::SetFlag(0))
            }
            // Phase 60: HUD Toggle
            keyboard::Key::Character(c) if c == "i" || c == "I" => {
                once(&c, Message::ToggleInfoHud)
            }
            // Phase 67: Crop shortcut
            keyboard::Key::Character(c) if c == "c" => once(&c, Message::ToggleCrop),
            // WB eyedropper shortcut
            keyboard::Key::Character(c) if c == "w" || c == "W" => {
                once(&c, Message::ToggleWbPicker)
            }
            _ => None,
        }
}

/// A shortcut that must not repeat while its key is held (a rating or flag is
/// a database write, the toggles would flicker). iced 0.13's `KeyPressed` has
/// no repeat flag, so `update` tracks which keys are down (`keys_down`).
fn once(key: &str, message: Message) -> Option<Message> {
    Some(Message::ShortcutKey { key: key.to_lowercase(), message: Box::new(message) })
}

impl RawEditor {
    /// Is this press of `key` the first of a hold? Repeats refresh the entry,
    /// so it lives as long as the hold; an entry nothing has refreshed for
    /// `HOLD_GAP` is stale (a key-up that went to another window) and the
    /// next press counts as fresh.
    pub(crate) fn key_is_fresh(&mut self, key: String) -> bool {
        const HOLD_GAP: std::time::Duration = std::time::Duration::from_millis(1500);
        let now = std::time::Instant::now();
        match self.keys_down.insert(key, now) {
            Some(last) => now.duration_since(last) > HOLD_GAP,
            None => true,
        }
    }
}

#[cfg(test)]
mod keymap_tests {
    use super::*;
    use iced::keyboard::{Key, Modifiers};

    fn ch(c: &str) -> Key {
        Key::Character(c.into())
    }

    /// The message inside a once-per-hold wrapper.
    fn inner(m: Option<Message>) -> Option<Message> {
        match m {
            Some(Message::ShortcutKey { message, .. }) => Some(*message),
            other => other,
        }
    }

    /// A held key repeats its press: only the first of a hold counts, and a
    /// key-up (or the hold going stale) re-arms it.
    #[test]
    fn a_held_key_fires_once() {
        let mut e = RawEditor::new().0;
        assert!(e.key_is_fresh("r".into()));
        assert!(!e.key_is_fresh("r".into()));
        assert!(!e.key_is_fresh("r".into()));
        assert!(e.key_is_fresh("p".into()), "another key is independent");
        e.keys_down.remove("r");
        assert!(e.key_is_fresh("r".into()), "released, pressed again");
        e.keys_down.insert("p".into(), std::time::Instant::now() - std::time::Duration::from_secs(5));
        assert!(e.key_is_fresh("p".into()), "a stale hold must not block the key for good");
    }

    /// Through `update`: the second press of a held rating key does nothing.
    #[test]
    fn update_ignores_repeats_of_a_held_shortcut() {
        let mut e = crate::app::testkit::editor_with_images(2);
        crate::app::testkit::send(&mut e, Message::ImageSelected(1));
        let press = || Message::ShortcutKey { key: "3".into(), message: Box::new(Message::SetRating(3)) };
        crate::app::testkit::send(&mut e, press());
        assert_eq!(e.session.images[0].rating, 3);
        e.session.images[0].rating = 0; // a repeat must not set it again
        crate::app::testkit::send(&mut e, press());
        assert_eq!(e.session.images[0].rating, 0);
        crate::app::testkit::send(&mut e, Message::ShortcutKeyUp("3".into()));
        crate::app::testkit::send(&mut e, press());
        assert_eq!(e.session.images[0].rating, 3);
    }

    /// Ctrl/Cmd/Alt + a letter or digit is not a shortcut: Ctrl+R must not
    /// reset the edits, nor Ctrl+1 rate, nor Alt+C open the crop.
    #[test]
    fn a_held_modifier_disables_the_single_key_shortcuts() {
        for m in [Modifiers::CTRL, Modifiers::ALT, Modifiers::LOGO, Modifiers::CTRL | Modifiers::SHIFT] {
            for k in ["r", "d", "1", "5", "p", "x", "u", "i", "c", "w", "0"] {
                // Ctrl/Cmd+C is the clipboard, not ToggleCrop.
                if k == "c" && m.command() {
                    continue;
                }
                assert!(key_shortcut(ch(k), m).is_none(), "{k} with {m:?}");
            }
        }
    }

    #[test]
    fn bare_keys_and_the_modified_chords_still_work() {
        assert!(matches!(inner(key_shortcut(ch("r"), Modifiers::empty())), Some(Message::ResetEdits)));
        assert!(matches!(inner(key_shortcut(ch("3"), Modifiers::empty())), Some(Message::SetRating(3))));
        assert!(matches!(inner(key_shortcut(ch("R"), Modifiers::SHIFT)), Some(Message::ResetEdits)));
        assert!(matches!(key_shortcut(ch("?"), Modifiers::SHIFT), Some(Message::OpenModal(Modal::Help))));
        assert!(matches!(key_shortcut(ch("z"), Modifiers::CTRL), Some(Message::Undo)));
        assert!(matches!(key_shortcut(ch("z"), Modifiers::CTRL | Modifiers::SHIFT), Some(Message::Redo)));
        assert!(matches!(key_shortcut(ch("y"), Modifiers::CTRL), Some(Message::Redo)));
        assert!(matches!(key_shortcut(ch("c"), Modifiers::CTRL), Some(Message::CopySettings)));
        assert!(matches!(key_shortcut(ch("v"), Modifiers::CTRL), Some(Message::PasteSettings)));
        assert!(matches!(key_shortcut(ch("a"), Modifiers::CTRL), Some(Message::SelectAll)));
        assert!(matches!(key_shortcut(Key::Named(iced::keyboard::key::Named::ArrowRight), Modifiers::CTRL), Some(Message::SelectNextImage)));
    }
}
