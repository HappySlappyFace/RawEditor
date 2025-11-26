use iced::{Background, Border, Color, Element, Task, Theme, Point, Font};
use iced::font::Weight;
use iced::widget::{button, column, container, row, scrollable, text, Image, slider, canvas, checkbox, Container, stack};
use iced::{Alignment, Length};
use iced::widget::image::Handle;
use iced_aw::Wrap;
use iced::window;
use rfd::FileDialog;
use rusqlite::{Connection, ErrorCode};
use std::path::PathBuf;
use std::sync::Arc;
use std::collections::{HashMap, HashSet};  // Phase 55: Multi-selection
use walkdir::WalkDir;
use chrono::Utc;
// use crate::canvas;

// Declare the state, raw, gpu, and ui modules
mod debug;
mod gpu;
mod raw;
mod state;
mod ui;
mod color;  // Phase 15: Color space conversion utilities

// Import shared data structures (alias to avoid conflict with iced's image widget)
use state::data::Image as ImageData;

// Phase 15: Color space conversion


// Phase 57: Embedded font for icons and typography
const ICON_FONT: iced::Font = iced::Font::with_name("JetBrainsMono Nerd Font");
const ICON_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/icons.ttf");

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
    /// Phase 29: Instant preview handle (displayed while RAW loads)
    working_preview: Option<Handle>,
    /// Phase 41: Current image metadata for inspection
    current_metadata: Option<raw::loader::RawDataResult>,
    /// Phase 54: Edit settings clipboard for copy/paste
    edit_clipboard: Option<state::edit::EditParams>,
    /// Phase 55: Multi-selection set (image IDs)
    multi_selection: HashSet<i64>,
    /// Phase 55: Track modifier keys for Ctrl/Cmd+Click
    last_modifiers: iced::keyboard::Modifiers,
    /// Phase 59: Minimum rating filter (0 = show all, 1-5 = show rating or higher)
    min_filter_rating: u8,
    /// Phase 60: Toggle for HUD overlay (ISO, Shutter, etc.)
    show_info_hud: bool,
    /// Phase 65: Undo/Redo History Map<ImageID, (HistoryStack, CurrentIndex)>
    history_map: HashMap<i64, (Vec<state::edit::EditParams>, usize)>,
    /// Phase 67: Interactive crop mode
    is_cropping: bool,
    /// Phase 67: Drag mode for interaction
    drag_mode: DragMode,
}

/// Phase 67: Drag mode for mouse interaction
#[derive(Debug, Clone, Copy, PartialEq)]
enum DragMode {
    None,
    Pan,
    CropHandle(CropHandle),
}

/// Phase 67: Crop handles
#[derive(Debug, Clone, Copy, PartialEq)]
enum CropHandle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Helper for creating a professional slider row
fn slider_row<'a, F>(
    label: &'a str, 
    value: f32, 
    range: std::ops::RangeInclusive<f32>, 
    step: f32, 
    on_change: F
) -> Element<'a, Message>
where
    F: Fn(f32) -> Message + 'a,
{
    row![
        text(label)
            .width(Length::Fixed(90.0))
            .size(13)
            .style(|_theme| text::Style { color: Some(Color::from_rgb(0.7, 0.7, 0.7)) }),
        slider(range, value, on_change)
            .step(step)
            .width(Length::Fill)
            .style(crate::ui::styles::ProSlider::style)
            .on_release(Message::CommitEdit), // Phase 65: Commit edit on release
        text(format!("{:.2}", value))
            .width(Length::Fixed(40.0))
            .size(13)
            .align_x(iced::alignment::Horizontal::Right)
            .style(|_theme| text::Style { color: Some(Color::from_rgb(0.7, 0.7, 0.7)) }),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center)
    .into()
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
    /// Phase 28: Multi-tier cache processing completed
    /// Result is (image_id, thumb_path, instant_path, working_path) or (image_id, error)
    CacheProcessed(Result<(i64, String, String, String), (i64, String)>),
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
    /// User changed manual black level offset (channel_index, value)
    BlackOffsetChanged(usize, f32),
    /// User changed black level phase (is_y, value)
    BlackPhaseChanged(bool, u32),
    
    // ========== Color Messages ==========
    /// User changed vibrance slider
    VibranceChanged(f32),
    /// User changed saturation slider
    SaturationChanged(f32),
    /// User changed temperature slider (Phase 18)
    TemperatureChanged(f32),
    /// User changed tint slider (Phase 18)
    TintChanged(f32),
    /// User changed noise reduction slider (Phase 49)
    NoiseReductionChanged(f32),
    /// User changed sharpening slider (Phase 50)
    SharpeningChanged(f32),
    /// User changed sharpen masking slider (Phase 51)
    SharpenMaskingChanged(f32),
    /// User changed rotation slider (Phase 52)
    RotationChanged(f32),
    /// User changed crop rectangle (Phase 66)
    SetCrop([f32; 4]),
    /// User clicked Reset button to clear all edits
    ResetEdits,
    // Phase 67: Interactive Crop
    ToggleCropMode,
    // Phase 63: Copy/Paste Edits
    CopyEdits,
    PasteEdits,
    
    // Phase 65: Undo/Redo
    Undo,
    Redo,
    CommitEdit,
    
    // ========== Phase 54: Settings Clipboard ==========
    /// Copy current edit settings to clipboard (Ctrl/Cmd+C)
    CopySettings,
    /// Paste edit settings from clipboard (Ctrl/Cmd+V)
    PasteSettings,
    
    // ========== Phase 55: Multi-Selection ==========
    /// Modifier keys changed (for Ctrl/Cmd+Click detection)
    ModifiersChanged(iced::keyboard::Modifiers),
    
    // ========== Phase 56: Ratings & Culling ==========
    /// Set rating for selected image(s) (0-5 stars)
    SetRating(u8),
    
    // ========== Phase 59: Rating Filter ==========
    /// Set minimum rating filter (0 = all, 1-5 = show rating or higher)
    SetMinRating(u8),

    // ========== Phase 60: Modern Layout & HUD ==========
    /// Toggle HUD overlay (ISO, Shutter, etc.)
    ToggleInfoHud,


    
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
    ExportComplete(Result<PathBuf, String>),
    
    // Window Controls
    MinimizeWindow,
    MaximizeWindow,
    CloseWindow,
    DragWindow,
    
    // ========== Histogram Messages (Phase 22) ==========
    /// User toggled histogram on/off
    HistogramToggled(bool),
    
    // ========== Phase 30: Multi-Tier Preview Loading ==========
    /// Background loading of 1280px working preview completed
    WorkingPreviewReady(iced::widget::image::Handle),
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
    fn title(&self) -> String {
        String::from("RAW Editor")
    }

    /// Phase 23: Create a new instance of the application (INSTANT!)
    /// The database now loads in the background to show splash screen immediately
    fn new() -> (Self, Task<Message>) {
        println!("🚀 RAW Editor starting (instant splash screen)...");
        
        // Initialize preview cache directory (fast)
        let preview_cache_dir = raw::preview::get_preview_cache_dir();
        
        // Determine the database path (e.g., in the application's data directory)
        let db_path = state::library::Library::get_db_path();

        (
            RawEditor { 
                library: None, // Phase 23: Database loads in background
                status: "Initializing...".to_string(),
                images: Vec::new(), // Empty until database loads
                selected_image_id: None,
                preview_cache_dir,
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
                show_info_hud: false,  // Phase 60: HUD hidden by default
                history_map: HashMap::new(), // Phase 65: Undo/Redo History
                is_cropping: false, // Phase 67: Interactive Crop
                drag_mode: DragMode::None, // Phase 67: Interactive Crop
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
    fn get_current_history(&mut self) -> Option<&mut (Vec<state::edit::EditParams>, usize)> {
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
    fn commit_current_state(&mut self) {
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
    fn get_image_screen_bounds(&self, pipeline: &gpu::RenderPipeline) -> Rectangle {
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
    fn detect_crop_handle(&self, cursor_pos: Point) -> Option<CropHandle> {
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
        }
        None
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
                                
                                // Phase 28: Start multi-tier cache processing for any pending images
                                if let Some(lib) = &self.library {
                                    let db_path = lib.path().clone();
                                    return Task::batch(vec![
                                        maximize_window,
                                        Task::perform(
                                            process_cache_async(db_path),
                                            Message::CacheProcessed,
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
                    
                    // Phase 28: Start multi-tier cache processing for newly imported images
                    let db_path = library.path().clone();
                    return Task::perform(
                        process_cache_async(db_path),
                        Message::CacheProcessed,
                    );
                }
                Task::none()
            }
            Message::ThumbnailGenerated(_result) => {
                // Phase 28: DEPRECATED - Old thumbnail system completely disabled
                // Phase 28 multi-tier cache processor handles all cache generation now
                Task::none()
            }
            Message::CacheProcessed(result) => {
                // Phase 28: Multi-tier cache processing completed
                if let Some(library) = &self.library {
                    match result {
                        Ok((image_id, thumb_path, instant_path, working_path)) => {
                            // Save all 3 cache paths to database
                            if let Err(e) = library.set_image_cache_paths(
                                image_id,
                                &thumb_path,
                                &instant_path,
                                &working_path,
                            ) {
                                eprintln!("❌ Failed to save cache paths for image {}: {:?}", image_id, e);
                            } else {
                                println!("✅ Cached 3 tiers for image {}", image_id);
                                println!("   📁 Thumb: {}", thumb_path);
                                println!("   📁 Instant: {}", instant_path);
                                println!("   📁 Working: {}", working_path);
                            }
                        },
                        Err((image_id, error)) => {
                            // Only log real errors (not "No pending images")
                            if image_id != 0 {
                                eprintln!("❌ Cache processing failed for image {}: {}", image_id, error);
                                // Mark as failed in database
                                let _ = library.conn().execute(
                                    "UPDATE images SET cache_status = 'failed' WHERE id = ?1",
                                    [image_id],
                                );
                            }
                        },
                    }
                    
                    // Reload images to update UI
                    self.images = library.get_all_images().unwrap_or_default();
                    
                    // Check if there are more pending images
                    let pending_count: i64 = library.conn()
                        .query_row(
                            "SELECT COUNT(*) FROM images WHERE cache_status = 'pending'",
                            [],
                            |row| row.get(0)
                        )
                        .unwrap_or(0);
                    
                    if pending_count > 0 {
                        // Calculate progress
                        let total_count = self.images.len() as i64;
                        let cached_count = total_count - pending_count;
                        let progress_pct = if total_count > 0 {
                            (cached_count * 100) / total_count
                        } else {
                            0
                        };
                        
                        // Update status with progress
                        self.status = format!(
                            "📦 Caching: {}/{} ({}%) - {} remaining",
                            cached_count, total_count, progress_pct, pending_count
                        );
                        
                        // Trigger next cache processing job
                        let db_path = library.path().clone();
                        return Task::perform(
                            process_cache_async(db_path),
                            Message::CacheProcessed,
                        );
                    } else {
                        // All done!
                        self.status = format!("{} All cache tiers generated! ({} images)", ui::icons::CHECK, self.images.len());
                        println!("🎉 Phase 28: All images cached with 3 tiers!");
                    }
                }
                
                Task::none()
            }
            Message::ImageSelected(image_id) => {
                // Phase 55: Multi-selection logic
                if self.last_modifiers.command() {
                    // Ctrl/Cmd+Click: Toggle in multi-selection
                    if !self.multi_selection.remove(&image_id) {
                        // Wasn't in set, add it
                        self.multi_selection.insert(image_id);
                    }
                    println!("🔘 Toggled image {} in selection (total: {})", image_id, self.multi_selection.len());
                } else {
                    // Normal click: Reset to single selection
                    self.multi_selection.clear();
                    self.multi_selection.insert(image_id);
                    println!("✨ Selected image {} (single selection)", image_id);
                }
                
                // Always update selected_image_id for "hot swap"
                // Phase 20: INSTANT selection - just update state, don't load anything!
                // Loading is deferred until user switches to Develop tab
                self.selected_image_id = Some(image_id);
                
                // Phase 25: Clear canvas cache since we're switching to a different image
                self.canvas_cache.clear();
                
                // Phase 23: Load edit parameters from database (only if loaded)
                if let Some(library) = &self.library {
                    self.current_edit_params = library.load_edit_params(image_id)
                        .unwrap_or_else(|_| state::edit::EditParams::default());
                    
                    if !self.current_edit_params.is_unedited() {
                        println!("📝 Loaded existing edits for image {}", image_id);
                    }
                    
                    // Phase 65: Initialize history for this image if needed
                    self.history_map.entry(image_id).or_insert_with(|| {
                        (vec![self.current_edit_params.clone()], 0)
                    });
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
                        // Phase 30: Multi-Tier Preview Loading
                        // 1. Clear current preview
                        self.working_preview = None;
                        
                        if let Some(img) = self.images.iter().find(|i| i.id == image_id) {
                            // 2. Try to load "Instant" (384px) preview IMMEDIATELY (synchronous)
                            // This is small enough to load on main thread without blocking
                            if let Some(path) = &img.cache_path_instant {
                                println!("⚡ Loading instant preview from: {}", path);
                                self.working_preview = Some(Handle::from_path(path.clone()));
                            } else if let Some(path) = &img.cache_path_working {
                                // Fallback to working if instant missing
                                println!("⚡ Loading working preview (fallback) from: {}", path);
                                self.working_preview = Some(Handle::from_path(path.clone()));
                            }
                            
                            // Set editor status to loading
                            self.editor_status = EditorStatus::Loading(image_id);

                            let mut tasks = Vec::new();
                            
                            // 3. Spawn background task to load "Working" (1280px) preview
                            // This upgrades the quality while RAW loads
                            if let Some(path) = &img.cache_path_working {
                                let path_clone = path.clone();
                                tasks.push(Task::perform(
                                    load_image_handle(path_clone),
                                    Message::WorkingPreviewReady
                                ));
                            }
                            
                            // 4. Spawn background task to load full RAW data
                            let raw_path = img.path.clone();
                            tasks.push(Task::perform(
                                raw::loader::load_raw_data(raw_path),
                                Message::RawDataLoaded,
                            ));
                            
                            return Task::batch(tasks);
                        }
                    } else {
                        println!("⚡ Pipeline already loaded for image {}", image_id);
                    }
                }
                
                Task::none()
            }
            Message::PreviewGenerated(_result) => {
                // Phase 28: DEPRECATED - Old preview system replaced by multi-tier cache
                // This message is never sent anymore, kept for compilation compatibility
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
            Message::BlackOffsetChanged(index, value) => {
                if index < 4 {
                    self.current_edit_params.black_offsets[index] = value;
                    self.save_current_edits();
                    // Phase 25: Update GPU uniforms and invalidate canvas cache
                    if let EditorStatus::Ready(pipeline) = &self.editor_status {
                        pipeline.update_uniforms(&self.current_edit_params);
                        self.canvas_cache.clear();
                    }
                }
                Task::none()
            }
            Message::BlackPhaseChanged(is_y, value) => {
                if is_y {
                    self.current_edit_params.black_phase_y = value;
                } else {
                    self.current_edit_params.black_phase_x = value;
                }
                self.save_current_edits();
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
            Message::NoiseReductionChanged(value) => {
                self.current_edit_params.noise_reduction = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::SharpeningChanged(value) => {
                self.current_edit_params.sharpening = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::SharpenMaskingChanged(value) => {
                self.current_edit_params.sharpen_masking = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            Message::RotationChanged(value) => {
                self.current_edit_params.rotation = value;
                self.save_current_edits();
                // Phase 25: Update GPU uniforms and invalidate canvas cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                }
                Task::none()
            }
            
            // Phase 66: Crop Handler
            Message::SetCrop(crop) => {
                self.current_edit_params.crop = crop;
                self.save_current_edits();
                
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.canvas_cache.clear();
                    self.histogram_cache.clear(); // Crop changes histogram!
                }
                
                // Commit to history
                self.commit_current_state();
                
                Task::none()
            }
            
            // Phase 67: Interactive Crop
            Message::ToggleCropMode => {
                self.is_cropping = !self.is_cropping;
                println!("✂️ Crop Mode: {}", if self.is_cropping { "ON" } else { "OFF" });
                
                // Reset drag mode when toggling
                self.drag_mode = DragMode::None;
                
                // Force redraw
                self.canvas_cache.clear();
                Task::none()
            }

            // Phase 65: Undo/Redo
            Message::CommitEdit => {
                self.commit_current_state();
                Task::none()
            }
            
            Message::Undo => {
                let new_params = if let Some((stack, index)) = self.get_current_history() {
                    if *index > 0 {
                        *index -= 1;
                        println!("↩️ Undo: Index {}", *index);
                        Some(stack[*index].clone())
                    } else {
                        None
                    }
                } else {
                    None
                };
                
                if let Some(params) = new_params {
                    self.current_edit_params = params;
                    // Trigger update pipeline
                    if let EditorStatus::Ready(pipeline) = &mut self.editor_status {
                        pipeline.update_uniforms(&self.current_edit_params);
                    }
                    self.canvas_cache.clear();
                    self.histogram_cache.clear();
                    self.save_current_edits();
                    Task::none()
                } else {
                    Task::none()
                }
            }
            
            Message::Redo => {
                let new_params = if let Some((stack, index)) = self.get_current_history() {
                    if *index < stack.len() - 1 {
                        *index += 1;
                        println!("↪️ Redo: Index {}", *index);
                        Some(stack[*index].clone())
                    } else {
                        None
                    }
                } else {
                    None
                };
                
                if let Some(params) = new_params {
                    self.current_edit_params = params;
                    // Trigger update pipeline
                    if let EditorStatus::Ready(pipeline) = &mut self.editor_status {
                        pipeline.update_uniforms(&self.current_edit_params);
                    }
                    self.canvas_cache.clear();
                    self.histogram_cache.clear();
                    self.save_current_edits();
                    Task::none()
                } else {
                    Task::none()
                }
            }

            Message::CopyEdits => {
                self.edit_clipboard = Some(self.current_edit_params.clone());
                Task::none()
            }
            Message::PasteEdits => {
                if let Some(clipboard) = &self.edit_clipboard {
                    self.current_edit_params = clipboard.clone();
                    self.save_current_edits(); // Save the pasted edits
                    if let EditorStatus::Ready(pipeline) = &self.editor_status {
                        pipeline.update_uniforms(&self.current_edit_params);
                        self.canvas_cache.clear();
                        self.histogram_cache.clear();
                    }
                    // Phase 65: Commit the pasted state to history
                    self.commit_current_state();
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
                
                // Phase 65: Commit the reset state to history
                self.commit_current_state();
                
                Task::none()
            }
            
            // ========== Phase 54: Settings Clipboard Handlers ==========
            
            Message::CopySettings => {
                // Copy current edit parameters to clipboard
                self.edit_clipboard = Some(self.current_edit_params);
                self.status = "Settings copied!".to_string();
                println!("📋 Copied edit settings to clipboard");
                Task::none()
            }
            
            Message::PasteSettings => {
                // Phase 55: Paste edit parameters to ALL selected images
                if let Some(clipboard_params) = self.edit_clipboard {
                    // Overwrite current parameters (for the displayed image)
                    self.current_edit_params = clipboard_params;
                    
                    // Save to ALL selected images (batch operation)
                    if let Some(library) = &self.library {
                        let count = self.multi_selection.len();
                        for &image_id in &self.multi_selection {
                            // Skip the current image in the loop, we'll save it explicitly below
                            if Some(image_id) != self.selected_image_id {
                                let _ = library.save_edit_params(image_id, &self.current_edit_params);
                            }
                        }
                        self.status = "Settings pasted to selection".to_string();
                        println!("📋 Pasted settings to {} images", count);
                    }
                    
                    // Explicitly save the current image (triggers log and ensures consistency)
                    self.save_current_edits();
                    
                    // Phase 25: Update GPU uniforms if we pasted to the current image
                    if let EditorStatus::Ready(pipeline) = &self.editor_status {
                        pipeline.update_uniforms(&self.current_edit_params);
                        self.canvas_cache.clear();
                        self.histogram_cache.clear();
                    }
                    
                    // Phase 65: Commit the pasted state to history (for the current image)
                    self.commit_current_state();
                }
                Task::none()
            }

            
            // ========== Phase 55: Multi-Selection Handlers ==========
            
            Message::ModifiersChanged(modifiers) => {
                self.last_modifiers = modifiers;
                Task::none()
            }
            
            // Phase 59: Rating filter
            Message::SetMinRating(rating) => {
                self.min_filter_rating = rating;
                Task::none()
            }
            
            // Phase 60: Toggle HUD
            Message::ToggleInfoHud => {
                self.show_info_hud = !self.show_info_hud;
                Task::none()
            }


            
            // ========== Phase 56: Ratings & Culling Handlers ==========
            
            Message::SetRating(rating) => {
                // Batch support: apply to all selected images
                let target_ids: Vec<i64> = if !self.multi_selection.is_empty() {
                    self.multi_selection.iter().copied().collect()
                } else if let Some(id) = self.selected_image_id {
                    vec![id]
                } else {
                    vec![]
                };
                
                if let Some(library) = &self.library {
                    for &id in &target_ids {
                        // Update database
                        let _ = library.set_image_rating(id, rating);
                        // Update in-memory
                        if let Some(img) = self.images.iter_mut().find(|i| i.id == id) {
                            img.rating = rating;
                        }
                    }
                }
                
                let count = target_ids.len();
                let stars = "★".repeat(rating as usize);
                self.status = if count > 0 {
                    format!("Rated {} image(s): {}", count, if rating > 0 { &stars } else { "None" })
                } else {
                    "No image selected".to_string()
                };
                
                println!("⭐ Set rating {} for {} image(s)", rating, count);
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
                // Phase 67: Disable zoom while cropping to avoid confusion
                if self.is_cropping {
                    return Task::none();
                }

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
                // Phase 67: Disable pan while cropping
                if self.is_cropping {
                    return Task::none();
                }

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
                
                // Phase 67: If cropping, handle interaction
                if self.is_cropping {
                    if let Some(last_pos) = self.last_cursor_position {
                        if let Some(handle) = self.detect_crop_handle(last_pos) {
                            println!("✂️  Dragging handle: {:?}", handle);
                            self.drag_mode = DragMode::CropHandle(handle);
                            self.is_dragging = true;
                        }
                    }
                    return Task::none();
                }

                // Single click - start dragging for panning
                self.is_dragging = true;
                self.drag_mode = DragMode::Pan;
                // Position will be updated by next MouseMoved event
                Task::none()
            }
            
            Message::MouseReleased => {
                // Stop dragging
                if self.is_dragging {
                    if let DragMode::CropHandle(_) = self.drag_mode {
                        // Commit crop change to history
                        self.save_current_edits();
                        self.commit_current_state();
                        
                        // Update GPU uniforms (though we're in crop mode so it might be overridden)
                        if let EditorStatus::Ready(pipeline) = &self.editor_status {
                             pipeline.update_uniforms(&self.current_edit_params);
                        }
                    }
                }
                
                self.is_dragging = false;
                self.drag_mode = DragMode::None;
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
                
                // If dragging, calculate delta
                if self.is_dragging {
                    if let Some(last_pos) = self.last_cursor_position {
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
                        println!("✅ RAW data loaded successfully: {}x{} pixels", raw_data.width, raw_data.height);
                        
                        // Phase 41: Store metadata for inspection
                        self.current_metadata = Some(raw_data.clone());
                        
                        // Phase 15: Calculate proper cam-to-sRGB color matrix
                        let xyz_to_cam = raw_data.color_matrix;
                        let cam_to_srgb = color::calculate_cam_to_srgb(xyz_to_cam);
                        
                        // Phase 42: Use measured black levels if available and valid
                    // If measurement failed (all 0), fall back to metadata
                    let use_measured = raw_data.measured_black_levels.iter().any(|&x| x > 0.0);
                    let black_levels_u32 = if use_measured {
                        println!("Using MEASURED black levels for GPU: {:?}", raw_data.measured_black_levels);
                        [
                            raw_data.measured_black_levels[0] as u32,
                            raw_data.measured_black_levels[1] as u32,
                            raw_data.measured_black_levels[2] as u32,
                            raw_data.measured_black_levels[3] as u32,
                        ]
                    } else {
                        println!("Using METADATA black levels for GPU: {:?}", raw_data.black_levels);
                        raw_data.black_levels
                    };

                    // Initialize GPU pipeline with loaded image data
                    // We pass the UI edit params and the raw metadata separately
                    
                    // Update current edit params with measured black levels if we want to force them?
                    // No, RenderPipeline::new takes black_levels as a separate argument.
                    
                    let image_id = self.selected_image_id.unwrap_or(0);
                    let edit_params = self.current_edit_params.clone();
                        
                    Task::perform(
                        async move {
                            gpu::RenderPipeline::new(
                                image_id,
                                raw_data.data,
                                raw_data.width,
                                raw_data.height,
                                &edit_params, // Pass the cloned params which is Send
                                raw_data.wb_multipliers,
                                cam_to_srgb,
                                raw_data.cfa_pattern,
                                black_levels_u32, // Use our MEASURED black levels here!
                                raw_data.white_level,
                            ).await
                        },
                        |result| Message::GpuPipelineReady(result.map(Arc::new)),
                    )
                    }
                    Err(e) => {
                        let err_msg = format!("Failed to load RAW data: {}", e);
                        eprintln!("❌ {}", err_msg);
                        self.editor_status = EditorStatus::Failed(0, err_msg.clone());
                        self.status = err_msg;
                        Task::none()
                    }
                }
            }
            
            Message::GpuPipelineReady(result) => {
                match result {
                    Ok(pipeline) => {
                        println!("🎨 GPU pipeline initialized!");
                        
                        // Phase 25: Apply current edit params to new pipeline
                        // This ensures edits persist when switching images or reloading
                        pipeline.update_uniforms(&self.current_edit_params);
                        
                        // Store pipeline in EditorStatus::Ready
                        self.editor_status = EditorStatus::Ready(pipeline);
                        
                        // Phase 29: Clear working preview now that full pipeline is ready
                        self.working_preview = None;
                        
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
            
            Message::WorkingPreviewReady(handle) => {
                // Phase 30: Upgrade to higher resolution preview if still loading
                if let EditorStatus::Loading(_) = self.editor_status {
                    println!("✨ Upgraded to 1280px working preview");
                    self.working_preview = Some(handle);
                }
                Task::none()
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
                        let crop = self.current_edit_params.crop;
                        
                        // Run export in background to avoid freezing UI
                        return Task::perform(
                            export_image_async(pipeline_clone, path, crop),
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
            
            // Window Controls
            Message::MinimizeWindow => {
                window::get_latest().and_then(|id| window::minimize(id, true))
            }
            Message::MaximizeWindow => {
                window::get_latest().and_then(window::toggle_maximize)
            }
            Message::CloseWindow => {
                window::get_latest().and_then(window::close)
            }
            Message::DragWindow => {
                window::get_latest().and_then(window::drag)
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
    
    /// Phase 66: Helper to calculate a center crop for a target aspect ratio
    /// Returns [x, y, w, h] in normalized coordinates (0.0 to 1.0)
    fn calculate_center_crop(target_ratio: f32, image_w: u32, image_h: u32) -> [f32; 4] {
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
    fn subscription(&self) -> iced::Subscription<Message> {
        use iced::keyboard;
        use iced::keyboard::key::Named;
        
        iced::event::listen_with(|event, _status, _window| {
            // Phase 55: Track modifier key changes for Ctrl/Cmd+Click
            if let iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
                return Some(Message::ModifiersChanged(modifiers));
            }
            
            if let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event {
                // Phase 54: Check for Ctrl/Cmd+C (Copy) and Ctrl/Cmd+V (Paste)
                let ctrl_or_cmd = modifiers.command();
                
                if ctrl_or_cmd {
                    match key.as_ref() {
                        keyboard::Key::Character("c") | keyboard::Key::Character("C") => return Some(Message::CopySettings),
                        keyboard::Key::Character("v") | keyboard::Key::Character("V") => return Some(Message::PasteSettings),
                        // Phase 65: Undo/Redo Shortcuts
                        keyboard::Key::Character("z") | keyboard::Key::Character("Z") => {
                            if modifiers.shift() {
                                return Some(Message::Redo);
                            } else {
                                return Some(Message::Undo);
                            }
                        }
                        // Phase 65: Ctrl+R for Reset (Alias)
                        keyboard::Key::Character("r") | keyboard::Key::Character("R") => return Some(Message::ResetEdits),
                        _ => {}
                    }
                }
                
                // Other keyboard shortcuts (Phase 24)
                match key.as_ref() {
                    keyboard::Key::Named(Named::Space) => Some(Message::ToggleBeforeAfter),
                    keyboard::Key::Character("r") | keyboard::Key::Character("R") => Some(Message::ResetEdits),
                    keyboard::Key::Named(Named::ArrowRight) => Some(Message::SelectNextImage),
                    keyboard::Key::Named(Named::ArrowLeft) => Some(Message::SelectPreviousImage),
                    // Phase 56: Ratings 0-5
                    keyboard::Key::Character("0") => Some(Message::SetRating(0)),
                    keyboard::Key::Character("1") => Some(Message::SetRating(1)),
                    keyboard::Key::Character("2") => Some(Message::SetRating(2)),
                    keyboard::Key::Character("3") => Some(Message::SetRating(3)),
                    keyboard::Key::Character("4") => Some(Message::SetRating(4)),
                    keyboard::Key::Character("5") => Some(Message::SetRating(5)),
                    // Phase 60: HUD Toggle
                    keyboard::Key::Character("i") | keyboard::Key::Character("I") => Some(Message::ToggleInfoHud),

                    _ => None,
                }
            } else {
                None
            }
        })
    }
    
    /// Build the custom window title bar
    fn view_title_bar(&self) -> Element<Message> {
        // Left: Menus
        let menus = row![
            button(container(text("File").size(13)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .style(ui::styles::WindowControlButton::style)
                .height(Length::Fill)
                .padding([0, 10]),
            button(container(text("Edit").size(13)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .style(ui::styles::WindowControlButton::style)
                .height(Length::Fill)
                .padding([0, 10]),
            button(container(text("Window").size(13)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .style(ui::styles::WindowControlButton::style)
                .height(Length::Fill)
                .padding([0, 10]),
            button(container(text("Help").size(13)).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .style(ui::styles::WindowControlButton::style)
                .height(Length::Fill)
                .padding([0, 10]),
        ]
        .spacing(0)
        .align_y(Alignment::Center);

        // Center: Navigation (Library | Develop)
        let navigation = container(
            row![
                button(
                    container(
                        row![
                            text(ui::icons::FOLDER).font(ICON_FONT).size(14),
                            text(" Library").size(14)
                        ]
                        .spacing(5)
                        .align_y(Alignment::Center)
                    )
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center)
                )
                .height(Length::Fill)
                .padding([0, 15])
                .style(|t, s| ui::styles::TabButton { is_active: self.current_tab == AppTab::Library }.style(t, s))
                .on_press(Message::TabChanged(AppTab::Library)),
                
                button(
                    container(
                        row![
                            text(ui::icons::PAINTBRUSH).font(ICON_FONT).size(14),
                            text(" Develop").size(14)
                        ]
                        .spacing(5)
                        .align_y(Alignment::Center)
                    )
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center)
                )
                .height(Length::Fill)
                .padding([0, 15])
                .style(|t, s| ui::styles::TabButton { is_active: self.current_tab == AppTab::Develop }.style(t, s))
                .on_press(Message::TabChanged(AppTab::Develop)),
            ]
            .spacing(0)
            .height(Length::Fill)
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center);

        // Right: Logo + Window Controls
        let controls = row![
            // Logo
            container(
                text(ui::icons::CAMERA)
                    .font(ICON_FONT)
                    .size(16)
                    .style(|_theme| text::Style { color: Some(Color::from_rgb(0.4, 0.4, 0.4)) })
            )
            .height(Length::Fill)
            .align_y(iced::alignment::Vertical::Center),
                
            iced::widget::Space::with_width(Length::Fixed(15.0)),
            
            // Window Controls
            button(container(text(ui::icons::MINIMIZE).font(ICON_FONT).size(14)).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .on_press(Message::MinimizeWindow)
                .style(ui::styles::WindowControlButton::style)
                .width(Length::Fixed(45.0))
                .height(Length::Fill),
                
            button(container(text(ui::icons::MAXIMIZE).font(ICON_FONT).size(14)).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .on_press(Message::MaximizeWindow)
                .style(ui::styles::WindowControlButton::style)
                .width(Length::Fixed(45.0))
                .height(Length::Fill),
                
            button(container(text(ui::icons::CLOSE).font(ICON_FONT).size(14)).width(Length::Fill).height(Length::Fill).align_x(iced::alignment::Horizontal::Center).align_y(iced::alignment::Vertical::Center))
                .on_press(Message::CloseWindow)
                .style(|_theme, status| {
                    if status == button::Status::Hovered {
                        button::Style {
                            background: Some(Background::Color(Color::from_rgb(0.9, 0.2, 0.2))),
                            text_color: Color::WHITE,
                            ..button::Style::default()
                        }
                    } else {
                        button::Style {
                            text_color: Color::from_rgb(0.7, 0.7, 0.7),
                            ..button::text(_theme, status)
                        }
                    }
                })
                .width(Length::Fixed(45.0))
                .height(Length::Fill),
        ]
        .spacing(0)
        .height(Length::Fill)
        .align_y(Alignment::Center);

        // Assemble Title Bar
        container(
            stack![
                // Layer 1: Menus (Left) and Controls (Right)
                row![
                    menus,
                    // Draggable space
                    iced::widget::mouse_area(
                        container(iced::widget::Space::with_width(Length::Fill))
                    ).on_press(Message::DragWindow),
                    
                    controls,
                ]
                .height(Length::Fill)
                .align_y(Alignment::Center)
                .padding(0),
                
                // Layer 2: Navigation (Centered)
                navigation,
            ]
        )
        .height(Length::Fixed(35.0))
        .style(|_theme| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.05, 0.05, 0.05))),
            ..Default::default()
        })
        .into()
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
                text(ui::icons::HOURGLASS)
                    .size(32)
                    .font(ICON_FONT)
                    .center()
                    .style(|_theme| text::Style {
                        color: Some(Color::from_rgb(0.5, 0.7, 1.0)),
                    }),
                Space::with_height(Length::Fill),
                text("Version 0.4")
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
        // Custom Window Title Bar (Phase 62)
        let title_bar = self.view_title_bar();

        let content = match self.current_tab {
            AppTab::Library => self.view_library(),
            AppTab::Develop => self.view_develop(),
        };
        
        column![
            title_bar,
            content,
        ]
        .into()
    }

    
    /// Build the Library tab view (grid of thumbnails)
    fn view_library(&self) -> Element<Message> {
        // Phase 59: Filter images by rating
        let filtered_images: Vec<&ImageData> = self.images.iter()
            .filter(|img| self.min_filter_rating == 0 || img.rating >= self.min_filter_rating)
            .collect();
        
        // Count thumbnails and deleted files
        let cached_count = filtered_images.iter()
            .filter(|img| img.cache_path_thumb.is_some())
            .count();
        let deleted_count = filtered_images.iter()
            .filter(|img| img.file_status == "deleted")
            .count();
        let total_count = self.images.len();  // Total, not filtered
        let filtered_count = filtered_images.len();
        
        // ========== LEFT PANE: Thumbnail Grid ==========
        
        // Phase 59: Filter bar
        let filter_bar = row![
            text("Filter: ").size(14),
            button(text("All").size(12))
                .on_press(Message::SetMinRating(0))
                .padding(5)
                .style(if self.min_filter_rating == 0 {
                    |_theme: &Theme, _status| button::Style {
                        background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))),
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                } else {
                    |_theme: &Theme, _status| button::Style {
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                }),
            button(row![
                text(ui::icons::STAR).font(ICON_FONT).size(12),
                text(" 1+").size(12)
            ])
                .on_press(Message::SetMinRating(1))
                .padding(5)
                .style(if self.min_filter_rating == 1 {
                   |_theme: &Theme, _status| button::Style {
                        background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))),
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                } else {
                    |_theme: &Theme, _status| button::Style {
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                }),
            button({
                let stars = format!("{} {}", ui::icons::STAR, ui::icons::STAR);
                row![
                    text(stars).font(ICON_FONT).size(12),
                    text(" 2+").size(12)
                ]
            })
                .on_press(Message::SetMinRating(2))
                .padding(5)
                .style(if self.min_filter_rating == 2 {
                    |_theme: &Theme, _status| button::Style {
                        background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))),
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                } else {
                    |_theme: &Theme, _status| button::Style {
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                }),
            button({
                let stars = format!("{} {} {}", ui::icons::STAR, ui::icons::STAR, ui::icons::STAR);
                row![
                    text(stars).font(ICON_FONT).size(12),
                    text(" 3+").size(12)
                ]
            })
                .on_press(Message::SetMinRating(3))
                .padding(5)
                .style(if self.min_filter_rating == 3 {
                    |_theme: &Theme, _status| button::Style {
                        background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))),
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                } else {
                    |_theme: &Theme, _status| button::Style {
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                }),
            button({
                let stars = format!("{} {} {} {}", ui::icons::STAR, ui::icons::STAR, ui::icons::STAR, ui::icons::STAR);
                row![
                    text(stars).font(ICON_FONT).size(12),
                    text(" 4+").size(12)
                ]
            })
                .on_press(Message::SetMinRating(4))
                .padding(5)
                .style(if self.min_filter_rating == 4 {
                    |_theme: &Theme, _status| button::Style {
                        background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))),
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                } else {
                    |_theme: &Theme, _status| button::Style {
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                }),
            button({
                let stars = format!("{} {} {} {} {}", ui::icons::STAR, ui::icons::STAR, ui::icons::STAR, ui::icons::STAR, ui::icons::STAR);
                row![
                    text(stars).font(ICON_FONT).size(12),
                    text(" 5").size(12)
                ]
            })
                .on_press(Message::SetMinRating(5))
                .padding(5)
                .style(if self.min_filter_rating == 5 {
                    |_theme: &Theme, _status| button::Style {
                        background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.7))),
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                } else {
                    |_theme: &Theme, _status| button::Style {
                        text_color: Color::WHITE,
                        ..Default::default()
                    }
                }),
        ]
        .spacing(5)
        .padding(5);
        
        // Header for grid pane
        let grid_header = column![
            text("RAW Editor v0.3 - Culling features")
                .size(24),
            button("Import Folder")
                .on_press(Message::ImportFolder)
                .padding(8),
            text(&self.status).size(12),
            text(format!("Showing: {}/{}  |  Thumbnails: {}/{}  |  Deleted: {}", 
                filtered_count, total_count, cached_count, filtered_count, deleted_count))
                .size(11),
            filter_bar,  // Phase 59: Add filter bar
        ]
        .spacing(10)
        .padding(10);
        
        // Create wrapping grid of clickable thumbnails
        const THUMB_SIZE: u16 = 1; // Equal size for all squares
        
        let thumbnail_grid = filtered_images.iter().fold(
            Wrap::new().spacing(8.0).line_spacing(8.0),
            |wrap, img| {
                // Check if file is deleted
                let is_deleted = img.file_status == "deleted";
                
                // Create thumbnail content
                let thumbnail_content = if is_deleted {
                    // Show deleted file indicator with grey background
                    container(
                        column![
                            text(ui::icons::TIMES).size(24).font(ICON_FONT),
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
                } else if let Some(ref thumb_path) = img.cache_path_thumb {
                    // Phase 28: Show 256px thumbnail tier
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
                        text(ui::icons::HOURGLASS).size(48).font(ICON_FONT)
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
        // 1. Determine which image we are looking at (if any)
        let current_image = self.selected_image_id
            .and_then(|id| self.images.iter().find(|i| i.id == id));

        // 3. Build the Sidebar (always visible, disabled if not ready)
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
        
        // Phase 54: Copy/Paste Shortcuts
        // Phase 54: Copy/Paste Settings buttons - Removed (Duplicate)
        
        let sidebar = sidebar
            // Tone Controls
            .push(text("Tone").size(14))
            .push(slider_row("Exposure", self.current_edit_params.exposure, -5.0..=5.0, 0.1, Message::ExposureChanged))
            .push(slider_row("Contrast", self.current_edit_params.contrast, -10.0..=10.0, 0.005, Message::ContrastChanged))
            .push(slider_row("Highlights", self.current_edit_params.highlights, -1.0..=1.0, 0.01, Message::HighlightsChanged))
            .push(slider_row("Shadows", self.current_edit_params.shadows, -1.0..=1.0, 0.01, Message::ShadowsChanged))
            .push(slider_row("Whites", self.current_edit_params.whites, 0.8..=1.2, 0.01, Message::WhitesChanged))
            .push(slider_row("Blacks", self.current_edit_params.blacks, 0.0..=0.2, 0.005, Message::BlacksChanged))
            
            // Color Controls
            .push(text("Color").size(14))
            .push(slider_row("Temp", self.current_edit_params.temperature, -1.0..=1.0, 0.01, Message::TemperatureChanged))
            .push(slider_row("Tint", self.current_edit_params.tint, -1.0..=1.0, 0.01, Message::TintChanged))
            .push(slider_row("Vibrance", self.current_edit_params.vibrance, -1.0..=1.0, 0.01, Message::VibranceChanged))
            .push(slider_row("Saturation", self.current_edit_params.saturation, -100.0..=100.0, 1.0, Message::SaturationChanged))
            
            // Detail Controls
            .push(text("Detail").size(14))
            .push(slider_row("Denoise", self.current_edit_params.noise_reduction, 0.0..=2.0, 0.01, Message::NoiseReductionChanged))
            .push(slider_row("Sharpen", self.current_edit_params.sharpening, 0.0..=2.0, 0.01, Message::SharpeningChanged))
            .push(slider_row("Masking", self.current_edit_params.sharpen_masking, 0.0..=1.0, 0.01, Message::SharpenMaskingChanged))
            
            // Geometry
            .push(text("Geometry").size(14))
            .push(slider_row("Rotate", self.current_edit_params.rotation, -45.0..=45.0, 0.1, Message::RotationChanged))
            
            // Action Buttons
            .push(
                row![
                    // Copy/Paste Icons
                    button(text(ui::icons::COPY).font(ICON_FONT).size(16))
                        .style(ui::styles::NeutralButton::style)
                        .on_press(Message::CopyEdits)
                        .padding(8),
                    button(text(ui::icons::PASTE).font(ICON_FONT).size(16))
                        .style(ui::styles::NeutralButton::style)
                        .on_press_maybe(self.edit_clipboard.as_ref().map(|_| Message::PasteEdits))
                        .padding(8),
                        
                    iced::widget::Space::with_width(Length::Fill),
                    
                    // Reset
                    button(
                        row![
                            text(ui::icons::RESET).font(ICON_FONT).size(14),
                            text("Reset").size(14)
                        ].spacing(5)
                    )
                    .style(ui::styles::NeutralButton::style)
                    .on_press(Message::ResetEdits)
                    .padding([8, 12]),
                ]
                .spacing(10)
                .width(Length::Fill)
            )
            
            // Phase 66: Crop
            .push(text("Crop").size(14).font(Font { weight: Weight::Bold, ..Default::default() }))
            // Phase 67: Crop Tool Toggle
            .push(
                button(
                    row![
                        text(if self.is_cropping { "Done" } else { "Crop Tool" }).size(14),
                        text(ui::icons::CROP).font(ICON_FONT).size(14)
                    ]
                    .spacing(5)
                    .align_y(Alignment::Center)
                )
                .style(if self.is_cropping { ui::styles::AccentButton::style } else { ui::styles::NeutralButton::style })
                .on_press(Message::ToggleCropMode)
                .width(Length::Fill)
            )
            .push(
                row![
                    button(text("Reset").size(12))
                        .style(ui::styles::NeutralButton::style)
                        .on_press(Message::SetCrop([0.0, 0.0, 1.0, 1.0])),
                    
                    button(text("1:1").size(12))
                        .style(ui::styles::NeutralButton::style)
                        .on_press_maybe(
                            if let EditorStatus::Ready(pipeline) = &self.editor_status {
                                Some(Message::SetCrop(Self::calculate_center_crop(1.0, pipeline.width, pipeline.height)))
                            } else {
                                None
                            }
                        ),
                        
                    button(text("16:9").size(12))
                        .style(ui::styles::NeutralButton::style)
                        .on_press_maybe(
                            if let EditorStatus::Ready(pipeline) = &self.editor_status {
                                Some(Message::SetCrop(Self::calculate_center_crop(16.0/9.0, pipeline.width, pipeline.height)))
                            } else {
                                None
                            }
                        ),
                        
                    button(text("2:3").size(12))
                        .style(ui::styles::NeutralButton::style)
                        .on_press_maybe(
                            if let EditorStatus::Ready(pipeline) = &self.editor_status {
                                Some(Message::SetCrop(Self::calculate_center_crop(2.0/3.0, pipeline.width, pipeline.height)))
                            } else {
                                None
                            }
                        ),
                ]
                .spacing(5)
            );
            
        let mut sidebar = sidebar;

        // Phase 60: Hide sensor correction debug controls by default
        if crate::debug::SHOW_SENSOR_CORRECTION {
            sidebar = sidebar
                .push(text("Sensor Correction").size(14))
                .push(checkbox("Shift Grid X", self.current_edit_params.black_phase_x != 0)
                    .on_toggle(|checked| Message::BlackPhaseChanged(false, if checked { 1 } else { 0 })))
                .push(checkbox("Shift Grid Y", self.current_edit_params.black_phase_y != 0)
                    .on_toggle(|checked| Message::BlackPhaseChanged(true, if checked { 1 } else { 0 })))
                
                .push(text(format!("Black TL (Red): {:.1}", self.current_edit_params.black_offsets[0])).size(12))
                .push(slider(-50.0..=50.0, self.current_edit_params.black_offsets[0], |v| Message::BlackOffsetChanged(0, v)).step(0.1))
                
                .push(text(format!("Black TR (Green): {:.1}", self.current_edit_params.black_offsets[1])).size(12))
                .push(slider(-50.0..=50.0, self.current_edit_params.black_offsets[1], |v| Message::BlackOffsetChanged(1, v)).step(0.1))
                
                .push(text(format!("Black BL (Green): {:.1}", self.current_edit_params.black_offsets[2])).size(12))
                .push(slider(-50.0..=50.0, self.current_edit_params.black_offsets[2], |v| Message::BlackOffsetChanged(2, v)).step(0.1))
                
                .push(text(format!("Black BR (Blue): {:.1}", self.current_edit_params.black_offsets[3])).size(12))
                .push(slider(-50.0..=50.0, self.current_edit_params.black_offsets[3], |v| Message::BlackOffsetChanged(3, v)).step(0.1));
        }
            
        let sidebar = sidebar
            .push(
                button(
                    row![
                        text(ui::icons::SAVE).font(ICON_FONT).size(14),
                        text(" Export Image").size(14)
                    ]
                    .spacing(5)
                    .align_y(Alignment::Center)
                )
                .style(ui::styles::AccentButton::style)
                .on_press(Message::ExportImage)
                .padding(12)
                .width(Length::Fill)
            )
            
            // Phase 41: Metadata Info
            // Removed: Metadata info is now hidden by default.
        .spacing(10)
        .padding(15);
        
        // Wrap sidebar in scrollable to allow access to all controls
        let sidebar_scrollable = scrollable(sidebar)
            .width(Length::Fixed(300.0))
            .height(Length::Fill);
        
        let sidebar_container = container(sidebar_scrollable)
            .style(|_theme| {
                container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.15, 0.15, 0.15))),
                    ..Default::default()
                }
            });


        // 4. Build the Main Content Area based on state
        // Phase 31: Unified Image Container for seamless transitions
        
        // Determine the image handle and overlay based on state
        let (image_handle, overlay_content) = match &self.editor_status {
            EditorStatus::NoSelection => (None, Option::<Element<Message>>::None),
            EditorStatus::Loading(_) => {
                // Use working preview if available, otherwise None
                let handle = self.working_preview.clone();
                
                // Create loading overlay
                let overlay = container(
                    column![
                        row![
                            text(ui::icons::HOURGLASS).font(ICON_FONT).size(14)
                                .style(|theme: &Theme| {
                                    text::Style {
                                        color: Some(Color::WHITE),
                                    }
                                }),
                            text(" Loading RAW...").size(14)
                                .style(|theme: &Theme| {
                                    text::Style {
                                        color: Some(Color::WHITE),
                                    }
                                })
                        ],
                    ]
                    .padding(8)
                )
                .style(|_theme| {
                    container::Style {
                        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
                        border: Border {
                            radius: 4.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
                .padding(20)
                .align_x(iced::Alignment::End)
                .align_y(iced::Alignment::End);
                
                (handle, Some(overlay.into()))
            }
            EditorStatus::Ready(pipeline) => {
                // GPU pipeline ready - render frame
                
                // Phase 25: GPU-Accelerated Zoom & Pan
                let mut params_to_render = if self.show_before {
                    state::edit::EditParams::default()
                } else {
                    self.current_edit_params.clone()
                };
                
                // Phase 67: If cropping, render full image (no crop, no zoom)
                if self.is_cropping {
                    params_to_render.crop = [0.0, 0.0, 1.0, 1.0];
                    pipeline.update_uniforms_with_zoom(&params_to_render, 1.0, 0.0, 0.0);
                } else {
                    pipeline.update_uniforms_with_zoom(&params_to_render, self.zoom, self.pan_offset.x, self.pan_offset.y);
                }
                
                // Phase 66: Calculate dynamic preview size based on crop
                // This ensures correct aspect ratio AND "digital zoom" (higher detail when cropped)
                let crop = params_to_render.crop;
                let crop_w = crop[2];
                let crop_h = crop[3];
                
                // Calculate aspect ratio of the CROP
                // AR = (OriginalW * CropW) / (OriginalH * CropH)
                let original_aspect = pipeline.width as f32 / pipeline.height as f32;
                let crop_aspect = original_aspect * (crop_w / crop_h);
                
                // Determine target dimensions (max 1280px on long edge)
                const MAX_PREVIEW_SIZE: u32 = 1280;
                let (target_w, target_h) = if crop_aspect > 1.0 {
                    // Landscape
                    let w = MAX_PREVIEW_SIZE;
                    let h = (w as f32 / crop_aspect) as u32;
                    (w, h)
                } else {
                    // Portrait
                    let h = MAX_PREVIEW_SIZE;
                    let w = (h as f32 * crop_aspect) as u32;
                    (w, h)
                };
                
                let rgba_bytes = pipeline.render_to_bytes(target_w, target_h);
                
                // Phase 22: Histogram
                if self.histogram_enabled {
                    let histogram_bytes = pipeline.render_to_histogram_bytes();
                    let histogram = pipeline.calculate_histogram(&histogram_bytes);
                    *self.histogram_data.borrow_mut() = histogram;
                    self.histogram_cache.clear();
                }
                
                let handle = iced::widget::image::Handle::from_rgba(
                    target_w,
                    target_h,
                    rgba_bytes
                );
                
                (Some(handle), None)
            }
            EditorStatus::Failed(_, error) => {
                // For failed state, we return None handle and an error overlay
                let overlay = container(
                    column![
                        row![
                            text(ui::icons::TIMES).font(ICON_FONT).size(24),
                            text(" Preview Failed").size(24)
                        ],
                        text("").size(20),
                        text(error.clone())
                            .size(14)
                            .style(|theme: &Theme| {
                                text::Style {
                                    color: Some(theme.palette().danger),
                                }
                            }),
                    ]
                    .padding(40)
                    .align_x(Alignment::Center)
                );
                (None, Some(overlay.into()))
            }
        };

        // Construct the Unified Container
        let main_content: Element<Message> = if let EditorStatus::NoSelection = self.editor_status {
            // Special case for NoSelection (keep existing design)
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
        } else {
            // Unified container for Loading, Ready, and Failed states
            
            // 1. The Image Widget - different rendering for preview vs RAW
            let image_widget: Element<Message> = if let Some(handle) = image_handle {
                // Check if we're showing a preview (Loading) or RAW (Ready)
                match &self.editor_status {
                    EditorStatus::Loading(_) => {
                        // Phase 32: Use Canvas-based preview renderer for JPEG previews
                        // This applies zoom/pan to match what the RAW will show
                        use iced::widget::canvas::Canvas;
                        use crate::ui::preview_renderer::PreviewRenderer;
                        
                        Canvas::new(PreviewRenderer {
                            handle,
                            zoom: self.zoom,
                            offset: self.pan_offset,
                            is_cropping: false,
                            crop: [0.0, 0.0, 1.0, 1.0],
                            image_width: 3, // Dummy 3:2 aspect
                            image_height: 2,
                        })
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                    }
                    EditorStatus::Ready(pipeline) => {
                        if self.is_cropping {
                            // Phase 67: Use PreviewRenderer for interactive cropping overlay
                            use iced::widget::canvas::Canvas;
                            use crate::ui::preview_renderer::PreviewRenderer;
                            
                            Canvas::new(PreviewRenderer {
                                handle,
                                zoom: self.zoom,
                                offset: self.pan_offset,
                                is_cropping: true,
                                crop: self.current_edit_params.crop,
                                image_width: pipeline.width,
                                image_height: pipeline.height,
                            })
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .into()
                        } else {
                            // RAW image: Pan/zoom already baked into pixels by GPU shader
                            // Use plain Image widget WITHOUT any transformation
                            Image::new(handle)
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .content_fit(iced::ContentFit::Contain)
                                .into()
                        }
                    }
                    _ => {
                        // Fallback for other states (shouldn't happen with current logic)
                        Image::new(handle)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .content_fit(iced::ContentFit::Contain)
                            .into()
                    }
                }
            } else {
                // Placeholder if no image yet (e.g. loading without preview)
                iced::widget::Space::new(Length::Fill, Length::Fill).into()
            };
            
            // Phase 60: Canvas Overlay (HUD)
            let overlay = if self.show_info_hud {
                container(
                    column![
                        // Camera Info
                        text(format!("{} {}", 
                            self.current_metadata.as_ref().map(|m| m.make.clone()).unwrap_or_default(),
                            self.current_metadata.as_ref().map(|m| m.model.clone()).unwrap_or_default()
                        )).size(12).style(|_theme| text::Style { color: Some(Color::WHITE) }),
                        
                        // Lens Info
                        text(self.current_metadata.as_ref().map(|m| m.lens.clone()).unwrap_or_default())
                            .size(12).style(|_theme| text::Style { color: Some(Color::WHITE) }),
                            
                        // Settings
                        text(format!("ISO {}  {}  f/{}", 
                            self.current_metadata.as_ref().map(|m| m.iso.to_string()).unwrap_or("---".to_string()),
                            self.current_metadata.as_ref().map(|m| m.shutter_speed.clone()).unwrap_or("---".to_string()),
                            self.current_metadata.as_ref().map(|m| m.aperture.to_string()).unwrap_or("---".to_string())
                        )).size(12).style(|_theme| text::Style { color: Some(Color::WHITE) }),
                    ]
                    .spacing(2)
                )
                .padding(10)
                .style(|_theme| container::Style {
                    background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))),
                    border: iced::border::Border {
                        radius: 5.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
            } else {
                container(column![]) // Empty container if hidden
            };

            // Filename Overlay (Bottom-Left)
            let filename_overlay = container(
                text(self.selected_image_id
                    .and_then(|id| self.images.iter().find(|img| img.id == id))
                    .map(|img| img.filename.clone())
                    .unwrap_or_default())

                    .size(12)
                    .style(|_theme| text::Style { color: Some(Color::WHITE) })
            )
            .padding(5)
            .style(|_theme| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.4))),
                border: iced::border::Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

            // Stack layers
            let stacked_image = stack![
                image_widget,
                
                // HUD (Top-Left)
                container(overlay)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Left)
                    .align_y(iced::alignment::Vertical::Top)
                    .padding(10),
                    
                // Filename (Bottom-Left)
                container(filename_overlay)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Left)
                    .align_y(iced::alignment::Vertical::Bottom)
                    .padding(10),
            ];
            
            // 2. Wrap in Mouse Area (ALWAYS present to capture events)
            use iced::widget::mouse_area;
            use iced::mouse::{self, ScrollDelta};
            
            let interactive_image = mouse_area(stacked_image)
                .on_scroll(|delta| {
                    let zoom_delta = match delta {
                        ScrollDelta::Lines { y, .. } => y * 0.1,
                        ScrollDelta::Pixels { y, .. } => y * 0.01,
                    };
                    Message::Zoom(zoom_delta, Point::new(-1.0, -1.0))
                })
                .on_press(Message::MousePressed)
                .on_release(Message::MouseReleased)
                .on_move(|position| Message::MouseMoved(position));
                
            // 3. Stack with Overlay (if any)
            let content_stack = if let Some(overlay) = overlay_content {
                iced::widget::stack![
                    interactive_image,
                    overlay
                ]
            } else {
                iced::widget::stack![
                    interactive_image
                ]
            };
            
            // 4. Final Container (Consistent styling + CLIPPING)
            // Phase 32: Add clip to prevent canvas from drawing outside bounds
            container(content_stack)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .clip(true) // ← CRITICAL: Clip content to prevent overflow
                .style(|_theme| {
                    container::Style {
                        background: Some(Background::Color(Color::BLACK)),
                        ..Default::default()
                    }
                })
                .into()
        };

        // 5. Assemble the final layout
        // If no image selected, just show main content (prompt)
        if matches!(self.editor_status, EditorStatus::NoSelection) {
            return main_content;
        }

        // Otherwise, show Header + (Main | Sidebar)
        // Otherwise, show Main | Sidebar
        let editor_content = column![
            row![
                main_content,
                sidebar_container,
            ]
            .spacing(0)
                .height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        
        // Phase 53: Filmstrip view
        // Phase 59: Filter by rating
        let filtered_images: Vec<&ImageData> = self.images.iter()
            .filter(|img| self.min_filter_rating == 0 || img.rating >= self.min_filter_rating)
            .collect();
        let filmstrip = ui::filmstrip::view(&filtered_images, &self.multi_selection);
        
        column![
            editor_content,
            Container::new(filmstrip)
                .width(Length::Fill)
                .height(Length::Fixed(115.0))  // Phase 56: Compact with overlaid stars
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }


    /// Set the application theme
    fn theme(&self) -> Theme {
        Theme::Dark
    }
}

/// Phase 19: Export helper (runs in background)
async fn export_image_async(pipeline: Arc<gpu::RenderPipeline>, save_path: std::path::PathBuf, crop: [f32; 4]) -> Result<std::path::PathBuf, String> {
    tokio::task::spawn_blocking(move || {
        println!("🖼️  Starting full-resolution export...");
        
        // Render at FULL resolution (24MP for 6016x4016 image)
        // This will take 1-2 seconds - that's why we're async!
        // Phase 67: Pass crop to ensure correct output dimensions
        let rgba_bytes = pipeline.render_full_res_to_bytes(crop);
        println!("✅ Rendered {} bytes at full resolution", rgba_bytes.len());
        
        // Determine format from file extension
        let extension = save_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg")
            .to_lowercase();
        
        let (width, height) = if crop[2] > 0.0 && crop[3] > 0.0 {
            // Calculate dimensions from crop
            let (full_w, full_h) = pipeline.dimensions();
            (
                (full_w as f32 * crop[2]) as u32,
                (full_h as f32 * crop[3]) as u32
            )
        } else {
            pipeline.dimensions()
        };

        // Save using image crate
        let result = match extension.as_str() {
            "png" => {
                image::save_buffer(
                    &save_path,
                    &rgba_bytes,
                    width,
                    height,
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
                    width,
                    height,
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
        RawEditor::title,
        RawEditor::update,
        RawEditor::view,
    )
    .subscription(RawEditor::subscription) // Phase 24: Enable keyboard shortcuts
    .theme(RawEditor::theme)
    .font(ICON_FONT_BYTES)  // Phase 57: Load embedded font
    .default_font(ICON_FONT)  // Phase 57: Set as default
    // Phase 23: Window settings - start with normal window (has title bar)
    // Note: iced::application() uses a single window throughout
    // To have a separate splash window, you'd need the multi-window API
    .window(iced::window::Settings {
        size: iced::Size::new(900.0, 350.0),  // Main app size
        min_size: Some(iced::Size::new(600.0, 350.0)),
        decorations: false,  // Remove title bar
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

/// Phase 28: Async function to process one multi-tier cache job
/// Processes one 'pending' image and generates all 3 cache tiers
async fn process_cache_async(db_path: PathBuf) -> Result<(i64, String, String, String), (i64, String)> {
    // Open database connection
    let conn = Connection::open(&db_path)
        .map_err(|e| (0, format!("Failed to open database: {}", e)))?;
    
    // Find one pending image
    let pending_image: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, path FROM images WHERE cache_status = 'pending' LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    
    if let Some((image_id, raw_path_str)) = pending_image {
        // Process in blocking task (image decoding is CPU-intensive)
        let result = tokio::task::spawn_blocking(move || {
            let cache_dir = std::path::PathBuf::from("/tmp"); // Not used by processor
            raw::processor::process_image(
                std::path::Path::new(&raw_path_str),
                image_id,
                &cache_dir,
            )
        })
        .await
        .map_err(|e| (image_id, format!("Task join error: {}", e)))?;
        
        match result {
            Ok((thumb, instant, working)) => Ok((image_id, thumb, instant, working)),
            Err(e) => Err((image_id, e)),
        }
    } else {
        // No pending images
        Err((0, "No pending images".to_string()))
    }
}

/// Phase 30: Async helper to load an image handle from disk
/// Used to load the 1280px working preview in the background
async fn load_image_handle(path: String) -> iced::widget::image::Handle {
    // This runs in a background thread via Task::perform
    iced::widget::image::Handle::from_path(path)
}
