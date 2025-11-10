use iced::{Background, Border, Color, Element, Task, Theme, Point};
use iced::widget::{button, column, container, row, scrollable, text, Image, slider, canvas};
use iced::{Alignment, Length};
use iced::widget::image::Handle;
use iced_aw::Wrap;
use iced::window;
use rfd::FileDialog;
use rusqlite::{Connection, ErrorCode};
use std::path::PathBuf;
use std::sync::Arc;
use walkdir::WalkDir;
use chrono::Utc;
// use crate::canvas;

// Declare the state, raw, gpu, and ui modules
mod state;
mod raw;
mod gpu;
mod ui;
mod color;  // Phase 15: Color space conversion utilities

// Import shared data structures (alias to avoid conflict with iced's image widget)
use state::data::Image as ImageData;

// Phase 15: Color space conversion
use color::calculate_cam_to_srgb_matrix;

/// Result of a folder import operation
#[derive(Debug, Clone)]
struct ImportResult {
    imported_count: usize,
    skipped_count: usize,
}

/// Result of thumbnail generation
#[derive(Debug, Clone)]
struct ThumbnailResult {
    generated_count: usize,
}

/// Application tabs/modules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppTab {
    Library,  // Browse, import, organize images
    Develop,  // Edit selected image with full preview
}

/// Result of preview generation
#[derive(Debug, Clone)]
struct PreviewResult {
    image_id: i64,
    preview_path: Result<String, String>,
}

/// State of the editor and GPU pipeline
#[derive(Clone)]
enum EditorStatus {
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

/// Main application state
struct RawEditor {
    /// The catalog database (Phase 23: Optional during startup)
    library: Option<state::library::Library>,
    /// Status message to display to the user
    status: String,
    /// All images loaded from the database
    images: Vec<ImageData>,
    /// Currently selected image ID
    selected_image_id: Option<i64>,
    /// Cache directory for full-size previews
    preview_cache_dir: PathBuf,
    /// Currently active tab
    current_tab: AppTab,
    /// Current edit parameters for the selected image
    current_edit_params: state::edit::EditParams,
    /// GPU pipeline status (holds the pipeline when ready)
    editor_status: EditorStatus,
    /// Phase 21: Histogram data [R[256], G[256], B[256]]
    histogram_data: std::cell::RefCell<[[u32; 256]; 3]>,
    /// Phase 21: Histogram canvas cache
    histogram_cache: iced::widget::canvas::Cache,
    /// Phase 22: Histogram toggle (keep for user control)
    histogram_enabled: bool,
    /// Phase 24: Before/After toggle (show original vs edited)
    show_before: bool,
    /// Phase 25: Zoom level (1.0 = 100%, 2.0 = 200%, etc.)
    zoom: f32,
    /// Phase 25: Pan offset in normalized coordinates
    pan_offset: cgmath::Vector2<f32>,
    /// Phase 25: Canvas cache for main image rendering
    canvas_cache: iced::widget::canvas::Cache,
    /// Phase 25: Drag state for panning
    is_dragging: bool,
    last_cursor_position: Option<Point>,
    /// Phase 26: Double-click detection
    last_click_time: Option<std::time::Instant>,
    /// Phase 26: Viewport size for zoom-to-cursor calculations (actual displayed size)
    viewport_size: (f32, f32),  // (width, height) in screen pixels
}

/// Application messages (events)
#[derive(Debug, Clone)]
enum Message {
    // ========== Startup Messages (Phase 23) ==========
    /// Database loading completed (async background task)
    /// Phase 23: Only send images Vec, Library created on main thread (not Send)
    DatabaseLoaded(Result<Vec<ImageData>, String>),
    
    /// User clicked the "Import Folder" button
    ImportFolder,
    /// Background import completed with results
    ImportComplete(ImportResult),
    /// Background thumbnail generation completed
    ThumbnailGenerated(ThumbnailResult),
    /// User selected an image from the grid
    ImageSelected(i64),
    /// Background preview generation completed
    PreviewGenerated(PreviewResult),
    /// User switched to a different tab
    TabChanged(AppTab),
    
    // ========== Edit Parameter Changes ==========
    /// User changed exposure slider
    ExposureChanged(f32),
    /// User changed contrast slider
    ContrastChanged(f32),
    /// User changed highlights slider
    HighlightsChanged(f32),
    /// User changed shadows slider
    ShadowsChanged(f32),
    /// User changed whites slider
    WhitesChanged(f32),
    /// User changed blacks slider
    BlacksChanged(f32),
    /// User changed vibrance slider
    VibranceChanged(f32),
    /// User changed saturation slider
    SaturationChanged(f32),
    /// User changed temperature slider (Phase 18)
    TemperatureChanged(f32),
    /// User changed tint slider (Phase 18)
    TintChanged(f32),
    /// User clicked Reset button to clear all edits
    ResetEdits,
    
    // ========== Phase 24: Workflow Messages ==========
    /// Toggle Before/After view (Spacebar)
    ToggleBeforeAfter,
    /// Select next image (Right arrow)
    SelectNextImage,
    /// Select previous image (Left arrow)
    SelectPreviousImage,
    
    // ========== Phase 25: Zoom & Pan Messages ==========
    /// User zoomed with mouse wheel (delta, cursor position)
    Zoom(f32, Point),
    /// User panned with mouse drag (delta in screen space)
    Pan(cgmath::Vector2<f32>),
    /// Mouse button pressed - start dragging
    MousePressed,
    /// Mouse button released - stop dragging
    MouseReleased,
    /// Mouse moved - track for panning
    MouseMoved(Point),
    
    // ========== Phase 26: Advanced Zoom Polish ==========
    /// Reset zoom and pan to default (1.0, 0.0)
    ResetView,
    
    // ========== GPU Pipeline Messages ==========
    /// Background RAW data loading completed
    RawDataLoaded(Result<raw::loader::RawDataResult, String>),
    /// GPU pipeline initialization completed
    GpuPipelineReady(Result<Arc<gpu::RenderPipeline>, String>),
    
    // ========== Export Messages (Phase 19) ==========
    /// User clicked Export button
    ExportImage,
    /// Background export completed
    ExportComplete(Result<std::path::PathBuf, String>),
    
    // ========== Histogram Messages (Phase 22) ==========
    /// User toggled histogram on/off
    HistogramToggled(bool),
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

impl RawEditor {
    /// Phase 23: Create a new instance of the application (INSTANT!)
    /// The database now loads in the background to show splash screen immediately
    fn new() -> (Self, Task<Message>) {
        println!("🚀 RAW Editor starting (instant splash screen)...");
        
        // Initialize preview cache directory (fast)
        let preview_cache_dir = raw::preview::get_preview_cache_dir();
        
        (
            RawEditor { 
                library: None, // Phase 23: Database loads in background
                status: "Loading database...".to_string(),
                images: Vec::new(), // Empty until database loads
                selected_image_id: None,
                preview_cache_dir,
                current_tab: AppTab::Library,
                current_edit_params: state::edit::EditParams::default(),
                editor_status: EditorStatus::NoSelection,
                histogram_data: std::cell::RefCell::new([[0; 256]; 3]),
                histogram_cache: iced::widget::canvas::Cache::default(),
                histogram_enabled: false, // Phase 22: Off by default
                show_before: false, // Phase 24: Show edited version by default
                zoom: 1.0, // Phase 25: Start at 100% zoom
                pan_offset: cgmath::Vector2::new(0.0, 0.0), // Phase 25: Centered
                canvas_cache: iced::widget::canvas::Cache::default(), // Phase 25: Canvas cache
                is_dragging: false, // Phase 25: Not dragging initially
                last_cursor_position: None, // Phase 25: No cursor position yet
                last_click_time: None, // Phase 26: No click yet
                viewport_size: (1280.0, 854.0), // Phase 26: Default viewport size (will be updated)
            },
            // Phase 23: Load database in background
            Task::perform(
                load_database_async(),
                Message::DatabaseLoaded,
            ),
        )
    }

    /// Handle application messages and update state
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // Phase 23: Handle database loading completion
            Message::DatabaseLoaded(result) => {
                match result {
                    Ok(images) => {
                        // Create Library on main thread (can't be sent across threads)
                        match state::library::Library::new() {
                            Ok(library) => {
                                let image_count = images.len();
                                self.library = Some(library);
                                self.images = images;
                                self.status = format!("Loaded {} images.", image_count);
                                println!("✅ Database loaded successfully ({} images)", image_count);
                                
                                // Phase 23: Maximize window using native OS maximize
                                use iced::window;
                                // let maximize_window = window::get_latest()
                                //     .and_then(|id| window::change_mode(id, window::Mode::Maximized));
                                let maximize_window =window::get_latest()
                                    .and_then(|id| window::maximize(id, true));

                                println!("🔲 Maximizing window...");
                                
                                // Start thumbnail generation now that database is ready
                                if let Some(lib) = &self.library {
                                    let db_path = lib.path().clone();
                                    return Task::batch(vec![
                                        maximize_window,
                                        Task::perform(
                                            generate_thumbnails_async(db_path),
                                            Message::ThumbnailGenerated,
                                        ),
                                    ]);
                                }
                                
                                // Just maximize if no thumbnails to generate
                                return maximize_window;
                            }
                            Err(e) => {
                                self.status = format!("Failed to create library: {:?}", e);
                                eprintln!("❌ Failed to create library: {:?}", e);
                            }
                        }
                    }
                    Err(e) => {
                        self.status = format!("Failed to load database: {}", e);
                        eprintln!("❌ Database loading failed: {}", e);
                    }
                }
                Task::none()
            }
            
            Message::ImportFolder => {
                // Phase 23: Only allow imports if database is loaded
                if let Some(library) = &self.library {
                    // Show the native folder picker dialog
                    let folder = FileDialog::new()
                        .set_title("Select Folder with RAW Photos")
                        .pick_folder();
                    
                    if let Some(folder_path) = folder {
                        // Update status to show we're importing
                        self.status = format!("Importing from {}...", folder_path.display());
                        
                        // Get the database path for the background thread
                        let db_path = library.path().clone();
                        
                        // Launch async import task
                        return Task::perform(
                            import_folder_async(folder_path, db_path),
                            Message::ImportComplete,
                        );
                    }
                }
                
                Task::none()
            }
            Message::ImportComplete(result) => {
                // Phase 23: Only process if database is loaded
                if let Some(library) = &self.library {
                    // Reload images from database to show newly imported files
                    self.images = library.get_all_images().unwrap_or_default();
                    
                    // Update status with import results
                    self.status = format!(
                        "✅ Import complete! Added {} images, skipped {} duplicates. Total: {} images.",
                        result.imported_count, result.skipped_count, self.images.len()
                    );
                    
                    println!(
                        "📊 Import summary: {} new, {} skipped, {} total",
                        result.imported_count, result.skipped_count, self.images.len()
                    );
                    
                    // Start thumbnail generation for newly imported images
                    let db_path = library.path().clone();
                    return Task::perform(
                        generate_thumbnails_async(db_path),
                        Message::ThumbnailGenerated,
                    );
                }
                Task::none()
            }
            Message::ThumbnailGenerated(result) => {
                // Phase 23: Only process if database is loaded
                if let Some(library) = &self.library {
                    // Always reload images to show updated thumbnail in the grid
                    self.images = library.get_all_images().unwrap_or_default();
                    
                    // Check both fast and slow queues
                    let fast_queue_count: i64 = library.conn()
                        .query_row(
                            "SELECT COUNT(*) FROM images WHERE cache_status = 'pending'",
                            [],
                            |row| row.get(0)
                        )
                        .unwrap_or(0);
                    
                    let slow_queue_count: i64 = library.conn()
                        .query_row(
                            "SELECT COUNT(*) FROM images WHERE cache_status = 'needs_slow'",
                            [],
                            |row| row.get(0)
                        )
                        .unwrap_or(0);
                    
                    if fast_queue_count > 0 {
                        // Still processing fast queue (high priority)
                        self.status = format!(
                            "⚡ Fast queue: {} remaining (slow queue: {})", 
                            fast_queue_count, slow_queue_count
                        );
                        
                        let db_path = library.path().clone();
                        return Task::perform(
                            generate_thumbnails_async(db_path),
                            Message::ThumbnailGenerated,
                        );
                } else if slow_queue_count > 0 {
                    // Fast queue empty, processing slow queue (low priority)
                    self.status = format!(
                        "🔥 Slow queue: {} remaining (RAW decode)", 
                        slow_queue_count
                    );
                    
                        let db_path = library.path().clone();
                        return Task::perform(
                            generate_thumbnails_async(db_path),
                            Message::ThumbnailGenerated,
                        );
                    } else {
                        // Both queues empty - all done!
                        self.status = format!("✅ All thumbnails generated! ({} images)", self.images.len());
                    }
                }
                
                Task::none()
            }
            Message::ImageSelected(image_id) => {
                // Phase 20: INSTANT selection - just update state, don't load anything!
                // Loading is deferred until user switches to Develop tab
                self.selected_image_id = Some(image_id);
                println!("✨ Selected image ID: {} (instant!)", image_id);
                
                // Phase 25: Clear canvas cache since we're switching to a different image
                self.canvas_cache.clear();
                
                // Phase 23: Load edit parameters from database (only if loaded)
                if let Some(library) = &self.library {
                    self.current_edit_params = library.load_edit_params(image_id)
                        .unwrap_or_else(|_| state::edit::EditParams::default());
                    
                    if !self.current_edit_params.is_unedited() {
                        println!("📝 Loaded existing edits for image {}", image_id);
                    }
                }
                
                // Phase 24: If already on Develop tab, reload RAW data for new image
                if self.current_tab == AppTab::Develop {
                    // Check if pipeline needs to be loaded for this image
                    let needs_load = match &self.editor_status {
                        EditorStatus::Ready(pipeline) => pipeline.image_id != image_id,
                        EditorStatus::Loading(id) => *id != image_id,
                        _ => true,  // NoSelection or Failed
                    };
                    
                    if needs_load {
                        println!("🔄 Loading RAW data for image {}...", image_id);
                        
                        // Find the image and start loading
                        if let Some(img) = self.images.iter().find(|i| i.id == image_id) {
                            let raw_path = img.path.clone();
                            
                            // Set editor status to loading
                            self.editor_status = EditorStatus::Loading(image_id);
                            
                            // Load RAW sensor data for GPU processing
                            return Task::perform(
                                raw::loader::load_raw_data(raw_path),
                                Message::RawDataLoaded,
                            );
                        }
                    } else {
                        println!("⚡ Pipeline already loaded for image {}", image_id);
                    }
                }
                
                Task::none()
            }
            Message::PreviewGenerated(result) => {
                // Phase 23: Update database with preview path for thumbnails (only if loaded)
                if let Some(library) = &self.library {
                    if let Ok(ref path) = result.preview_path {
                        let _ = library.set_image_preview_path(result.image_id, path);
                        
                        // Update in-memory image data
                        if let Some(img) = self.images.iter_mut().find(|i| i.id == result.image_id) {
                            img.preview_path = Some(path.clone());
                        }
                        
                        println!("✅ Preview cached for image {}", result.image_id);
                    } else if let Err(ref err) = result.preview_path {
                        eprintln!("❌ Preview failed for image {}: {}", result.image_id, err);
                    }
                }
                
                Task::none()
            }
            Message::TabChanged(tab) => {
                // Phase 20: Deferred loading trigger!
                self.current_tab = tab;
                
                // Only load when switching TO Develop tab (not FROM it)
                if tab == AppTab::Develop {
                    if let Some(image_id) = self.selected_image_id {
                        // Check if pipeline is already loaded for THIS specific image
                        let needs_load = match &self.editor_status {
                            EditorStatus::Ready(pipeline) => pipeline.image_id != image_id,
                            EditorStatus::Loading(id) => *id != image_id,
                            _ => true,  // NoSelection or Failed
                        };
                        
                        if needs_load {
                            println!("🔄 Switching to Develop tab - loading image {}...", image_id);
                            
                            // Find the image and start loading
                            if let Some(img) = self.images.iter().find(|i| i.id == image_id) {
                                let raw_path = img.path.clone();
                                
                                // Set editor status to loading
                                self.editor_status = EditorStatus::Loading(image_id);
                                
                                // Load RAW sensor data for GPU processing (this is the slow 3-second operation)
                                return Task::perform(
                                    raw::loader::load_raw_data(raw_path),
                                    Message::RawDataLoaded,
                                );
                            }
                        } else {
                            println!("⚡ Pipeline already loaded for image {}", image_id);
                        }
                    }
                }
                
                Task::none()
            }
            
            // ========== Edit Parameter Slider Handlers ==========
            
            Message::ExposureChanged(value) => {
                self.current_edit_params.exposure = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::ContrastChanged(value) => {
                self.current_edit_params.contrast = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::HighlightsChanged(value) => {
                self.current_edit_params.highlights = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::ShadowsChanged(value) => {
                self.current_edit_params.shadows = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::WhitesChanged(value) => {
                self.current_edit_params.whites = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::BlacksChanged(value) => {
                self.current_edit_params.blacks = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::VibranceChanged(value) => {
                self.current_edit_params.vibrance = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::SaturationChanged(value) => {
                self.current_edit_params.saturation = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::TemperatureChanged(value) => {
                self.current_edit_params.temperature = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::TintChanged(value) => {
                self.current_edit_params.tint = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::ResetEdits => {
                // Reset all edit parameters to default
                self.current_edit_params.reset();
                
                // Phase 23: Save to database (or delete the edit record, only if loaded)
                if let Some(library) = &self.library {
                    if let Some(image_id) = self.selected_image_id {
                        let _ = library.delete_edits(image_id);
                        println!("♻️  Reset edits for image {}", image_id);
                    }
                }
                
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                    self.histogram_cache.clear(); // Phase 24: Clear histogram cache
                }
                
                Task::none()
            }
            
            // ========== Phase 24: Workflow Message Handlers ==========
            
            Message::ToggleBeforeAfter => {
                // Toggle between edited and original (default params)
                self.show_before = !self.show_before;
                self.histogram_cache.clear(); // Histogram must update
                println!("{} {}", 
                    if self.show_before { "👁️  Showing" } else { "✏️  Showing" },
                    if self.show_before { "BEFORE (original)" } else { "AFTER (edited)" }
                );
                Task::none()
            }
            
            Message::SelectNextImage => {
                // Find current image index and select next
                if let Some(current_id) = self.selected_image_id {
                    if let Some(current_idx) = self.images.iter().position(|img| img.id == current_id) {
                        let next_idx = (current_idx + 1) % self.images.len();
                        let next_id = self.images[next_idx].id;
                        println!("⏭️  Next image: {} ({}/{})", next_id, next_idx + 1, self.images.len());
                        return self.update(Message::ImageSelected(next_id));
                    }
                }
                Task::none()
            }
            
            Message::SelectPreviousImage => {
                // Find current image index and select previous
                if let Some(current_id) = self.selected_image_id {
                    if let Some(current_idx) = self.images.iter().position(|img| img.id == current_id) {
                        let prev_idx = if current_idx == 0 { self.images.len() - 1 } else { current_idx - 1 };
                        let prev_id = self.images[prev_idx].id;
                        println!("⏮️  Previous image: {} ({}/{})", prev_id, prev_idx + 1, self.images.len());
                        return self.update(Message::ImageSelected(prev_id));
                    }
                }
                Task::none()
            }
            
            // ========== Phase 25: Zoom & Pan Message Handlers ==========

            Message::Zoom(delta, mut cursor_pos) => {
                // Phase 26: Zoom to cursor position (not center)
                
                // Get cursor position (use last known if sentinel value)
                if cursor_pos.x < 0.0 || cursor_pos.y < 0.0 {
                    cursor_pos = self.last_cursor_position.unwrap_or(Point::ORIGIN);
                }
                
                // Get pipeline dimensions for calculations
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    let old_zoom = self.zoom;
                    
                    // Phase 26: Calculate actual image position in viewport (centered)
                    let image_width = pipeline.preview_width as f32;
                    let image_height = pipeline.preview_height as f32;
                    let viewport_width = self.viewport_size.0;
                    let viewport_height = self.viewport_size.1;
                    
                    // Image is centered in viewport, calculate offsets
                    let x_offset = (viewport_width - image_width) / 2.0;
                    let y_offset = (viewport_height - image_height) / 2.0;
                    
                    // Convert viewport cursor position to image-relative position
                    let image_cursor_x = cursor_pos.x - x_offset;
                    let image_cursor_y = cursor_pos.y - y_offset;
                    
                    // Debug: Show offset calculation (helpful for diagnosing drift)
                    if false {  // Set to true for debugging
                        println!("📐 Zoom @ cursor: Viewport={:.0}x{:.0} Image={:.0}x{:.0} Offset=({:.1},{:.1})",
                            viewport_width, viewport_height, image_width, image_height, x_offset, y_offset);
                    }
                    
                    // Skip if cursor is far outside the image (allow small margins for edge precision)
                    let margin = 5.0; // Small margin in pixels
                    if image_cursor_x < -margin || image_cursor_y < -margin || 
                       image_cursor_x > image_width + margin || image_cursor_y > image_height + margin {
                        println!("⚠️  Cursor outside image, skipping zoom-to-cursor");
                        // Just do regular zoom without pan adjustment
                        if delta > 0.0 {
                            self.zoom *= 1.0 + (delta * 0.8);
                        } else {
                            self.zoom /= 1.0 + (-delta * 0.8);
                        }
                        self.zoom = self.zoom.clamp(0.1, 10.0);
                        self.canvas_cache.clear();
                        return Task::none();
                    }
                    
                    // Clamp cursor to image bounds for calculation
                    let image_cursor_x = image_cursor_x.clamp(0.0, image_width);
                    let image_cursor_y = image_cursor_y.clamp(0.0, image_height);
                    
                    // Calculate new zoom (exponential scaling)
                    let new_zoom = if delta > 0.0 {
                        old_zoom * (1.0 + delta * 0.8)  // Zoom in
                    } else {
                        old_zoom / (1.0 + (-delta * 0.8))  // Zoom out
                    };
                    self.zoom = new_zoom.clamp(0.1, 10.0);
                    
                    // Zoom-to-cursor math (matching shader transformation):
                    // Shader: tex = ((screen - 0.5) / zoom - pan) + 0.5
                    
                    // 1. Convert cursor position to normalized image coordinates (0-1)
                    let norm_cursor_x = image_cursor_x / image_width;
                    let norm_cursor_y = image_cursor_y / image_height;
                    
                    // 2. Find texture point under cursor BEFORE zoom
                    // tex = ((cursor - 0.5) / old_zoom - old_pan) + 0.5
                    let tex_x = ((norm_cursor_x - 0.5) / old_zoom - self.pan_offset.x) + 0.5;
                    let tex_y = ((norm_cursor_y - 0.5) / old_zoom - self.pan_offset.y) + 0.5;
                    
                    // 3. Calculate new pan so same texture point appears under cursor AFTER zoom
                    // We want: cursor = ((tex - 0.5) / new_zoom - new_pan) + 0.5
                    // Rearranging: new_pan = (tex - 0.5) / new_zoom - (cursor - 0.5)
                    // Wait, that's wrong. Let me rederive:
                    // cursor = ((tex - 0.5 - new_pan * new_zoom) / new_zoom) + 0.5
                    // No wait, the shader is: tex = ((screen - 0.5) / zoom - pan) + 0.5
                    // So inverse: screen = (tex - 0.5 + pan) * zoom + 0.5
                    // We want: cursor = (tex - 0.5 + new_pan) * new_zoom + 0.5
                    // Solving for new_pan:
                    // cursor - 0.5 = (tex - 0.5 + new_pan) * new_zoom
                    // (cursor - 0.5) / new_zoom = tex - 0.5 + new_pan
                    // new_pan = (cursor - 0.5) / new_zoom - tex + 0.5
                    
                    self.pan_offset.x = (norm_cursor_x - 0.5) / self.zoom - tex_x + 0.5;
                    self.pan_offset.y = (norm_cursor_y - 0.5) / self.zoom - tex_y + 0.5;
                    
                    println!("🔍 Zoom: {:.1}% (at cursor)", self.zoom * 100.0);
                } else {
                    // No pipeline loaded, just do simple zoom
                    if delta > 0.0 {
                        self.zoom *= 1.0 + (delta * 0.8);
                    } else {
                        self.zoom /= 1.0 + (-delta * 0.8);
                    }
                    self.zoom = self.zoom.clamp(0.1, 10.0);
                    println!("🔍 Zoom: {:.1}%", self.zoom * 100.0);
                }
                
                // Invalidate canvas cache to trigger redraw
                self.canvas_cache.clear();
                
                Task::none()
            }
            
            Message::ResetView => {
                // Phase 26: Reset zoom and pan to default
                self.zoom = 1.0;
                self.pan_offset = cgmath::Vector2::new(0.0, 0.0);
                self.canvas_cache.clear();
                println!("🔄 View reset: 100% zoom, centered");
                Task::none()
            }
            
            Message::Pan(delta) => {
                // Phase 25: Apply pan delta scaled by zoom (so panning speed feels consistent)
                // Scale by 1/zoom so panning at high zoom feels same speed as low zoom
                let scale = 1.0 / self.zoom;
                self.pan_offset.x += delta.x * scale;
                self.pan_offset.y += delta.y * scale;
                println!("🖐️  Pan: ({:.3}, {:.3}) at zoom {:.1}%", 
                    self.pan_offset.x, self.pan_offset.y, self.zoom * 100.0);
                
                // Invalidate canvas cache to trigger redraw
                self.canvas_cache.clear();
                
                Task::none()
            }
            
            Message::MousePressed => {
                // Phase 26: Detect double-click for reset view
                let now = std::time::Instant::now();
                let is_double_click = if let Some(last_click) = self.last_click_time {
                    now.duration_since(last_click).as_millis() < 300  // 300ms threshold
                } else {
                    false
                };
                
                self.last_click_time = Some(now);
                
                if is_double_click {
                    // Double-click detected - reset view
                    println!("👆 Double-click detected!");
                    return self.update(Message::ResetView);
                }
                
                // Single click - start dragging for panning
                self.is_dragging = true;
                // Position will be updated by next MouseMoved event
                Task::none()
            }
            
            Message::MouseReleased => {
                // Stop dragging
                self.is_dragging = false;
                self.last_cursor_position = None;
                Task::none()
            }
            
            Message::MouseMoved(current_position) => {
                // Phase 26: Update viewport size estimate
                // Learn the viewport size by tracking the maximum mouse coordinates
                // But don't let it shrink (only grow when we see larger coordinates)
                let new_viewport_w = (current_position.x * 1.01).max(self.viewport_size.0);
                let new_viewport_h = (current_position.y * 1.01).max(self.viewport_size.1);
                
                // Only update if change is significant (avoid tiny fluctuations)
                if (new_viewport_w - self.viewport_size.0).abs() > 10.0 {
                    self.viewport_size.0 = new_viewport_w;
                }
                if (new_viewport_h - self.viewport_size.1).abs() > 10.0 {
                    self.viewport_size.1 = new_viewport_h;
                }
                
                // If dragging, calculate pan delta and send Pan message
                if self.is_dragging {
                    if let Some(last_pos) = self.last_cursor_position {
                        // Calculate delta in screen pixels
                        let delta_x = current_position.x - last_pos.x;
                        let delta_y = current_position.y - last_pos.y;
                        
                        // Phase 26: Pan sensitivity using image dimensions (not viewport)
                        // Pan offset is in normalized image coordinates
                        let (sensitivity_x, sensitivity_y) = if let EditorStatus::Ready(pipeline) = &self.editor_status {
                            (
                                1.0 / pipeline.preview_width as f32,
                                1.0 / pipeline.preview_height as f32,
                            )
                        } else {
                            (0.001, 0.001)
                        };
                        
                        let delta = cgmath::Vector2::new(
                            delta_x * sensitivity_x,
                            delta_y * sensitivity_y,
                        );
                        
                        // Update cursor position AFTER calculating delta
                        self.last_cursor_position = Some(current_position);
                        
                        // Send Pan message
                        return self.update(Message::Pan(delta));
                    }
                }
                
                // Store cursor position for zoom-to-cursor (if not dragging)
                self.last_cursor_position = Some(current_position);
                Task::none()
            }
            
            // ========== GPU Pipeline Message Handlers ==========
            
            Message::RawDataLoaded(result) => {
                match result {
                    Ok(raw_data) => {
                        println!("📷 RAW data loaded: {}x{} pixels", raw_data.width, raw_data.height);
                        
                        // Phase 15: Calculate proper cam-to-sRGB color matrix
                        let xyz_to_cam = raw_data.color_matrix;
                        let cam_to_srgb = calculate_cam_to_srgb_matrix(xyz_to_cam);
                        println!("🎨 CAM-to-sRGB Matrix: [{:.3}, {:.3}, {:.3}]", 
                            cam_to_srgb[0], cam_to_srgb[1], cam_to_srgb[2]);
                        println!("                      [{:.3}, {:.3}, {:.3}]", 
                            cam_to_srgb[3], cam_to_srgb[4], cam_to_srgb[5]);
                        println!("                      [{:.3}, {:.3}, {:.3}]", 
                            cam_to_srgb[6], cam_to_srgb[7], cam_to_srgb[8]);
                        
                        // Create GPU pipeline with the RAW data + color metadata
                        let params = self.current_edit_params;
                        let wb = raw_data.wb_multipliers;
                        let image_id = self.selected_image_id.unwrap_or(0);  // Phase 20: Track which image
                        
                        Task::perform(
                            async move {
                                gpu::RenderPipeline::new(
                                    image_id,         // Phase 20: Track which image this pipeline is for
                                    raw_data.data,
                                    raw_data.width,
                                    raw_data.height,
                                    &params,
                                    wb,           // Phase 14: White balance from camera
                                    cam_to_srgb,  // Phase 15: Camera-to-sRGB color matrix
                                ).await
                            },
                            |result| Message::GpuPipelineReady(result.map(Arc::new)),
                        )
                    }
                    Err(err) => {
                        eprintln!("⚠️  Failed to load RAW data: {}", err);
                        self.editor_status = EditorStatus::Failed(
                            self.selected_image_id.unwrap_or(0),
                            err,
                        );
                        Task::none()
                    }
                }
            }
            
            Message::GpuPipelineReady(result) => {
                match result {
                    Ok(pipeline) => {
                        println!("🎨 GPU pipeline initialized!");
                        
                        // Phase 25: Clear canvas cache since this is a new pipeline for a new image
                        self.canvas_cache.clear();
                        
                        // Store pipeline in EditorStatus::Ready
                        self.editor_status = EditorStatus::Ready(pipeline);
                        
                        Task::none()
                    }
                    Err(err) => {
                        eprintln!("⚠️  Failed to initialize GPU pipeline: {}", err);
                        self.editor_status = EditorStatus::Failed(
                            self.selected_image_id.unwrap_or(0),
                            err,
                        );
                        Task::none()
                    }
                }
            }
            
            Message::ExportImage => {
                // Phase 19: Export full-resolution image
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    // Show file save dialog
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("JPEG Image", &["jpg", "jpeg"])
                        .add_filter("PNG Image", &["png"])
                        .set_file_name("export.jpg")
                        .save_file()
                    {
                        println!("📤 Exporting to: {:?}", path);
                        let pipeline_clone = Arc::clone(pipeline);
                        
                        // Run export in background to avoid freezing UI
                        return Task::perform(
                            export_image_async(pipeline_clone, path),
                            Message::ExportComplete
                        );
                    }
                }
                Task::none()
            }
            
            Message::ExportComplete(result) => {
                match result {
                    Ok(path) => {
                        println!("✅ Export complete: {:?}", path);
                        // TODO: Show status message to user
                    }
                    Err(err) => {
                        eprintln!("❌ Export failed: {}", err);
                        // TODO: Show error message to user
                    }
                }
                Task::none()
            }
            
            Message::HistogramToggled(enabled) => {
                self.histogram_enabled = enabled;
                println!("📊 Histogram {}", if enabled { "enabled" } else { "disabled" });
                
                // Phase 25: If enabling, clear canvas cache to force recalculation
                if enabled {
                    self.canvas_cache.clear();
                }
                
                Task::none()
            }
        }
    }
    
    /// Helper to save current edit parameters to database
    fn save_current_edits(&self) {
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
    
    /// Phase 24: Keyboard shortcuts subscription
    fn subscription(&self) -> iced::Subscription<Message> {
        use iced::keyboard;
        use iced::keyboard::key::Named;
        
        iced::event::listen_with(|event, _status, _window| {
            if let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) = event {
                match key.as_ref() {
                    keyboard::Key::Named(Named::Space) => Some(Message::ToggleBeforeAfter),
                    keyboard::Key::Character("r") | keyboard::Key::Character("R") => Some(Message::ResetEdits),
                    keyboard::Key::Named(Named::ArrowRight) => Some(Message::SelectNextImage),
                    keyboard::Key::Named(Named::ArrowLeft) => Some(Message::SelectPreviousImage),
                    _ => None,
                }
            } else {
                None
            }
        })
    }

    /// Build the user interface
    fn view(&self) -> Element<Message> {
        // Phase 23: Show splash screen if database is still loading
        match &self.library {
            None => self.view_splash(),
            Some(_) => self.view_main(),
        }
    }
    
    /// Phase 23: Splash screen shown during database loading
    fn view_splash(&self) -> Element<Message> {
        use iced::widget::Space;
        
        // Left half: Branding/image
        // To add your custom splash image:
        // 1. Create an "assets" folder in your project root
        // 2. Add your image: assets/splash.png (PNG with transparency recommended)
        // 3. Uncomment the image widget below and comment out the emoji
        //
        // For transparency/blending:
        // - Use PNG format with alpha channel
        // - The image will blend naturally with the dark background (#141418)
        // - For edge blending, add a gradient alpha in your image editor
        
        let left_content = column![
            Space::with_height(Length::Fill),
            // Option 1: Use emoji placeholder (current)
            // text("📸").size(120).center(),
            
            // Option 2: Use your custom image (fills container, maintains aspect ratio):
            // iced::widget::image("assets/splash.jpg")
            //     .width(Length::Fill)
            //     .height(Length::Fill)
            //     .content_fit(iced::ContentFit::Contain),  // Maintains aspect ratio
            // 
            // OR for full bleed (image fills entire space, may crop):
            iced::widget::image("assets/splash.png")
                .width(Length::Fill)
                // .height(Length::Fill)
                .content_fit(iced::ContentFit::Cover),  // Fills space, crops if needed
            
            Space::with_height(Length::Fill),
        ]
        .align_x(iced::Alignment::Center);
        
        let left_panel = container(left_content)
        .width(Length::FillPortion(7))  // 70% of width (7/10)
        .height(Length::Fill)
        .style(|_theme| {
            container::Style {
                background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.10))), // Darker, more Adobe-like
                ..Default::default()
            }
        });
        
        // Right half: Loading message
        let right_panel = container(
            column![
                Space::with_height(Length::Fill),
                text("RAW Editor")
                    .size(56)
                    .center()
                    .style(|_theme| text::Style {
                        color: Some(Color::from_rgb(0.9, 0.9, 0.9)),
                    }),
                Space::with_height(10.0),
                text("Professional RAW Photo Editor")
                    .size(14)
                    .center()
                    .style(|_theme| text::Style {
                        color: Some(Color::from_rgb(0.6, 0.6, 0.6)),
                    }),
                Space::with_height(40.0),
                text(&self.status)
                    .size(16)
                    .center()
                    .style(|_theme| text::Style {
                        color: Some(Color::from_rgb(0.8, 0.8, 0.8)),
                    }),
                Space::with_height(15.0),
                text("⏳")
                    .size(32)
                    .center()
                    .style(|_theme| text::Style {
                        color: Some(Color::from_rgb(0.5, 0.7, 1.0)),
                    }),
                Space::with_height(Length::Fill),
                text("Version 0.1.5")
                    .size(11)
                    .center()
                    .style(|_theme| text::Style {
                        color: Some(Color::from_rgb(0.4, 0.4, 0.4)),
                    }),
                Space::with_height(10.0),
            ]
            .align_x(iced::Alignment::Center)
        )
        .width(Length::FillPortion(3))  // 30% of width (3/10)
        .height(Length::Fill)
        .style(|_theme| {
            container::Style {
                background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.10))), // Match left panel for seamless look
                ..Default::default()
            }
        });
        
        // Full-screen splash layout
        row![
            left_panel,
            right_panel,
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
    
    /// Phase 23: Main application UI (shown after database loads)
    fn view_main(&self) -> Element<Message> {
        // Tab navigation bar
        let library_button = button(
            text("📚 Library")
                .size(16)
        )
        .on_press(Message::TabChanged(AppTab::Library))
        .padding(12);
        
        let library_button = if self.current_tab == AppTab::Library {
            library_button.style(button::primary)
        } else {
            library_button.style(button::secondary)
        };
        
        let develop_button = button(
            text("🎨 Develop")
                .size(16)
        )
        .on_press(Message::TabChanged(AppTab::Develop))
        .padding(12);
        
        let develop_button = if self.current_tab == AppTab::Develop {
            develop_button.style(button::primary)
        } else {
            develop_button.style(button::secondary)
        };
        
        let tab_bar = row![
            library_button,
            develop_button,
        ]
        .spacing(8)
        .padding(10);
        
        // Render content based on current tab
        let content = match self.current_tab {
            AppTab::Library => self.view_library(),
            AppTab::Develop => self.view_develop(),
        };
        
        // Main layout: tab bar + content
        column![
            tab_bar,
            content,
        ]
        .into()
    }
    
    /// Build the Library tab view (grid of thumbnails)
    fn view_library(&self) -> Element<Message> {
        // Count thumbnails and deleted files
        let cached_count = self.images.iter()
            .filter(|img| img.thumbnail_path.is_some())
            .count();
        let deleted_count = self.images.iter()
            .filter(|img| img.file_status == "deleted")
            .count();
        let total_count = self.images.len();
        
        // ========== LEFT PANE: Thumbnail Grid ==========
        
        // Header for grid pane
        let grid_header = column![
            text("RAW Editor v0.1.5 - Zoom and panning")
                .size(24),
            button("Import Folder")
                .on_press(Message::ImportFolder)
                .padding(8),
            text(&self.status).size(12),
            text(format!("Thumbnails: {}/{}  |  Deleted: {}", cached_count, total_count, deleted_count))
                .size(11),
        ]
        .spacing(10)
        .padding(10);
        
        // Create wrapping grid of clickable thumbnails
        const THUMB_SIZE: u16 = 1; // Equal size for all squares
        
        let thumbnail_grid = self.images.iter().fold(
            Wrap::new().spacing(8.0).line_spacing(8.0),
            |wrap, img| {
                // Check if file is deleted
                let is_deleted = img.file_status == "deleted";
                
                // Create thumbnail content
                let thumbnail_content = if is_deleted {
                    // Show deleted file indicator with grey background
                    container(
                        column![
                            text("❌").size(24),
                            text(&img.filename).size(8),
                            text("(deleted)").size(7),
                        ]
                        .align_x(Alignment::Center)
                        .spacing(4)
                    )
                    .width(THUMB_SIZE)
                    .height(THUMB_SIZE)
                    .center_x(iced::Length::Fixed(200.0))
                    .center_y(iced::Length::Fixed(150.0))
                    .style(|_theme| {
                        container::Style {
                            background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))),
                            border: Border {
                                color: Color::from_rgb(0.5, 0.2, 0.2),
                                width: 2.0,
                                radius: 4.0.into(),
                            },
                            ..Default::default()
                        }
                    })
                } else if let Some(ref thumb_path) = img.thumbnail_path {
                    // Show thumbnail image with grey background, fit to square
                    let handle = Handle::from_path(thumb_path.clone());
                    container(
                        Image::new(handle)
                            .content_fit(iced::ContentFit::Contain) // Fit image inside square
                    )
                    .width(THUMB_SIZE)
                    .height(THUMB_SIZE)
                    .center_x(iced::Length::Fixed(200.0))
                    .center_y(iced::Length::Fixed(150.0))
                    .style(|_theme| {
                        container::Style {
                            background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.25))),
                            border: Border {
                                color: Color::from_rgb(0.4, 0.4, 0.4),
                                width: 1.0,
                                radius: 4.0.into(),
                            },
                            ..Default::default()
                        }
                    })
                } else {
                    // Show placeholder for pending thumbnails with grey background
                    container(
                        text("⏳").size(48)
                    )
                    .width(THUMB_SIZE)
                    .height(THUMB_SIZE)
                    .center_x(iced::Length::Fixed(200.0))
                    .center_y(iced::Length::Fixed(150.0))
                    .style(|_theme| {
                        container::Style {
                            background: Some(Background::Color(Color::from_rgb(0.2, 0.2, 0.2))),
                            border: Border {
                                color: Color::from_rgb(0.3, 0.3, 0.3),
                                width: 1.0,
                                radius: 4.0.into(),
                            },
                            ..Default::default()
                        }
                    })
                };
                
                // Wrap in clickable button
                let thumbnail_widget = button(thumbnail_content)
                    .on_press(Message::ImageSelected(img.id))
                    .padding(0)
                    .style(|theme, status| {
                        button::Style {
                            background: None,
                            border: Border::default(),
                            ..button::primary(theme, status)
                        }
                    });
                
                wrap.push(thumbnail_widget)
            },
        );
        
        // Phase 20: Full-screen thumbnail grid (no preview pane)
        // Wrap grid in scrollable container
        let content = column![
            grid_header,
            scrollable(thumbnail_grid)
                .height(Length::Fill)
                .width(Length::Fill),
        ];
        
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
    
    /// Build the Develop tab view (full-screen editor with preview)
    fn view_develop(&self) -> Element<Message> {
        match &self.editor_status {
            EditorStatus::NoSelection => {
                // No image selected - show prompt
                container(
                    column![
                        text("No Image Selected").size(32),
                        text("").size(20),
                        text("← Switch to Library tab to select an image")
                            .size(18)
                            .style(|theme: &Theme| {
                                text::Style {
                                    color: Some(theme.palette().text.scale_alpha(0.6)),
                                }
                            }),
                    ]
                    .padding(40)
                    .align_x(Alignment::Center)
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
            }
            EditorStatus::Loading(image_id) => {
                // Show loading state
                if let Some(img) = self.images.iter().find(|i| i.id == *image_id) {
                    container(
                        column![
                            text(&img.filename).size(24),
                            text("").size(30),
                            text("⌛ Generating full preview...").size(20),
                            text("").size(10),
                            text("This may take a few seconds for large RAW files")
                                .size(14)
                                .style(|theme: &Theme| {
                                    text::Style {
                                        color: Some(theme.palette().text.scale_alpha(0.7)),
                                    }
                                }),
                        ]
                        .padding(40)
                        .align_x(Alignment::Center)
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
                } else {
                    container(text("Loading...").size(24))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .into()
                }
            }
            EditorStatus::Ready(pipeline) => {
                // GPU pipeline ready - show live canvas rendering!
                if let Some(image_id) = self.selected_image_id {
                    if let Some(img) = self.images.iter().find(|i| i.id == image_id) {
                        // Header with image info
                        let header = row![
                            text(&img.filename).size(18),
                            text(" • ").size(18),
                            text("🎨 GPU Rendering + Debayering").size(18),
                        ]
                        .spacing(5)
                        .padding(10);
                        
                        // 🎨 Phase 25: GPU-Accelerated Zoom & Pan (with smart caching)
                        // Determine which params to render based on show_before toggle
                        let params_to_render = if self.show_before {
                            state::edit::EditParams::default() // Show original (no edits)
                        } else {
                            self.current_edit_params.clone() // Show edited version
                        };
                        
                        // Phase 25: Update GPU uniforms with correct params + zoom/pan
                        // This updates the shader uniforms (very fast, no readback)
                        pipeline.update_uniforms_with_zoom(&params_to_render, self.zoom, self.pan_offset.x, self.pan_offset.y);
                        
                        // Phase 25: Render with zoom/pan applied in shader
                        println!("🎨 GPU rendering {}x{} preview (zoom: {:.1}%, pan: {:.3}, {:.3})", 
                            pipeline.preview_width, 
                            pipeline.preview_height,
                            self.zoom * 100.0,
                            self.pan_offset.x,
                            self.pan_offset.y
                        );
                        let rgba_bytes = pipeline.render_to_bytes();
                        println!("✅ Rendered {} bytes (preview with zoom/pan)", rgba_bytes.len());
                        
                        // Phase 22: Calculate histogram from TINY 256px render (only if enabled)
                        if self.histogram_enabled {
                            let histogram_bytes = pipeline.render_to_histogram_bytes();
                            let histogram = pipeline.calculate_histogram(&histogram_bytes);
                            *self.histogram_data.borrow_mut() = histogram;
                            self.histogram_cache.clear(); // Force histogram redraw
                        }
                        
                        // Create Image handle from rendered bytes
                        let image_handle = iced::widget::image::Handle::from_rgba(
                            pipeline.preview_width,
                            pipeline.preview_height,
                            rgba_bytes
                        );
                        
                        // Phase 25: Image widget with zoom/pan already applied in GPU shader!
                        let gpu_image = iced::widget::Image::new(image_handle)
                            .content_fit(iced::ContentFit::Contain);
                        
                        // Phase 25: Wrap in mouse_area to capture zoom/pan events
                        use iced::widget::mouse_area;
                        use iced::mouse::{self, ScrollDelta};
                        
                        let interactive_image = mouse_area(gpu_image)
                            .on_scroll(|delta| {
                                let zoom_delta = match delta {
                                    ScrollDelta::Lines { y, .. } => y * 0.1,
                                    ScrollDelta::Pixels { y, .. } => y * 0.01,
                                };
                                // Phase 26: Pass sentinel value (-1, -1) for cursor
                                // Actual position will be retrieved from last_cursor_position in handler
                                Message::Zoom(zoom_delta, Point::new(-1.0, -1.0))
                            })
                            .on_press(Message::MousePressed)
                            .on_release(Message::MouseReleased)
                            .on_move(|position| Message::MouseMoved(position));
                        
                        let preview = container(interactive_image)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .center_x(Length::Fill)
                            .center_y(Length::Fill)
                            .style(|_theme| {
                                container::Style {
                                    background: Some(Background::Color(Color::from_rgb(0.0, 0.0, 0.0))),
                                    ..Default::default()
                                }
                            });
                    
                    // Right sidebar with editing controls
                    // Phase 21: Histogram toggle
                    let histogram_toggle = iced::widget::checkbox(
                        "Show Histogram",
                        self.histogram_enabled
                    )
                    .on_toggle(Message::HistogramToggled);
                    
                    // Build histogram widget only if enabled
                    let histogram_section = if self.histogram_enabled {
                        let histogram_widget = iced::widget::canvas::Canvas::new(
                            crate::ui::histogram::Histogram {
                                data: self.histogram_data.borrow().clone(),
                            }
                        )
                        .width(iced::Length::Fill)
                        .height(iced::Length::Fixed(120.0));
                        
                        Some(container(histogram_widget)
                            .padding(5)
                            .style(|_theme| {
                                iced::widget::container::Style {
                                    background: Some(iced::Background::Color(iced::Color::from_rgb(0.1, 0.1, 0.1))),
                                    border: iced::Border {
                                        color: iced::Color::from_rgb(0.3, 0.3, 0.3),
                                        width: 1.0,
                                        radius: 4.0.into(),
                                    },
                                    ..Default::default()
                                }
                            }))
                    } else {
                        None
                    };
                    
                    let mut sidebar = column![
                        text("Edit Controls").size(16),
                        histogram_toggle,
                    ];
                    
                    if let Some(hist) = histogram_section {
                        sidebar = sidebar.push(hist);
                    }
                    
                    let sidebar = sidebar
                        // Exposure
                        .push(text(format!("Exposure: {:.2}", self.current_edit_params.exposure)))
                        .push(slider(-5.0..=5.0, self.current_edit_params.exposure, Message::ExposureChanged)
                            .step(0.1))
                        // Highlights
                        .push(text(format!("Highlights: {:.0}", self.current_edit_params.highlights * 100.0)))
                        .push(slider(-1.0..=1.0, self.current_edit_params.highlights, Message::HighlightsChanged)
                            .step(0.01))
                        // Shadows
                        .push(text(format!("Shadows: {:.0}", self.current_edit_params.shadows * 100.0)))
                        .push(slider(-1.0..=1.0, self.current_edit_params.shadows, Message::ShadowsChanged)
                            .step(0.01))
                        // Contrast
                        .push(text(format!("Contrast: {:.2}", self.current_edit_params.contrast)))
                        .push(slider(-10.0..=10.0, self.current_edit_params.contrast, Message::ContrastChanged)
                            .step(0.005))
                        // Vibrance (Phase 27: Smart saturation protecting skin tones)
                        .push(text(format!("Vibrance: {:.0}", self.current_edit_params.vibrance * 100.0)))
                        .push(slider(-1.0..=1.0, self.current_edit_params.vibrance, Message::VibranceChanged)
                            .step(0.01))
                        // Saturation
                        .push(text(format!("Saturation: {:.0}", self.current_edit_params.saturation)))
                        .push(slider(-100.0..=100.0, self.current_edit_params.saturation, Message::SaturationChanged))
                        // Temperature
                        .push(text(format!("Temperature: {:.0}", self.current_edit_params.temperature * 100.0)))
                        .push(slider(-1.0..=1.0, self.current_edit_params.temperature, Message::TemperatureChanged)
                            .step(0.01))
                        // Tint
                        .push(text(format!("Tint: {:.0}", self.current_edit_params.tint * 100.0)))
                        .push(slider(-1.0..=1.0, self.current_edit_params.tint, Message::TintChanged)
                            .step(0.01))
                        // Whites
                        .push(text(format!("Whites: {:.2}", self.current_edit_params.whites)))
                        .push(slider(0.8..=1.2, self.current_edit_params.whites, Message::WhitesChanged)
                            .step(0.01))
                        // Blacks
                        .push(text(format!("Blacks: {:.3}", self.current_edit_params.blacks)))
                        .push(slider(0.0..=0.2, self.current_edit_params.blacks, Message::BlacksChanged)
                            .step(0.005))
                        .push(button("Reset All").on_press(Message::ResetEdits))
                        .push(button("Export").on_press(Message::ExportImage))
                    .spacing(10)
                    .padding(15)

                    .width(Length::Fixed(200.0))
                    .height(Length::Fill);
                    
                    // Main layout: header + (preview + sidebar)
                    column![
                        header,
                        row![
                            preview,
                            sidebar,
                        ]
                        .spacing(0)
                        .height(Length::Fill),
                    ]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
                    } else {
                        container(text("Image not found").size(24))
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .center_x(Length::Fill)
                            .center_y(Length::Fill)
                            .into()
                    }
                } else {
                    container(text("No image selected").size(24))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .into()
                }
            }
            EditorStatus::Failed(image_id, error) => {
                // Show error state
                if let Some(img) = self.images.iter().find(|i| i.id == *image_id) {
                    container(
                        column![
                            text("❌ Preview Failed").size(24),
                            text("").size(20),
                            text(&img.filename).size(18),
                            text("").size(15),
                            text(error)
                                .size(14)
                                .style(|theme: &Theme| {
                                    text::Style {
                                        color: Some(theme.palette().danger),
                                    }
                                }),
                        ]
                        .padding(40)
                        .align_x(Alignment::Center)
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
                } else {
                    container(text("Error loading preview").size(24))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .into()
                }
            }
        }
    }

    /// Set the application theme
    fn theme(&self) -> Theme {
        Theme::Dark
    }
}

/// Phase 19: Async export function that renders full resolution and saves to disk
/// This runs in a background thread to avoid freezing the UI
async fn export_image_async(
    pipeline: Arc<gpu::RenderPipeline>,
    save_path: std::path::PathBuf,
) -> Result<std::path::PathBuf, String> {
    // Run the heavy rendering work in a blocking task
    tokio::task::spawn_blocking(move || {
        println!("🖼️  Starting full-resolution export...");
        
        // Render at FULL resolution (24MP for 6016x4016 image)
        // This will take 1-2 seconds - that's why we're async!
        let rgba_bytes = pipeline.render_full_res_to_bytes();
        println!("✅ Rendered {} bytes at full resolution", rgba_bytes.len());
        
        // Determine format from file extension
        let extension = save_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg")
            .to_lowercase();
        
        // Save using image crate
        let result = match extension.as_str() {
            "png" => {
                image::save_buffer(
                    &save_path,
                    &rgba_bytes,
                    pipeline.width,
                    pipeline.height,
                    image::ColorType::Rgba8,
                )
            }
            _ => {
                // Default to JPEG
                // Convert RGBA to RGB (JPEG doesn't support alpha)
                let rgb_bytes: Vec<u8> = rgba_bytes
                    .chunks_exact(4)
                    .flat_map(|rgba| [rgba[0], rgba[1], rgba[2]])
                    .collect();
                
                image::save_buffer(
                    &save_path,
                    &rgb_bytes,
                    pipeline.width,
                    pipeline.height,
                    image::ColorType::Rgb8,
                )
            }
        };
        
        result
            .map(|_| save_path.clone())
            .map_err(|e| format!("Failed to save image: {}", e))
    })
    .await
    .map_err(|e| format!("Export task failed: {}", e))?
}

/// Phase 23: Application entry point
/// 
/// To customize the splash screen window (Adobe-style borderless window):
/// 1. Use iced::window::Settings to set decorations: false
/// 2. Set a fixed size (e.g., 800x600) for splash
/// 3. Center the window
/// Example:
/// ```
/// .window(iced::window::Settings {
///     size: iced::Size::new(900.0, 600.0),
///     decorations: false,  // Remove title bar during splash
///     ..Default::default()
/// })
/// ```
/// Note: You'll need to manually add decorations back after loading,
/// or keep the app borderless throughout (like some Adobe products)
fn main() -> iced::Result {
    iced::application(
        "RAW Editor",
        RawEditor::update,
        RawEditor::view,
    )
    .theme(RawEditor::theme)
    .subscription(RawEditor::subscription) // Phase 24: Enable keyboard shortcuts
    // Phase 23: Window settings - start with normal window (has title bar)
    // Note: iced::application() uses a single window throughout
    // To have a separate splash window, you'd need the multi-window API
    .window(iced::window::Settings {
        size: iced::Size::new(900.0, 400.0),  // Main app size
        min_size: Some(iced::Size::new(600.0, 400.0)),
        decorations: true,  // Keep title bar for usability
        ..Default::default()
    })
    .centered()
    .run_with(RawEditor::new)
}

/// Async function to import all RAW files from a folder
/// Runs in a background thread to avoid blocking the UI
async fn import_folder_async(folder_path: PathBuf, db_path: PathBuf) -> ImportResult {
    let mut imported_count = 0;
    let mut skipped_count = 0;
    
    // Open a new database connection for this background thread
    // rusqlite::Connection is not Send, so we can't share the main connection
    let conn = Connection::open(&db_path)
        .expect("Failed to open database connection for import");
    
    println!("🔍 Scanning folder: {}", folder_path.display());
    
    // Supported RAW file extensions (common formats)
    let raw_extensions = [
        "nef", "dng", "cr2", "cr3", "arw", "raf", "orf", "rw2", 
        "pef", "srw", "erf", "kdc", "dcr", "mos", "raw", "rwl",
    ];
    
    // Walk the directory tree recursively
    for entry in WalkDir::new(&folder_path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        
        // Only process files (not directories)
        if !path.is_file() {
            continue;
        }
        
        // Check if this is a RAW file by extension
        if let Some(extension) = path.extension() {
            let ext = extension.to_string_lossy().to_lowercase();
            if !raw_extensions.contains(&ext.as_str()) {
                continue;
            }
        } else {
            continue;
        }
        
        // Extract path and filename
        let path_str = path.to_string_lossy().to_string();
        let filename = path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        
        // Try to insert into database
        let result = conn.execute(
            "INSERT INTO images (path, filename, imported_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                &path_str,
                &filename,
                Utc::now().timestamp(),
            ],
        );
        
        match result {
            Ok(_) => {
                imported_count += 1;
                if imported_count % 100 == 0 {
                    println!("⏳ Imported {} files...", imported_count);
                }
            }
            Err(rusqlite::Error::SqliteFailure(err, _)) => {
                // Check if this is a UNIQUE constraint violation (duplicate)
                if err.code == ErrorCode::ConstraintViolation {
                    skipped_count += 1;
                } else {
                    eprintln!("⚠️  Error importing {}: {:?}", filename, err);
                }
            }
            Err(e) => {
                eprintln!("⚠️  Error importing {}: {:?}", filename, e);
            }
        }
    }
    
    println!("✅ Import complete: {} new, {} skipped", imported_count, skipped_count);
    
    ImportResult {
        imported_count,
        skipped_count,
    }
}

/// Async function to generate thumbnails using two-tier queue system:
/// - HIGH PRIORITY: Process 'pending' images with fast methods (tiers 1-3)
/// - LOW PRIORITY: Process 'needs_slow' images with slow method (tier 4) AFTER fast queue is empty
async fn generate_thumbnails_async(db_path: PathBuf) -> ThumbnailResult {
    let mut generated_count = 0;
    
    // Open database connection
    let conn = Connection::open(&db_path)
        .expect("Failed to open database connection for thumbnail generation");
    
    // ========================================
    // PHASE 1: HIGH PRIORITY - Fast Queue
    // Process 'pending' images with fast methods (tiers 1-3)
    // ========================================
    let fast_batch_size = 5; // Process 5 at a time for efficiency
    
    let mut stmt = conn.prepare(
        "SELECT id, path FROM images 
         WHERE cache_status = 'pending' 
         ORDER BY id 
         LIMIT ?"
    ).expect("Failed to prepare statement for fast queue");
    
    let pending_images: Vec<(i64, String)> = stmt
        .query_map([fast_batch_size], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("Failed to query pending images")
        .filter_map(|r| r.ok())
        .collect();
    
    for (image_id, raw_path_str) in pending_images {
        let raw_path = std::path::Path::new(&raw_path_str);
        
        // Try FAST methods only (tiers 1-3)
        if let Some(thumbnail_path) = raw::thumbnail::generate_thumbnail_fast(raw_path, image_id) {
            // Success! Update database
            let thumbnail_path_str = thumbnail_path.to_string_lossy().to_string();
            let _ = conn.execute(
                "UPDATE images SET thumbnail_path = ?1, cache_status = 'cached' WHERE id = ?2",
                rusqlite::params![thumbnail_path_str, image_id],
            );
            generated_count += 1;
        } else {
            // Fast methods failed - add to low-priority slow queue
            let _ = conn.execute(
                "UPDATE images SET cache_status = 'needs_slow' WHERE id = ?1",
                rusqlite::params![image_id],
            );
        }
    }
    
    // ========================================
    // PHASE 2: LOW PRIORITY - Slow Queue
    // Only process if fast queue is empty (no more 'pending' images)
    // ========================================
    let pending_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM images WHERE cache_status = 'pending'",
        [],
        |row| row.get(0)
    ).unwrap_or(0);
    
    if pending_count == 0 {
        // Fast queue is empty - process slow queue
        let slow_batch_size = 1; // Process 1 at a time (slow operations)
        
        let mut stmt = conn.prepare(
            "SELECT id, path FROM images 
             WHERE cache_status = 'needs_slow' 
             ORDER BY id 
             LIMIT ?"
        ).expect("Failed to prepare statement for slow queue");
        
        let slow_images: Vec<(i64, String)> = stmt
            .query_map([slow_batch_size], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("Failed to query slow images")
            .filter_map(|r| r.ok())
            .collect();
        
        for (image_id, raw_path_str) in slow_images {
            let raw_path = std::path::Path::new(&raw_path_str);
            
            // Try SLOW method (tier 4)
            if let Some(thumbnail_path) = raw::thumbnail::generate_thumbnail_slow(raw_path, image_id) {
                // Success! Update database
                let thumbnail_path_str = thumbnail_path.to_string_lossy().to_string();
                let _ = conn.execute(
                    "UPDATE images SET thumbnail_path = ?1, cache_status = 'cached' WHERE id = ?2",
                    rusqlite::params![thumbnail_path_str, image_id],
                );
                generated_count += 1;
            } else {
                // All methods failed - mark as failed
                let _ = conn.execute(
                    "UPDATE images SET cache_status = 'failed' WHERE id = ?1",
                    rusqlite::params![image_id],
                );
            }
        }
    }
    
    ThumbnailResult {
        generated_count,
    }
}
