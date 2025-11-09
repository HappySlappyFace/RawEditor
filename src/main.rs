use iced::{Background, Border, Color, Element, Task, Theme};
use iced::widget::{button, column, container, row, scrollable, text, Image, slider};
use iced::advanced::image::Handle;
use iced::{Alignment, Length};
use iced_aw::Wrap;
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
    /// The catalog database
    library: state::library::Library,
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
    /// Cached GPU-rendered image (to avoid re-rendering every frame)
    cached_gpu_image: Option<(state::edit::EditParams, Handle)>,
}

/// Application messages (events)
#[derive(Debug, Clone)]
enum Message {
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
    /// User changed temperature slider
    TemperatureChanged(i32),
    /// User changed tint slider
    TintChanged(i32),
    /// User clicked Reset button to clear all edits
    ResetEdits,
    
    // ========== GPU Pipeline Messages ==========
    /// Background RAW data loading completed
    RawDataLoaded(Result<raw::loader::RawDataResult, String>),
    /// GPU pipeline initialization completed
    GpuPipelineReady(Result<Arc<gpu::RenderPipeline>, String>),
}

impl RawEditor {
    /// Create a new instance of the application
    fn new() -> (Self, Task<Message>) {
        // Initialize the database
        // If this fails, we panic because the app cannot function without its database
        let library = state::library::Library::new()
            .expect("Failed to initialize database. Check permissions and disk space.");
        
        // Verify thumbnails exist on disk (reset if deleted)
        let _ = library.verify_thumbnails();
        
        // Verify RAW files exist on disk (mark as deleted if missing)
        let _ = library.verify_files();
        
        // Load all images from the database
        let images = library.get_all_images().unwrap_or_default();
        let image_count = images.len();
        
        println!("🎨 RAW Editor initialized with {} images", image_count);
        
        let status = format!("Loaded {} images.", image_count);
        
        // Get database path for background thumbnail generation
        let db_path = library.path().clone();
        
        // Initialize preview cache directory
        let preview_cache_dir = raw::preview::get_preview_cache_dir();
        
        (
            RawEditor { 
                library, 
                status, 
                images,
                selected_image_id: None,
                preview_cache_dir,
                current_tab: AppTab::Library, // Start in Library tab
                current_edit_params: state::edit::EditParams::default(), // No edits initially
                editor_status: EditorStatus::NoSelection, // GPU pipeline created on demand
                cached_gpu_image: None, // No cached image initially
            },
            // Start thumbnail generation in the background
            Task::perform(
                generate_thumbnails_async(db_path),
                Message::ThumbnailGenerated,
            ),
        )
    }

    /// Handle application messages and update state
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ImportFolder => {
                // Show the native folder picker dialog
                let folder = FileDialog::new()
                    .set_title("Select Folder with RAW Photos")
                    .pick_folder();
                
                if let Some(folder_path) = folder {
                    // Update status to show we're importing
                    self.status = format!("Importing from {}...", folder_path.display());
                    
                    // Get the database path for the background thread
                    let db_path = self.library.path().clone();
                    
                    // Launch async import task
                    return Task::perform(
                        import_folder_async(folder_path, db_path),
                        Message::ImportComplete,
                    );
                }
                
                Task::none()
            }
            Message::ImportComplete(result) => {
                // Reload images from database to show newly imported files
                self.images = self.library.get_all_images().unwrap_or_default();
                
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
                let db_path = self.library.path().clone();
                Task::perform(
                    generate_thumbnails_async(db_path),
                    Message::ThumbnailGenerated,
                )
            }
            Message::ThumbnailGenerated(result) => {
                // Always reload images to show updated thumbnail in the grid
                self.images = self.library.get_all_images().unwrap_or_default();
                
                // Check both fast and slow queues
                let fast_queue_count: i64 = self.library.conn()
                    .query_row(
                        "SELECT COUNT(*) FROM images WHERE cache_status = 'pending'",
                        [],
                        |row| row.get(0)
                    )
                    .unwrap_or(0);
                
                let slow_queue_count: i64 = self.library.conn()
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
                    
                    let db_path = self.library.path().clone();
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
                    
                    let db_path = self.library.path().clone();
                    return Task::perform(
                        generate_thumbnails_async(db_path),
                        Message::ThumbnailGenerated,
                    );
                } else {
                    // Both queues empty - all done!
                    self.status = format!("✅ All thumbnails generated! ({} images)", self.images.len());
                }
                
                Task::none()
            }
            Message::ImageSelected(image_id) => {
                // Update the selected image
                self.selected_image_id = Some(image_id);
                println!("🖼️  Selected image ID: {}", image_id);
                
                // Load edit parameters from database (or use default if no edits)
                self.current_edit_params = self.library.load_edit_params(image_id)
                    .unwrap_or_else(|_| state::edit::EditParams::default());
                
                if !self.current_edit_params.is_unedited() {
                    println!("📝 Loaded existing edits for image {}", image_id);
                }
                
                // Find the selected image
                if let Some(img) = self.images.iter().find(|i| i.id == image_id) {
                    let raw_path = img.path.clone();
                    
                    // Set editor status to loading
                    self.editor_status = EditorStatus::Loading(image_id);
                    
                    // Generate JPEG preview for thumbnails (if needed)
                    let preview_task = if img.preview_path.is_none() {
                        let raw_path_preview = raw_path.clone();
                        let preview_cache_dir = self.preview_cache_dir.clone();
                        Task::perform(
                            async move {
                                let result = raw::preview::generate_full_preview(
                                    raw_path_preview,
                                    image_id,
                                    preview_cache_dir
                                ).await;
                                PreviewResult {
                                    image_id,
                                    preview_path: result,
                                }
                            },
                            Message::PreviewGenerated,
                        )
                    } else {
                        Task::none()
                    };
                    
                    // Load RAW sensor data for GPU processing
                    let raw_task = Task::perform(
                        raw::loader::load_raw_data(raw_path),
                        Message::RawDataLoaded,
                    );
                    
                    // Run both tasks
                    Task::batch(vec![preview_task, raw_task])
                } else {
                    Task::none()
                }
            }
            Message::PreviewGenerated(result) => {
                // Update database with preview path for thumbnails
                if let Ok(ref path) = result.preview_path {
                    let _ = self.library.set_image_preview_path(result.image_id, path);
                    
                    // Update in-memory image data
                    if let Some(img) = self.images.iter_mut().find(|i| i.id == result.image_id) {
                        img.preview_path = Some(path.clone());
                    }
                    
                    println!("✅ Preview cached for image {}", result.image_id);
                } else if let Err(ref err) = result.preview_path {
                    eprintln!("❌ Preview failed for image {}: {}", result.image_id, err);
                }
                
                Task::none()
            }
            Message::TabChanged(tab) => {
                // Switch to the selected tab
                self.current_tab = tab;
                
                // When switching to Develop tab with a selected image, ensure GPU pipeline is ready
                if tab == AppTab::Develop {
                    if let Some(image_id) = self.selected_image_id {
                        // Check if we need to load GPU pipeline
                        if matches!(self.editor_status, EditorStatus::NoSelection) {
                            // Trigger GPU pipeline loading
                            return self.update(Message::ImageSelected(image_id));
                        }
                    }
                }
                
                Task::none()
            }
            
            // ========== Edit Parameter Slider Handlers ==========
            
            Message::ExposureChanged(value) => {
                self.current_edit_params.exposure = value;
                self.save_current_edits();
                // Update GPU uniforms and invalidate cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.cached_gpu_image = None; // Force re-render
                }
                Task::none()
            }
            Message::ContrastChanged(value) => {
                self.current_edit_params.contrast = value;
                self.save_current_edits();
                // Update GPU uniforms and invalidate cache
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                    self.cached_gpu_image = None; // Force re-render
                }
                Task::none()
            }
            Message::HighlightsChanged(value) => {
                self.current_edit_params.highlights = value;
                self.save_current_edits();
                // Update GPU uniforms and trigger redraw
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                }
                Task::none()
            }
            Message::ShadowsChanged(value) => {
                self.current_edit_params.shadows = value;
                self.save_current_edits();
                // Update GPU uniforms and trigger redraw
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                }
                Task::none()
            }
            Message::WhitesChanged(value) => {
                self.current_edit_params.whites = value;
                self.save_current_edits();
                // Update GPU uniforms and trigger redraw
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                }
                Task::none()
            }
            Message::BlacksChanged(value) => {
                self.current_edit_params.blacks = value;
                self.save_current_edits();
                // Update GPU uniforms and trigger redraw
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                }
                Task::none()
            }
            Message::VibranceChanged(value) => {
                self.current_edit_params.vibrance = value;
                self.save_current_edits();
                // Update GPU uniforms and trigger redraw
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                }
                Task::none()
            }
            Message::SaturationChanged(value) => {
                self.current_edit_params.saturation = value;
                self.save_current_edits();
                // Update GPU uniforms and trigger redraw
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                }
                Task::none()
            }
            Message::TemperatureChanged(value) => {
                self.current_edit_params.temperature = value;
                self.save_current_edits();
                // Update GPU uniforms and trigger redraw
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                }
                Task::none()
            }
            Message::TintChanged(value) => {
                self.current_edit_params.tint = value;
                self.save_current_edits();
                // Update GPU uniforms and trigger redraw
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                }
                Task::none()
            }
            Message::ResetEdits => {
                // Reset all edit parameters to default
                self.current_edit_params.reset();
                
                // Save to database (or delete the edit record)
                if let Some(image_id) = self.selected_image_id {
                    let _ = self.library.delete_edits(image_id);
                    println!("♻️  Reset edits for image {}", image_id);
                }
                
                // Update GPU uniforms and trigger redraw
                if let EditorStatus::Ready(pipeline) = &self.editor_status {
                    pipeline.update_uniforms(&self.current_edit_params);
                }
                
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
                        
                        Task::perform(
                            async move {
                                gpu::RenderPipeline::new(
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
        }
    }
    
    /// Helper to save current edit parameters to database
    fn save_current_edits(&self) {
        if let Some(image_id) = self.selected_image_id {
            if let Err(e) = self.library.save_edit_params(image_id, &self.current_edit_params) {
                eprintln!("⚠️  Failed to save edits for image {}: {:?}", image_id, e);
            } else {
                println!("💾 Saved edits for image {}", image_id);
            }
        }
    }

    /// Build the user interface
    fn view(&self) -> Element<Message> {
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
            text("RAW Editor v0.0.10A - GPU Pipeline libraries")
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
        
        // Wrap grid in scrollable container
        let grid_pane = column![
            grid_header,
            scrollable(thumbnail_grid)
                .height(Length::Fill)
                .width(Length::Fill),
        ]
        .width(Length::FillPortion(2)); // 2/3 of screen
        
        // ========== RIGHT PANE: Simple Preview ==========
        
        let editor_content = if let Some(image_id) = self.selected_image_id {
            if let Some(img) = self.images.iter().find(|i| i.id == image_id) {
                // Show selected image preview (if cached)
                if let Some(ref preview_path) = img.preview_path {
                    let handle = Handle::from_path(preview_path.clone());
                    column![
                        text(&img.filename).size(18),
                        text("").size(10),
                        // Preview image
                        scrollable(
                            Image::new(handle)
                                .width(Length::Fill)
                        )
                        .height(Length::Fill),
                        text("").size(10),
                        text(format!("Status: {}", if img.file_status == "deleted" { "❌ Deleted" } else { "✅ Exists" }))
                            .size(12),
                    ]
                    .spacing(5)
                    .padding(10)
                } else {
                    column![
                        text(&img.filename).size(20),
                        text("").size(20),
                        text("Switch to Develop tab to load full preview").size(14),
                    ]
                    .padding(20)
                    .align_x(Alignment::Center)
                }
            } else {
                column![text("Image not found").size(18)].padding(20)
            }
        } else {
            column![
                text("No Image Selected").size(24),
                text("").size(20),
                text("← Click a thumbnail to select")
                    .size(16)
                    .style(|theme: &Theme| {
                        text::Style {
                            color: Some(theme.palette().text.scale_alpha(0.6)),
                        }
                    }),
            ]
            .padding(20)
            .align_x(Alignment::Center)
        };
        
        let editor_pane = container(editor_content)
            .width(Length::FillPortion(1)) // 1/3 of screen
            .height(Length::Fill)
            .padding(10);
        
        // ========== Main Layout: Two-Pane Row ==========
        
        let main_row = row![
            grid_pane,
            editor_pane,
        ]
        .spacing(0);
        
        container(main_row)
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
                        
                        // 🎨 Phase 12: GPU Rendering with Debayering + Smart Caching
                        let image_handle = if let Some((cached_params, cached_handle)) = &self.cached_gpu_image {
                            if cached_params == &self.current_edit_params {
                                // Use cached image (no re-render!)
                                println!("⚡ Using cached GPU image");
                                cached_handle.clone()
                            } else {
                                // Params changed, render new with debayering
                                println!("🎨 GPU rendering {}x{} preview...", pipeline.preview_width, pipeline.preview_height);
                                let rgba_bytes = pipeline.render_to_bytes();
                                println!("✅ Rendered {} bytes (preview)", rgba_bytes.len());
                                Handle::from_rgba(pipeline.preview_width, pipeline.preview_height, rgba_bytes)
                            }
                        } else {
                            // First render
                            println!("🎨 First GPU render (preview)...");
                            let rgba_bytes = pipeline.render_to_bytes();
                            println!("✅ Rendered {} bytes (preview)", rgba_bytes.len());
                            Handle::from_rgba(pipeline.preview_width, pipeline.preview_height, rgba_bytes)
                        };
                        
                        let gpu_image = Image::new(image_handle)
                            .content_fit(iced::ContentFit::Contain);
                        
                        let preview = container(gpu_image)
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
                    
                    // Right sidebar with editing controls (placeholder for future)
                    let sidebar = column![
                        text("Edit Controls").size(16),
                        
                        // Exposure
                        text(format!("Exposure: {:.2}", self.current_edit_params.exposure)),
                        slider(-5.0..=5.0, self.current_edit_params.exposure, Message::ExposureChanged)
                            .step(0.1),
                        
                        // Highlights (Phase 17: Smart Tone - Detail Recovery)
                        text(format!("Highlights: {:.0}", self.current_edit_params.highlights * 100.0)),
                        slider(-1.0..=1.0, self.current_edit_params.highlights, Message::HighlightsChanged)
                            .step(0.01),
                        
                        // Shadows (Phase 17: Smart Tone - Shadow Lift)
                        text(format!("Shadows: {:.0}", self.current_edit_params.shadows * 100.0)),
                        slider(-1.0..=1.0, self.current_edit_params.shadows, Message::ShadowsChanged)
                            .step(0.01),
                        
                        // Contrast  
                        text(format!("Contrast: {:.0}", self.current_edit_params.contrast)),
                        slider(-100.0..=100.0, self.current_edit_params.contrast, Message::ContrastChanged),
                        
                        // Saturation (Phase 15: Color boost!)
                        text(format!("Saturation: {:.0}", self.current_edit_params.saturation)),
                        slider(-100.0..=100.0, self.current_edit_params.saturation, Message::SaturationChanged),
                        
                        // Whites (Phase 16: Tone Controls)
                        text(format!("Whites: {:.2}", self.current_edit_params.whites)),
                        slider(0.8..=1.2, self.current_edit_params.whites, Message::WhitesChanged)
                            .step(0.01),
                        
                        // Blacks (Phase 16: Tone Controls)
                        text(format!("Blacks: {:.3}", self.current_edit_params.blacks)),
                        slider(0.0..=0.2, self.current_edit_params.blacks, Message::BlacksChanged)
                            .step(0.005),
                        
                        // ... repeat for remaining parameters ...
                        
                        button("Reset All").on_press(Message::ResetEdits),
                    ]
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

fn main() -> iced::Result {
    iced::application(
        "RAW Editor",
        RawEditor::update,
        RawEditor::view,
    )
    .theme(RawEditor::theme)
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
