use super::models::Image;
use crate::core::types::EditParams;
use rusqlite::{Connection, Result as SqlResult};
use std::path::PathBuf;

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
                .map_err(|e| rusqlite::Error::InvalidPath(format!("Failed to create application data directory: {}", e).into()))?;
        }

        // Open or create the database
        let conn = Connection::open(&db_path)?;

        tracing::info!("Database initialized at: {}", db_path.display());

        let mut library = Library { conn, db_path };
        library.init_schema()?;

        Ok(library)
    }

    /// Get the path where the database should be stored
    pub fn get_db_path() -> PathBuf {
        let mut path = dirs::data_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("."));

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

        // Phase 28: Multi-tier cache system
        // Add 3 cache path columns for different resolution tiers
        let _ = self.conn.execute(
            "ALTER TABLE images ADD COLUMN cache_path_thumb TEXT", // 256px
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE images ADD COLUMN cache_path_instant TEXT", // 384px
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE images ADD COLUMN cache_path_working TEXT", // 1280px
            [],
        );

        // Phase 56: Ratings & Culling
        // Add rating column for 0-5 star ratings
        let _ = self
            .conn
            .execute("ALTER TABLE images ADD COLUMN rating INTEGER DEFAULT 0", []);

        // Add file_status column for tracking deleted files
        let _ = self.conn.execute(
            "ALTER TABLE images ADD COLUMN file_status TEXT DEFAULT 'exists'",
            [],
        );

        // Phase 83: Culling Flags
        // Add flag column: 0=Unflagged, 1=Pick, -1=Reject
        let _ = self
            .conn
            .execute("ALTER TABLE images ADD COLUMN flag INTEGER DEFAULT 0", []);

        // Create index for cache_status to quickly find pending thumbnails
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_images_cache_status 
             ON images(cache_status)",
            [],
        )?;

        tracing::info!("Database schema initialized");

        Ok(())
    }

    /// Get the path to the database file
    pub fn path(&self) -> &PathBuf {
        &self.db_path
    }

    /// Get a reference to the database connection
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Get a count of images in the library
    pub fn image_count(&self) -> SqlResult<i64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM images", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Import a new image into the library
    /// Returns the new image ID
    pub fn import_image(&self, path: &str, filename: &str) -> SqlResult<i64> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
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
            "SELECT id, filename, path, cache_path_thumb, cache_path_instant, cache_path_working, COALESCE(file_status, 'exists'), COALESCE(rating, 0), COALESCE(flag, 0) 
             FROM images 
             ORDER BY imported_at DESC"
        )?;

        let image_iter = stmt.query_map([], |row| {
            Ok(Image {
                id: row.get(0)?,
                filename: row.get(1)?,
                path: row.get(2)?,
                cache_path_thumb: row.get(3)?,
                cache_path_instant: row.get(4)?,
                cache_path_working: row.get(5)?,
                file_status: row.get(6)?,
                rating: row.get(7)?,           // Phase 56
                flag: row.get(8).unwrap_or(0), // Phase 83
            })
        })?;

        let mut images = Vec::new();
        for image in image_iter {
            images.push(image?);
        }

        Ok(images)
    }

    /// Get images that need thumbnail generation (cache_status = 'pending')
    pub fn get_pending_images(&self, limit: usize) -> SqlResult<Vec<Image>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, filename, path, cache_path_thumb, cache_path_instant, cache_path_working, COALESCE(file_status, 'exists'), COALESCE(rating, 0), COALESCE(flag, 0) 
             FROM images 
             WHERE cache_status = 'pending' 
             LIMIT ?1"
        )?;

        let image_iter = stmt.query_map([limit], |row| {
            Ok(Image {
                id: row.get(0)?,
                filename: row.get(1)?,
                path: row.get(2)?,
                cache_path_thumb: row.get(3)?,
                cache_path_instant: row.get(4)?,
                cache_path_working: row.get(5)?,
                file_status: row.get(6)?,
                rating: row.get(7)?,           // Phase 56
                flag: row.get(8).unwrap_or(0), // Phase 83
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
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
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
            tracing::warn!("Reset {} missing thumbnails to pending", reset_count);
        }

        Ok(reset_count)
    }

    /// Verify that RAW files still exist on disk
    /// Mark as 'deleted' if file is missing
    pub fn verify_files(&self) -> SqlResult<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path FROM images WHERE file_status = 'exists'")?;

        let existing_images: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
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
            tracing::warn!("Marked {} missing files as deleted", deleted_count);
        }

        Ok(deleted_count)
    }

    // ========== Edit Parameters Management ==========

    /// Save edit parameters for an image to the database
    /// Creates a new edit record or updates the most recent one
    pub fn save_edit_params(&self, image_id: i64, params: &EditParams) -> SqlResult<()> {
        // Serialize params to JSON
        let json = params
            .to_json()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        // Check if an edit record already exists for this image
        let existing_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM edits WHERE image_id = ?1 ORDER BY id DESC LIMIT 1",
                [image_id],
                |row| row.get(0),
            )
            .ok();

        if let Some(edit_id) = existing_id {
            // Update existing edit
            self.conn.execute(
                "UPDATE edits SET settings_json = ?1 WHERE id = ?2",
                rusqlite::params![json, edit_id],
            )?;
        } else {
            // Create new edit
            self.conn.execute(
                "INSERT INTO edits (image_id, settings_json) VALUES (?1, ?2)",
                rusqlite::params![image_id, json],
            )?;
        }

        Ok(())
    }

    /// Load edit parameters for an image from the database
    /// Returns Default if no edits exist for this image
    pub fn load_edit_params(&self, image_id: i64) -> SqlResult<EditParams> {
        let json: String = self.conn.query_row(
            "SELECT settings_json FROM edits WHERE image_id = ?1 ORDER BY id DESC LIMIT 1",
            [image_id],
            |row| row.get(0),
        )?;

        // Parse JSON to EditParams
        EditParams::from_json(&json)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    }

    /// Check if an image has any edits applied
    pub fn has_edits(&self, image_id: i64) -> SqlResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM edits WHERE image_id = ?1",
            [image_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Delete all edits for an image (reset to unedited)
    pub fn delete_edits(&self, image_id: i64) -> SqlResult<()> {
        self.conn
            .execute("DELETE FROM edits WHERE image_id = ?1", [image_id])?;
        Ok(())
    }

    /// Phase 56: Set star rating for an image (0-5)
    pub fn set_image_rating(&self, image_id: i64, rating: u8) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE images SET rating = ?1 WHERE id = ?2",
            rusqlite::params![rating, image_id],
        )?;
        Ok(())
    }

    /// Phase 83: Set Pick/Reject flag (-1, 0, 1)
    pub fn set_image_flag(&self, image_id: i64, flag: i8) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE images SET flag = ?1 WHERE id = ?2",
            rusqlite::params![flag, image_id],
        )?;
        Ok(())
    }

    /// Phase 28: Set all 3 cache tier paths for an image
    /// Updates cache_status to 'cached' and stores paths for thumb, instant, and working tiers
    pub fn set_image_cache_paths(
        &self,
        image_id: i64,
        thumb_path: &str,
        instant_path: &str,
        working_path: &str,
    ) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE images 
             SET cache_status = 'cached',
                 cache_path_thumb = ?1,
                 cache_path_instant = ?2,
                 cache_path_working = ?3
             WHERE id = ?4",
            rusqlite::params![thumb_path, instant_path, working_path, image_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Create an in-memory Library for testing (avoids touching the real DB file)
    fn in_memory_library() -> Library {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        let mut lib = Library {
            conn,
            db_path: std::path::PathBuf::from(":memory:"),
        };
        lib.init_schema().expect("schema init");
        lib
    }

    // ── import_image stores an integer timestamp ──────────────────────────────

    #[test]
    fn test_import_image_stores_integer_timestamp() {
        let lib = in_memory_library();
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        lib.import_image("/tmp/test.nef", "test.nef").unwrap();

        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // imported_at must be an integer in the expected range
        let ts: i64 = lib
            .conn
            .query_row("SELECT imported_at FROM images WHERE filename = 'test.nef'", [], |r| r.get(0))
            .expect("row must exist");

        assert!(ts >= before, "timestamp {} must be >= before {}", ts, before);
        assert!(ts <= after, "timestamp {} must be <= after {}", ts, after);
    }

    // ── ORDER BY imported_at works correctly with integer timestamps ──────────

    #[test]
    fn test_images_ordered_by_integer_timestamp() {
        let lib = in_memory_library();

        // Insert two images with explicit timestamps to control order
        lib.conn.execute(
            "INSERT INTO images (path, filename, imported_at) VALUES ('/a.nef', 'a.nef', 1000)",
            [],
        ).unwrap();
        lib.conn.execute(
            "INSERT INTO images (path, filename, imported_at) VALUES ('/b.nef', 'b.nef', 2000)",
            [],
        ).unwrap();

        let images = lib.get_all_images().unwrap();
        assert_eq!(images.len(), 2);
        // Newest first (2000 > 1000)
        assert_eq!(images[0].filename, "b.nef");
        assert_eq!(images[1].filename, "a.nef");
    }

    // ── duplicate path is rejected ────────────────────────────────────────────

    #[test]
    fn test_import_duplicate_path_fails() {
        let lib = in_memory_library();
        lib.import_image("/dup.nef", "dup.nef").unwrap();
        let second = lib.import_image("/dup.nef", "dup.nef");
        assert!(second.is_err(), "Inserting duplicate path must fail");
    }

    // ── save / load edit params round-trips correctly ─────────────────────────

    #[test]
    fn test_edit_params_round_trip() {
        let lib = in_memory_library();
        let id = lib.import_image("/edit.nef", "edit.nef").unwrap();

        let mut params = crate::core::types::EditParams::default();
        params.exposure = 1.5;
        params.saturation = -30.0;
        params.crop = [0.1, 0.2, 0.7, 0.6];

        lib.save_edit_params(id, &params).unwrap();
        let loaded = lib.load_edit_params(id).unwrap();

        assert_eq!(params, loaded);
    }

    // ── load_edit_params returns error when no edit exists ────────────────────

    #[test]
    fn test_load_edit_params_missing_returns_err() {
        let lib = in_memory_library();
        let id = lib.import_image("/no_edit.nef", "no_edit.nef").unwrap();
        let result = lib.load_edit_params(id);
        assert!(result.is_err(), "Missing edits must return Err, not panic");
    }

    // ── save_edit_params is idempotent (updates rather than duplicate insert) ─

    #[test]
    fn test_save_edit_params_updates_existing() {
        let lib = in_memory_library();
        let id = lib.import_image("/upd.nef", "upd.nef").unwrap();

        let mut p1 = crate::core::types::EditParams::default();
        p1.exposure = 0.5;
        lib.save_edit_params(id, &p1).unwrap();

        let mut p2 = crate::core::types::EditParams::default();
        p2.exposure = 2.0;
        lib.save_edit_params(id, &p2).unwrap();

        let loaded = lib.load_edit_params(id).unwrap();
        assert_eq!(loaded.exposure, 2.0, "Second save must overwrite first");

        let count: i64 = lib
            .conn
            .query_row("SELECT COUNT(*) FROM edits WHERE image_id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "Must have exactly one edit row, not {}", count);
    }

    // ── set_image_cache_paths marks status as cached ──────────────────────────

    #[test]
    fn test_cache_paths_mark_image_as_cached() {
        let lib = in_memory_library();
        let id = lib.import_image("/cache.nef", "cache.nef").unwrap();

        lib.set_image_cache_paths(id, "/t.jpg", "/i.jpg", "/w.jpg").unwrap();

        let status: String = lib
            .conn
            .query_row("SELECT cache_status FROM images WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "cached");
    }

    // ── star rating clamped to stored value ───────────────────────────────────

    #[test]
    fn test_set_image_rating() {
        let lib = in_memory_library();
        let id = lib.import_image("/rate.nef", "rate.nef").unwrap();

        lib.set_image_rating(id, 5).unwrap();
        let images = lib.get_all_images().unwrap();
        let img = images.iter().find(|i| i.id == id).unwrap();
        assert_eq!(img.rating, 5);
    }

    // ── pick/reject flag round-trips ──────────────────────────────────────────

    #[test]
    fn test_set_image_flag() {
        let lib = in_memory_library();
        let id = lib.import_image("/flag.nef", "flag.nef").unwrap();

        lib.set_image_flag(id, 1).unwrap();
        let images = lib.get_all_images().unwrap();
        let img = images.iter().find(|i| i.id == id).unwrap();
        assert_eq!(img.flag, 1);

        lib.set_image_flag(id, -1).unwrap();
        let images = lib.get_all_images().unwrap();
        let img = images.iter().find(|i| i.id == id).unwrap();
        assert_eq!(img.flag, -1);
    }

    // ── get_pending_images respects limit ─────────────────────────────────────

    #[test]
    fn test_get_pending_images_limit() {
        let lib = in_memory_library();
        for i in 0..5 {
            lib.import_image(&format!("/img{}.nef", i), &format!("img{}.nef", i)).unwrap();
        }
        let pending = lib.get_pending_images(2).unwrap();
        assert_eq!(pending.len(), 2);
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

/// Async helper to load the database
pub async fn load_database(_path: String) -> Result<Vec<super::models::Image>, String> {
    // In a real app, we might want to pass the path, but Library::new() determines it automatically.
    // We'll just use Library::new() here.
    match Library::new() {
        Ok(lib) => match lib.get_all_images() {
            Ok(images) => Ok(images),
            Err(e) => Err(format!("Failed to load images: {}", e)),
        },
        Err(e) => Err(format!("Failed to initialize library: {}", e)),
    }
}
