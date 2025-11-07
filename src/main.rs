use iced::{Element, Task, Theme};
use iced::widget::{button, column, container, row, scrollable, text, Column};
use iced::{Alignment, Length};
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
        
        // Load all images from the database
        let images = library.get_all_images().unwrap_or_default();
        let image_count = images.len();
        
        println!("🎨 RAW Editor initialized with {} images", image_count);
        
        let status = format!("Loaded {} images.", image_count);
        
        // Get database path for background thumbnail generation
        let db_path = library.path().clone();
        
        (
            RawEditor { library, status, images },
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
        }
    }

    /// Build the user interface
    fn view(&self) -> Element<Message> {
        // Count thumbnails
        let cached_count = self.images.iter()
            .filter(|img| img.thumbnail_path.is_some())
            .count();
        let total_count = self.images.len();
        
        // Create a column of image entries with status indicators
        let image_list: Column<Message> = self.images.iter().fold(
            column![].spacing(5),
            |col, img| {
                // Show status indicator based on thumbnail availability
                let status_icon = if img.thumbnail_path.is_some() {
                    "✅" // Checkmark for cached
                } else {
                    "⏳" // Hourglass for pending
                };
                
                let row_content = row![
                    text(status_icon).size(20),
                    text(&img.filename).size(14),
                ]
                .spacing(10)
                .align_y(Alignment::Center);
                
                col.push(row_content)
            },
        );
        
        // Main content layout
        let content: Column<Message> = column![
            text("RAW Editor v0.0.5")
                .size(48),
            
            button("Import Folder")
                .on_press(Message::ImportFolder)
                .padding(10),
            
            text(&self.status)
                .size(16),
            
            text(format!("Thumbnails: {}/{}", cached_count, total_count))
                .size(14),
            
            // Scrollable list of images with thumbnails
            scrollable(image_list)
                .height(Length::Fill)
                .width(Length::Fill),
        ]
        .spacing(20)
        .padding(40)
        .align_x(Alignment::Center);

        container(content)
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
