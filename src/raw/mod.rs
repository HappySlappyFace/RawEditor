pub mod loader;
pub mod preview;
pub mod processor; // Phase 28: Multi-tier cache processor
#[cfg(test)]
mod tests;
/// RAW image decoding module
///
/// This module handles:
/// - Extracting embedded JPEGs from RAW files
/// - Generating thumbnails
/// - Generating full-size previews
/// - Caching thumbnails and previews to disk
/// - Loading raw sensor data for GPU processing
pub mod thumbnail;
