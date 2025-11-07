use rusqlite::{Connection, Result as SqlResult};
use std::path::PathBuf;
use super::data::Image;

/// The Library manages the SQLite catalog database.
/// It stores image metadata, edit history, and references to RAW files.
pub struct Library {
    conn: Connection,
    db_path: PathBuf,
}

impl Library {
    /// Create a new Library instance and initialize the database.
    /// 
    /// The database file is created in the user's data directory:
    /// - Linux: ~/.local/share/raw-editor/raw_editor.db
    /// - macOS: ~/Library/Application Support/raw-editor/raw_editor.db
    /// - Windows: %APPDATA%\raw-editor\raw_editor.db
    pub fn new() -> SqlResult<Self> {
        let db_path = Self::get_db_path();
        
        // Ensure the parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .expect("Failed to create application data directory");
        }

        // Open or create the database
        let conn = Connection::open(&db_path)?;
        
        println!("📁 Database initialized at: {}", db_path.display());
        
        let mut library = Library { conn, db_path };
        library.init_schema()?;
        
        Ok(library)
    }

    /// Get the path where the database should be stored
    fn get_db_path() -> PathBuf {
        let mut path = dirs::data_dir()
            .or_else(|| dirs::home_dir())
            .expect("Could not determine user data directory");
        
        path.push("raw-editor");
        path.push("raw_editor.db");
        path
    }

    /// Initialize the database schema.
    /// Creates all necessary tables and indexes if they don't exist.
    fn init_schema(&mut self) -> SqlResult<()> {
        // Create images table
        // This stores metadata about imported RAW files
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS images (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                path            TEXT NOT NULL UNIQUE,
                filename        TEXT NOT NULL,
                width           INTEGER,
                height          INTEGER,
                imported_at     INTEGER NOT NULL,
                cache_status    TEXT DEFAULT 'pending'
            )",
            [],
        )?;

        // Create edits table
        // This stores the edit stack for each image as JSON
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS edits (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                image_id        INTEGER NOT NULL,
                settings_json   TEXT NOT NULL,
                FOREIGN KEY(image_id) REFERENCES images(id) ON DELETE CASCADE
            )",
            [],
        )?;

        // Create indexes for fast queries
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_images_imported_at 
             ON images(imported_at DESC)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_edits_image_id 
             ON edits(image_id)",
            [],
        )?;

        // Add thumbnail_path column if it doesn't exist (for existing databases)
        // This is safe - if the column exists, the ALTER will be silently ignored
        let _ = self.conn.execute(
            "ALTER TABLE images ADD COLUMN thumbnail_path TEXT",
            [],
        );

        // Add file_status column for tracking deleted files
        let _ = self.conn.execute(
            "ALTER TABLE images ADD COLUMN file_status TEXT DEFAULT 'exists'",
            [],
        );

        // Add preview_path column for full-size preview JPEGs
        let _ = self.conn.execute(
            "ALTER TABLE images ADD COLUMN preview_path TEXT",
            [],
        );

        // Create index for cache_status to quickly find pending thumbnails
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_images_cache_status 
             ON images(cache_status)",
            [],
        )?;

        println!("✅ Database schema initialized");
        
        Ok(())
    }

    /// Get the path to the database file
    pub fn path(&self) -> &PathBuf {
        &self.db_path
    }

    /// Get a count of images in the library
    pub fn image_count(&self) -> SqlResult<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM images",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Import a new image into the library
    /// Returns the new image ID
    pub fn import_image(&self, path: &str, filename: &str) -> SqlResult<i64> {
        use std::time::{SystemTime, UNIX_EPOCH};
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.conn.execute(
            "INSERT INTO images (path, filename, imported_at) VALUES (?1, ?2, ?3)",
            [path, filename, &now.to_string()],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get all images from the library
    /// Returns a vector of Image structs ordered by import date (newest first)
    pub fn get_all_images(&self) -> SqlResult<Vec<Image>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, filename, path, thumbnail_path, preview_path, COALESCE(file_status, 'exists') FROM images ORDER BY imported_at DESC"
        )?;

        let image_iter = stmt.query_map([], |row| {
            Ok(Image {
                id: row.get(0)?,
                filename: row.get(1)?,
                path: row.get(2)?,
                thumbnail_path: row.get(3)?,
                preview_path: row.get(4)?,
                file_status: row.get(5)?,
            })
        })?;

        let mut images = Vec::new();
        for image in image_iter {
            images.push(image?);
        }

        Ok(images)
    }

    /// Get images that need thumbnail generation (cache_status = 'pending')
    pub fn get_pending_thumbnails(&self, limit: usize) -> SqlResult<Vec<Image>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, filename, path, thumbnail_path, preview_path, COALESCE(file_status, 'exists') 
             FROM images 
             WHERE cache_status = 'pending' 
             LIMIT ?1"
        )?;

        let image_iter = stmt.query_map([limit], |row| {
            Ok(Image {
                id: row.get(0)?,
                filename: row.get(1)?,
                path: row.get(2)?,
                thumbnail_path: row.get(3)?,
                preview_path: row.get(4)?,
                file_status: row.get(5)?,
            })
        })?;

        let mut images = Vec::new();
        for image in image_iter {
            images.push(image?);
        }

        Ok(images)
    }

    /// Update an image's thumbnail path and mark it as cached
    pub fn update_thumbnail(&self, image_id: i64, thumbnail_path: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE images SET thumbnail_path = ?1, cache_status = 'cached' WHERE id = ?2",
            rusqlite::params![thumbnail_path, image_id],
        )?;
        Ok(())
    }

    /// Set an image's preview path (full-size embedded JPEG)
    pub fn set_image_preview_path(&self, image_id: i64, path: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE images SET preview_path = ?1 WHERE id = ?2",
            rusqlite::params![path, image_id],
        )?;
        Ok(())
    }

    /// Verify cached thumbnails actually exist on disk
    /// Reset to 'pending' if thumbnail file is missing
    pub fn verify_thumbnails(&self) -> SqlResult<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT id, thumbnail_path FROM images WHERE cache_status = 'cached' AND thumbnail_path IS NOT NULL"
        )?;

        let cached_images: Vec<(i64, String)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut reset_count = 0;
        for (id, thumbnail_path) in cached_images {
            // Check if file exists
            if !std::path::Path::new(&thumbnail_path).exists() {
                // Reset to pending since thumbnail is missing
                self.conn.execute(
                    "UPDATE images SET cache_status = 'pending', thumbnail_path = NULL WHERE id = ?1",
                    rusqlite::params![id],
                )?;
                reset_count += 1;
            }
        }

        if reset_count > 0 {
            println!("🔄 Reset {} missing thumbnails to pending", reset_count);
        }

        Ok(reset_count)
    }

    /// Verify that RAW files still exist on disk
    /// Mark as 'deleted' if file is missing
    pub fn verify_files(&self) -> SqlResult<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path FROM images WHERE file_status = 'exists'"
        )?;

        let existing_images: Vec<(i64, String)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut deleted_count = 0;
        for (id, file_path) in existing_images {
            // Check if file exists
            if !std::path::Path::new(&file_path).exists() {
                // Mark as deleted since file is missing
                self.conn.execute(
                    "UPDATE images SET file_status = 'deleted' WHERE id = ?1",
                    rusqlite::params![id],
                )?;
                deleted_count += 1;
            }
        }

        if deleted_count > 0 {
            println!("⚠️  Marked {} missing files as deleted", deleted_count);
        }

        Ok(deleted_count)
    }
}

// Implement Debug for better error messages
impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("db_path", &self.db_path)
            .finish()
    }
}
