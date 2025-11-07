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
}

// Implement Debug for better error messages
impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("db_path", &self.db_path)
            .finish()
    }
}
