use std::path::PathBuf;
use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use iced::{Point, Rectangle, Task};
use iced::widget::image::Handle;
use crate::state;
use crate::gpu;
use crate::raw;
use crate::ui::preview_renderer::CropHandle;
use crate::app::message::{Message, AppTab};
use crate::state::data::Image as ImageData;

/// State of the editor and GPU pipeline
#[derive(Clone)]
pub enum EditorStatus {
    /// No image selected
    NoSelection,
    /// Loading RAW data and initializing GPU pipeline
    Loading(i64),
    /// GPU pipeline ready for rendering
    Ready(Arc<gpu::RenderPipeline>),
    /// Failed to initialize pipeline
    Failed(i64, String),
}

impl std::fmt::Debug for EditorStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditorStatus::NoSelection => write!(f, "NoSelection"),
            EditorStatus::Loading(id) => write!(f, "Loading({})", id),
            EditorStatus::Ready(_) => write!(f, "Ready(pipeline)"),
            EditorStatus::Failed(id, err) => write!(f, "Failed({}, {})", id, err),
        }
    }
}

/// Phase 67: Drag mode for mouse interaction
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragMode {
    None,
    Pan,
    CropHandle(CropHandle),
}

use lru::LruCache;
use std::num::NonZeroUsize;

/// Main application state
#[derive(Debug)]
pub struct RawEditor {
    /// The catalog database (Phase 23: Optional during startup)
    pub library: Option<state::library::Library>,
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
    pub current_edit_params: state::edit::EditParams,
    /// GPU pipeline status (holds the pipeline when ready)
    pub editor_status: EditorStatus,
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
    pub viewport_size: (f32, f32),  // (width, height) in screen pixels
    /// Phase 29: Instant preview handle (displayed while RAW loads)
    pub working_preview: Option<Handle>,
    /// Phase 41: Current image metadata for inspection
    pub current_metadata: Option<raw::loader::RawDataResult>,
    /// Phase 54: Edit settings clipboard for copy/paste
    pub edit_clipboard: Option<state::edit::EditParams>,
    /// Phase 55: Multi-selection set (image IDs)
    pub multi_selection: HashSet<i64>,
    /// Phase 55: Track modifier keys for Ctrl/Cmd+Click
    pub last_modifiers: iced::keyboard::Modifiers,
    /// Phase 59: Minimum rating filter (0 = show all, 1-5 = show rating or higher)
    pub min_filter_rating: u8,
    /// Phase 79: Modular Info Overlay state
    pub info_overlay: InfoOverlayState,
    /// Phase 65: Undo/Redo History Map<ImageID, (HistoryStack, CurrentIndex)>
    pub history_map: HashMap<i64, (Vec<state::edit::EditParams>, usize)>,
    /// Phase 67: Interactive crop mode
    pub is_cropping: bool,
    /// Phase 67: Drag mode for interaction
    pub drag_mode: DragMode,
    /// Phase 78: Async Task Deduplication (track pending background loads)
    pub pending_loads: HashSet<i64>,
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
        let library = state::library::Library::new()
            .map_err(|e| format!("Failed to initialize database: {:?}", e))?;
        
        // Verify thumbnails exist on disk (reset if deleted)
        let _ = library.verify_thumbnails();
        
        // Verify RAW files exist on disk (mark as deleted if missing)
        let _ = library.verify_files();
        
        // Load all images from the database
        let images = library.get_all_images()
            .map_err(|e| format!("Failed to load images: {:?}", e))?;
        
        println!("🎨 RAW Editor initialized with {} images", images.len());
        
        Ok(images)
    })
    .await
    .map_err(|e| format!("Database task failed: {:?}", e))?
}

/// Phase 80: Configurable Cache Size
pub const PRELOAD_BEHIND: usize = 10;
pub const PRELOAD_AHEAD: usize = 50;
// Capacity should be enough for behind + ahead + current + some buffer
pub const CACHE_CAPACITY: usize = PRELOAD_BEHIND + PRELOAD_AHEAD + 5;

impl RawEditor {
    pub fn title(&self) -> String {
        String::from("RAW Editor")
    }

    /// Phase 23: Create a new instance of the application (INSTANT!)
    /// The database now loads in the background to show splash screen immediately
    pub fn new() -> (Self, Task<Message>) {
        println!("🚀 RAW Editor starting (instant splash screen)...");
        
        // Initialize preview cache directory (fast)
        let preview_cache_dir = raw::preview::get_preview_cache_dir();
        
        // Determine the database path (e.g., in the application's data directory)
        let _db_path = state::library::Library::get_db_path();

        (
            RawEditor { 
                library: None, // Phase 23: Database loads in background
                status: "Initializing...".to_string(),
                images: Vec::new(), // Empty until database loads
                selected_image_id: None,
                preview_cache_dir,
                preview_cache: LruCache::new(NonZeroUsize::new(CACHE_CAPACITY).unwrap()),
                current_tab: AppTab::Library, // Start in Library view
                current_edit_params: state::edit::EditParams::default(),
                editor_status: EditorStatus::NoSelection,
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
                current_metadata: None,
                edit_clipboard: None,
                multi_selection: HashSet::new(),
                last_modifiers: iced::keyboard::Modifiers::default(),
                min_filter_rating: 0,  // Phase 59: Start with "show all"
                info_overlay: Default::default(),
                history_map: HashMap::new(), // Phase 65: Undo/Redo History
                is_cropping: false, // Phase 67: Interactive Crop
                drag_mode: DragMode::None, // Phase 67: Interactive Crop
                pending_loads: HashSet::new(),
            },
            // Phase 23: Trigger database loading in background
            Task::perform(
                load_database_async(), 
                Message::DatabaseLoaded
            )
        )
    }

    // Phase 65: Undo/Redo Helper
    // Returns a mutable reference to the (Stack, Index) tuple for the current image.
    // Initializes it if missing.
    pub fn get_current_history(&mut self) -> Option<&mut (Vec<state::edit::EditParams>, usize)> {
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
            
            println!("📝 History Commit: Stack size {}, Index {}", stack.len(), *index);
        }
    }

    // Phase 67: Calculate image screen bounds for interaction
    pub fn get_image_screen_bounds(&self, pipeline: &gpu::RenderPipeline) -> Rectangle {
        let viewport_width = self.viewport_size.0;
        let viewport_height = self.viewport_size.1;
        
        // Use actual image aspect ratio
        let image_aspect = pipeline.width as f32 / pipeline.height as f32;
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
        if let EditorStatus::Ready(pipeline) = &self.editor_status {
            let bounds = self.get_image_screen_bounds(pipeline);
            let crop = self.current_edit_params.crop;
            
            let crop_x = bounds.x + (crop[0] * bounds.width);
            let crop_y = bounds.y + (crop[1] * bounds.height);
            let crop_w = crop[2] * bounds.width;
            let crop_h = crop[3] * bounds.height;
            
            let handle_radius = 15.0; // Hitbox radius (generous)
            
            let check_handle = |x, y| {
                let dx = cursor_pos.x - x;
                let dy = cursor_pos.y - y;
                (dx*dx + dy*dy) < handle_radius * handle_radius
            };
            
            if check_handle(crop_x, crop_y) { return Some(CropHandle::TopLeft); }
            if check_handle(crop_x + crop_w, crop_y) { return Some(CropHandle::TopRight); }
            if check_handle(crop_x, crop_y + crop_h) { return Some(CropHandle::BottomLeft); }
            if check_handle(crop_x + crop_w, crop_y + crop_h) { return Some(CropHandle::BottomRight); }
            
            // Phase 70: Check if inside body
            if cursor_pos.x >= crop_x && cursor_pos.x <= crop_x + crop_w &&
               cursor_pos.y >= crop_y && cursor_pos.y <= crop_y + crop_h {
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
                    eprintln!("⚠️  Failed to save edits for image {}: {:?}", image_id, e);
                } else {
                    println!("💾 Saved edits for image {}", image_id);
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
    pub fn subscription(&self) -> iced::Subscription<Message> {
        use iced::keyboard;
        use iced::keyboard::key::Named;
        
        iced::event::listen_with(|event, _status, _window| {
            // Phase 55: Track modifier key changes for Ctrl/Cmd+Click
            if let iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
                return Some(Message::ModifiersChanged(modifiers));
            }
            
            if let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event {
                // Phase 54: Settings Clipboard (Ctrl/Cmd+C/V)
                if modifiers.command() {
                    match key {
                        keyboard::Key::Character(c) if c == "c" || c == "C" => {
                            return Some(Message::CopySettings);
                        }
                        keyboard::Key::Character(c) if c == "v" || c == "V" => {
                            return Some(Message::PasteSettings);
                        }
                        // Phase 65: Undo/Redo (Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z)
                        keyboard::Key::Character(c) if c == "z" || c == "Z" => {
                            if modifiers.shift() {
                                return Some(Message::Redo);
                            } else {
                                return Some(Message::Undo);
                            }
                        }
                        keyboard::Key::Character(c) if c == "y" || c == "Y" => {
                            return Some(Message::Redo);
                        }
                        _ => {}
                    }
                }

                match key {
                    keyboard::Key::Named(Named::Space) => Some(Message::ToggleBeforeAfter),
                    keyboard::Key::Named(Named::ArrowRight) => Some(Message::SelectNextImage),
                    keyboard::Key::Named(Named::ArrowLeft) => Some(Message::SelectPreviousImage),
                    // Phase 56: Rating Shortcuts (1-5)
                    keyboard::Key::Character(c) if c == "0" => Some(Message::SetRating(0)),
                    keyboard::Key::Character(c) if c == "1" => Some(Message::SetRating(1)),
                    keyboard::Key::Character(c) if c == "2" => Some(Message::SetRating(2)),
                    keyboard::Key::Character(c) if c == "3" => Some(Message::SetRating(3)),
                    keyboard::Key::Character(c) if c == "4" => Some(Message::SetRating(4)),
                    keyboard::Key::Character(c) if c == "5" => Some(Message::SetRating(5)),
                    // Phase 60: HUD Toggle
                    keyboard::Key::Character(c) if c == "i" || c == "I" => Some(Message::ToggleInfoHud),

                    _ => None,
                }
            } else {
                None
            }
        })
    }
}
