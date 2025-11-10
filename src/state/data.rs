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
    /// Phase 28: Path to 256px thumbnail tier (None if not yet generated)
    pub cache_path_thumb: Option<String>,
    /// Phase 28: Path to 384px instant preview tier (None if not yet generated)
    pub cache_path_instant: Option<String>,
    /// Phase 28: Path to 1280px working preview tier (None if not yet generated)
    pub cache_path_working: Option<String>,
    /// File status: 'exists' or 'deleted'
    pub file_status: String,
}
