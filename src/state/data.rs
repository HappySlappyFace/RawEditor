/// Shared data structures for the application state
/// 
/// These structs represent the data model that flows between
/// the database layer and the UI layer.

/// Represents a single image in the library
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// Unique database ID
    pub id: i64,
    /// Filename only (e.g., "DSC_0001.NEF")
    pub filename: String,
    /// Full path to the RAW file
    pub path: String,
    /// Path to the cached thumbnail (None if not yet generated)
    pub thumbnail_path: Option<String>,
    /// Path to the full-size preview JPEG (None if not yet generated)
    pub preview_path: Option<String>,
    /// File status: 'exists' or 'deleted'
    pub file_status: String,
}
