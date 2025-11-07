use iced::{Element, Task, Theme};
use iced::widget::{button, column, container, row, scrollable, text, Image};
use iced::advanced::image::Handle;
use iced::{Alignment, Length};
use iced_aw::Wrap;
use rfd::FileDialog;
use rusqlite::{Connection, ErrorCode};
use std::path::PathBuf;
use walkdir::WalkDir;
use chrono::Utc;

// Declare the state and raw modules
mod state;
mod raw;

// Import shared data structures (alias to avoid conflict with iced's image widget)
use state::data::Image as ImageData;

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
        
        (
            RawEditor { 
                library, 
                status, 
                images,
                selected_image_id: None,
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
                // Reload images to show updated thumbnail paths
                self.images = self.library.get_all_images().unwrap_or_default();
                
                println!(
                    "🖼️  Generated {} thumbnails",
                    result.generated_count
                );
                
                // Update status to show thumbnail generation progress
                let pending_count = self.library.get_pending_thumbnails(1)
                    .map(|imgs| imgs.len())
                    .unwrap_or(0);
                
                if pending_count > 0 {
                    self.status = format!(
                        "Generating thumbnails... {} remaining",
                        pending_count
                    );
                    
                    // Continue generating more thumbnails
                    let db_path = self.library.path().clone();
                    Task::perform(
                        generate_thumbnails_async(db_path),
                        Message::ThumbnailGenerated,
                    )
                } else {
                    self.status = format!(
                        "Ready. {} images in library. All thumbnails generated!",
                        self.images.len()
                    );
                    Task::none()
                }
            }
            Message::ImageSelected(image_id) => {
                // Update the selected image
                self.selected_image_id = Some(image_id);
                println!("🖼️  Selected image ID: {}", image_id);
                Task::none()
            }
        }
    }

    /// Build the user interface
    fn view(&self) -> Element<Message> {
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
            text("RAW Editor v0.0.6 - Grid & Selection")
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
        let thumbnail_grid = self.images.iter().fold(
            Wrap::new().spacing(8.0).line_spacing(8.0),
            |wrap, img| {
                // Check if file is deleted
                let is_deleted = img.file_status == "deleted";
                
                // Create thumbnail button - simplified to let Rust infer types
                let thumbnail_widget = if is_deleted {
                    // Show deleted file indicator
                    button(
                        column![
                            text("❌").size(24),
                            text(&img.filename).size(8),
                            text("(deleted)").size(7),
                        ]
                        .align_x(Alignment::Center)
                        .width(128)
                        .height(128)
                        .padding(5)
                    )
                    .on_press(Message::ImageSelected(img.id))
                } else if let Some(ref thumb_path) = img.thumbnail_path {
                    let handle = Handle::from_path(thumb_path.clone());
                    button(
                        Image::new(handle)
                            .width(128)
                            .height(128)
                    )
                    .on_press(Message::ImageSelected(img.id))
                } else {
                    // Show placeholder for pending thumbnails
                    button(
                        text("⏳").size(48)
                    )
                    .width(128)
                    .height(128)
                    .on_press(Message::ImageSelected(img.id))
                };
                
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
        
        // ========== RIGHT PANE: Editor View ==========
        
        let editor_content = if let Some(selected_id) = self.selected_image_id {
            // Find the selected image
            if let Some(selected_img) = self.images.iter().find(|img| img.id == selected_id) {
                column![
                    text("Selected Image").size(24),
                    text("").size(10),
                    text("Filename:").size(14),
                    text(&selected_img.filename).size(16),
                    text("").size(10),
                    text("Path:").size(14),
                    text(&selected_img.path).size(12),
                    text("").size(10),
                    text(format!("Status: {}", if selected_img.file_status == "deleted" { "❌ Deleted" } else { "✅ Exists" }))
                        .size(14),
                    text("").size(10),
                    text(format!("Image ID: {}", selected_img.id)).size(12),
                ]
                .spacing(5)
                .padding(20)
            } else {
                column![
                    text("Image not found").size(18),
                ]
                .padding(20)
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

/// Async function to generate thumbnails for pending images
/// Processes images in batches to avoid blocking the UI
async fn generate_thumbnails_async(db_path: PathBuf) -> ThumbnailResult {
    let mut generated_count = 0;
    const BATCH_SIZE: usize = 20; // Process 20 images per batch for speed
    
    // Open a separate database connection for this background thread
    let conn = Connection::open(&db_path)
        .expect("Failed to open database connection for thumbnail generation");
    
    // Get pending images that need thumbnails
    // Prioritize 'pending' over 'failed' so fresh imports get processed first
    let mut stmt = conn.prepare(
        "SELECT id, path FROM images 
         WHERE cache_status IN ('pending', 'failed') 
         ORDER BY 
           CASE cache_status 
             WHEN 'pending' THEN 1 
             WHEN 'failed' THEN 2 
           END,
           id 
         LIMIT ?"
    ).expect("Failed to prepare statement");
    
    let pending_images: Vec<(i64, String)> = stmt
        .query_map([BATCH_SIZE], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("Failed to query pending images")
        .filter_map(|r| r.ok())
        .collect();
    
    println!("🔍 Processing {} images for thumbnail generation...", pending_images.len());
    
    // Generate thumbnails for each pending image
    for (image_id, raw_path_str) in pending_images {
        let raw_path = std::path::Path::new(&raw_path_str);
        
        // Try to generate thumbnail
        if let Some(thumbnail_path) = raw::thumbnail::generate_thumbnail(raw_path, image_id) {
            // Update database with thumbnail path
            let thumbnail_path_str = thumbnail_path.to_string_lossy().to_string();
            let _ = conn.execute(
                "UPDATE images SET thumbnail_path = ?1, cache_status = 'cached' WHERE id = ?2",
                rusqlite::params![thumbnail_path_str, image_id],
            );
            
            generated_count += 1;
        } else {
            // Mark as failed if thumbnail generation didn't work
            let _ = conn.execute(
                "UPDATE images SET cache_status = 'failed' WHERE id = ?1",
                rusqlite::params![image_id],
            );
            eprintln!("⚠️  Failed to generate thumbnail for image ID {}", image_id);
        }
    }
    
    ThumbnailResult {
        generated_count,
    }
}
