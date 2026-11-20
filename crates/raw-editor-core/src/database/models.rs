/// Shared data structures for the application state.
/// These structs represent the data model that flows between
/// the database layer and the UI layer.
///
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
    /// Small render of the image as edited (`handlers::proxy`). Only set while
    /// the image has edits and the file matches them.
    pub cache_path_edited: Option<String>,
    /// File status: 'exists' or 'deleted'
    pub file_status: String,
    /// Phase 56: Star rating (0-5)
    pub rating: u8,
    /// Phase 83: Pick/Reject flag (0=None, 1=Pick, -1=Reject)
    pub flag: i8,
}

/// "Marked for removal" (soft delete) rides in the `rating` column as
/// `MARKED_BASE + the star rating it hides` (200-205), so un-marking gives the
/// stars back. Anything `>= MARKED_BASE` is marked. The ONLY writers are
/// `Library::set_marked` and its in-memory twin [`with_mark`] — never the
/// digit-key (0-5) rating shortcuts. Databases from before the stars were
/// kept hold 255 (marked, stars lost); it still reads as marked, with no
/// stars to return, and the schema setup rewrites it to 200.
pub const MARKED_BASE: u8 = 200;

/// Is this stored `rating` a mark for removal?
pub fn is_marked(rating: u8) -> bool {
    rating >= MARKED_BASE
}

/// The star rating (0-5) hidden behind a stored `rating`, marked or not.
pub fn stars(rating: u8) -> u8 {
    if is_marked(rating) {
        let s = rating - MARKED_BASE;
        if s <= 5 { s } else { 0 }
    } else {
        rating.min(5)
    }
}

/// `rating` with the mark set (keeping its stars) or cleared (restoring them).
pub fn with_mark(rating: u8, marked: bool) -> u8 {
    if marked { MARKED_BASE + stars(rating) } else { stars(rating) }
}
