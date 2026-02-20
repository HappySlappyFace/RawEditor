use crate::app::message::{AppTab, Message};
use crate::database;
use crate::database::models::Image as ImageData;
use crate::gpu;
use crate::raw;
use crate::ui::preview_renderer::CropHandle;
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
}

/// Phase 89: Export Format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Jpeg,
    Png,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportFormat::Jpeg => write!(f, "JPEG"),
            ExportFormat::Png => write!(f, "PNG"),
        }
    }
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

/// Main application state
#[derive(Debug)]
pub struct RawEditor {
    // Phase 84: Active modal overlay
    pub active_modal: Modal,

    // Phase 85: User-configurable cache capacity
    pub cache_capacity: usize,

    // Phase 88: Responsive Grid
    pub thumbnail_size: f32,

    // Phase 89: Export Studio
    pub export_settings: ExportSettings,
    pub export_queue: Vec<i64>,
    pub is_exporting: bool,

    /// The catalog database (Phase 23: Optional during startup)
    pub library: Option<database::library::Library>,
    /// Status message to display to the user
    pub status: String,
    /// All images loaded from the database
    pub images: Vec<ImageData>,
    /// Currently selected image ID
    pub selected_image_id: Option<i64>,
    /// Cache directory for full-size previews
    pub preview_cache_dir: PathBuf,
    /// Phase 73: RAM cache for 1280px previews (Look-Ahead)
    pub preview_cache: LruCache<i64, Handle>,
    /// Currently active tab
    pub current_tab: AppTab,
    /// Current edit parameters for the selected image
    pub current_edit_params: crate::core::types::EditParams,

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
    /// Phase 24: Before/After toggle (show original vs edited)
    pub show_before: bool,
    /// Phase 25: Zoom level (1.0 = 100%, 2.0 = 200%, etc.)
    pub zoom: f32,
    /// Phase 25: Pan offset in normalized coordinates
    pub pan_offset: cgmath::Vector2<f32>,
    /// Phase 25: Canvas cache for main image rendering
    pub canvas_cache: iced::widget::canvas::Cache,
    /// Phase 25: Drag state for panning
    pub is_dragging: bool,
    pub last_cursor_position: Option<Point>,
    /// Phase 26: Double-click detection
    pub last_click_time: Option<std::time::Instant>,
    /// Phase 26: Viewport size for zoom-to-cursor calculations (actual displayed size)
    pub viewport_size: (f32, f32), // (width, height) in screen pixels
    /// Phase 29: Instant preview handle (displayed while RAW loads)
    pub working_preview: Option<Handle>,
    /// Phase 103: High-quality rendered preview (async updated)
    pub rendered_preview: Option<Handle>,
    /// Phase 41: Current image metadata for inspection
    pub current_metadata: Option<raw::loader::RawDataResult>,
    /// Phase 54: Edit settings clipboard for copy/paste
    pub edit_clipboard: Option<crate::core::types::EditParams>,
    /// Phase 55: Multi-selection set (image IDs)
    pub multi_selection: HashSet<i64>,
    /// Phase 55: Track modifier keys for Ctrl/Cmd+Click
    pub last_modifiers: iced::keyboard::Modifiers,
    /// Phase 83: Auto-advance to next image after rating/flagging
    pub auto_advance: bool,
    /// Phase 59: Minimum rating filter (0 = show all, 1-5 = show rating or higher)
    pub min_filter_rating: u8,
    /// Phase 79: Modular Info Overlay state
    pub info_overlay: InfoOverlayState,
    /// Phase 65: Undo/Redo History Map<ImageID, (HistoryStack, CurrentIndex)>
    pub history_map: HashMap<i64, (Vec<crate::core::types::EditParams>, usize)>,
    /// Phase 67: Interactive crop mode
    pub is_cropping: bool,
    /// Phase 67: Drag mode for interaction
    pub drag_mode: DragMode,
    /// Phase 78: Async Task Deduplication (track pending background loads)
    pub pending_loads: HashSet<i64>,
    /// Phase 81: Throttled Image Loader queue
    /// Phase 81: Throttled Image Loader queue
    pub queued_loads: Vec<(i64, String)>,

    /// Phase 104: Performance Profiler
    pub profiler: crate::core::profiler::Profiler,
    pub show_profiler: bool,

    /// Phase 106: Render Throttling
    pub is_rendering_preview: bool,
    pub pending_preview_update: bool,
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

/// Phase 23: Async database loading
/// Loads the database and images in the background to avoid blocking the UI
/// Returns only the images Vec - Library will be created on main thread
async fn load_database_async() -> Result<Vec<ImageData>, String> {
    // Use spawn_blocking because rusqlite is synchronous
    tokio::task::spawn_blocking(|| {
        // Initialize the database
        let library = database::library::Library::new()
            .map_err(|e| format!("Failed to initialize database: {:?}", e))?;

        // Verify thumbnails exist on disk (reset if deleted)
        let _ = library.verify_thumbnails();

        // Verify RAW files exist on disk (mark as deleted if missing)
        let _ = library.verify_files();

        // Load all images from the database
        let images = library
            .get_all_images()
            .map_err(|e| format!("Failed to load images: {:?}", e))?;

        tracing::info!("RAW Editor initialized with {} images", images.len());

        Ok(images)
    })
    .await
    .map_err(|e| format!("Database task failed: {:?}", e))?
}

/// Phase 80: Configurable Cache Size
pub const PRELOAD_BEHIND: usize = 10;
pub const PRELOAD_AHEAD: usize = 50;
// Phase 81: Increased cache capacity
pub const CACHE_CAPACITY: usize = 200;

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

        // Determine the database path (e.g., in the application's data directory)
        let _db_path = database::library::Library::get_db_path();

        (
            RawEditor {
                library: None, // Phase 23: Database loads in background
                status: "Initializing...".to_string(),
                images: Vec::new(), // Empty until database loads
                selected_image_id: None,
                preview_cache_dir,
                preview_cache: LruCache::new(NonZeroUsize::new(CACHE_CAPACITY).unwrap()),
                current_tab: AppTab::Library, // Start in Library view
                current_edit_params: crate::core::types::EditParams::default(),

                // Phase 95: Unified GPU Pipeline Architecture
                gpu_context: None, // Will be initialized on first image load
                image_resources: None,
                editor_readiness: EditorReadiness::NoSelection,

                histogram_data: std::cell::RefCell::new([[0; 256]; 3]),
                histogram_cache: iced::widget::canvas::Cache::default(),
                histogram_enabled: true,
                show_before: false,
                zoom: 1.0,
                pan_offset: cgmath::Vector2::new(0.0, 0.0),
                canvas_cache: iced::widget::canvas::Cache::default(),
                is_dragging: false,
                last_cursor_position: None,
                last_click_time: None,
                viewport_size: (800.0, 400.0),
                working_preview: None,
                rendered_preview: None,
                current_metadata: None,
                edit_clipboard: None,
                multi_selection: HashSet::new(),
                last_modifiers: iced::keyboard::Modifiers::default(),
                auto_advance: false,
                min_filter_rating: 0,
                info_overlay: crate::app::state::InfoOverlayState::Metadata,
                // Phase 84
                active_modal: Modal::None,
                // Phase 85
                cache_capacity: 200,
                // Phase 88
                thumbnail_size: 220.0,
                // Phase 89
                export_settings: ExportSettings::default(),
                export_queue: Vec::new(),
                is_exporting: false,

                // Phase 104: Profiler
                profiler: crate::core::profiler::Profiler::new(),
                show_profiler: false,
                is_rendering_preview: false,
                pending_preview_update: false,

                // Background task
                // background_task: None, // This field is not defined in the struct
                history_map: HashMap::new(),
                is_cropping: false,
                drag_mode: DragMode::None,
                pending_loads: HashSet::new(),
                queued_loads: Vec::new(),
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
                (vec![self.current_edit_params.clone()], 0)
            });

            self.history_map.get_mut(&image_id)
        } else {
            None
        }
    }

    // Helper to push current state to history
    pub fn commit_current_state(&mut self) {
        let params_to_push = self.current_edit_params.clone();
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

    // Phase 67: Calculate image screen bounds for interaction
    pub fn get_image_screen_bounds(
        &self,
        resources: &crate::gpu::shared::ImageResources,
    ) -> Rectangle {
        let viewport_width = self.viewport_size.0;
        let viewport_height = self.viewport_size.1;

        // Use actual image aspect ratio
        let image_aspect = resources.width as f32 / resources.height as f32;
        let viewport_aspect = viewport_width / viewport_height;

        // Calculate fitted size (contain mode)
        let (fitted_width, fitted_height) = if image_aspect > viewport_aspect {
            let w = viewport_width;
            let h = w / image_aspect;
            (w, h)
        } else {
            let h = viewport_height;
            let w = h * image_aspect;
            (w, h)
        };

        let center_x = viewport_width / 2.0;
        let center_y = viewport_height / 2.0;

        let zoomed_width = fitted_width * self.zoom;
        let zoomed_height = fitted_height * self.zoom;

        // Apply pan offset (scaled by zoom)
        let pan_x = (self.pan_offset.x * fitted_width) / self.zoom;
        let pan_y = (self.pan_offset.y * fitted_height) / self.zoom;

        let image_x = center_x - (zoomed_width / 2.0) + pan_x;
        let image_y = center_y - (zoomed_height / 2.0) + pan_y;

        Rectangle {
            x: image_x,
            y: image_y,
            width: zoomed_width,
            height: zoomed_height,
        }
    }

    // Phase 67: Detect if cursor is over a crop handle
    pub fn detect_crop_handle(&self, cursor_pos: Point) -> Option<CropHandle> {
        if let Some(resources) = &self.image_resources {
            let bounds = self.get_image_screen_bounds(resources);
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
    pub fn save_current_edits(&self) {
        // Phase 23: Only save if database is loaded
        if let Some(library) = &self.library {
            if let Some(image_id) = self.selected_image_id {
                if let Err(e) = library.save_edit_params(image_id, &self.current_edit_params) {
                    tracing::error!("Failed to save edits for image {}: {:?}", image_id, e);
                } else {
                    tracing::info!("Saved edits for image {}", image_id);
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

            if let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event
            {
                // Phase 54: Settings Clipboard (Ctrl/Cmd+C/V)
                if modifiers.command() {
                    match key {
                        keyboard::Key::Character(c) if c == "c" || c == "C" => {
                            return Some(Message::CopySettings);
                        }
                        keyboard::Key::Character(c) if c == "v" || c == "V" => {
                            return Some(Message::PasteSettings);
                        }
                        _ => {}
                    }
                }

                match key {
                    keyboard::Key::Named(Named::F3) => Some(Message::ToggleProfiler),
                    keyboard::Key::Named(Named::Escape) => Some(Message::Escape),
                    keyboard::Key::Named(Named::Space) => Some(Message::ToggleBeforeAfter),
                    keyboard::Key::Named(Named::ArrowRight) => Some(Message::SelectNextImage),
                    keyboard::Key::Named(Named::ArrowLeft) => Some(Message::SelectPreviousImage),
                    keyboard::Key::Named(Named::Delete)
                    | keyboard::Key::Named(Named::Backspace) => Some(Message::DeleteImage),
                    // Phase 56: Rating Shortcuts (1-5)
                    keyboard::Key::Character(c) if c == "0" => Some(Message::SetRating(0)),
                    keyboard::Key::Character(c) if c == "1" => Some(Message::SetRating(1)),
                    keyboard::Key::Character(c) if c == "2" => Some(Message::SetRating(2)),
                    keyboard::Key::Character(c) if c == "3" => Some(Message::SetRating(3)),
                    keyboard::Key::Character(c) if c == "4" => Some(Message::SetRating(4)),
                    keyboard::Key::Character(c) if c == "5" => Some(Message::SetRating(5)),
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
                    keyboard::Key::Character(c) if c == "5" => Some(Message::SetRating(5)),
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
                    _ => None,
                }
            } else {
                None
            }
        });

        let loader_subscription = crate::app::loader::subscription(self.queued_loads.clone());

        iced::Subscription::batch(vec![keyboard_subscription, loader_subscription])
    }
}
