use std::fs::{self, File};
use std::io::Write;
/// Full-size preview generation from RAW files
/// Extracts the largest embedded JPEG without resizing
use std::path::{Path, PathBuf};
use super::jpeg::extract_largest_jpeg;

/// Generate a full-size preview from a RAW file
/// Returns the path to the cached preview JPEG
pub async fn generate_full_preview(
    raw_path: String,
    image_id: i64,
    preview_cache_dir: PathBuf,
) -> Result<String, String> {
    // Spawn blocking task for CPU-bound work
    tokio::task::spawn_blocking(move || {
        generate_full_preview_blocking(raw_path, image_id, preview_cache_dir)
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Blocking version of preview generation
fn generate_full_preview_blocking(
    raw_path: String,
    image_id: i64,
    preview_cache_dir: PathBuf,
) -> Result<String, String> {
    let raw_path = Path::new(&raw_path);

    // Verify file exists
    if !raw_path.exists() {
        return Err(format!("RAW file does not exist: {}", raw_path.display()));
    }

    // Try to extract the largest embedded JPEG
    if let Some(jpeg_data) = extract_largest_jpeg(raw_path, None)? {
        // Save to cache
        let preview_path = preview_cache_dir.join(format!("{}.jpg", image_id));

        let mut file = File::create(&preview_path)
            .map_err(|e| format!("Failed to create preview file: {}", e))?;

        file.write_all(&jpeg_data)
            .map_err(|e| format!("Failed to write preview: {}", e))?;

        tracing::debug!("Generated full preview: {}", preview_path.display());
        Ok(preview_path.to_string_lossy().to_string())
    } else {
        Err(format!(
            "No embedded JPEG found in: {:?}",
            raw_path.file_name()
        ))
    }
}

/// Get the cache directory for preview JPEGs
pub fn get_preview_cache_dir() -> PathBuf {
    let mut path = dirs::cache_dir()
        .or_else(dirs::home_dir)
        .expect("Could not determine cache directory");

    path.push("raw-editor");
    path.push("previews");

    // Create directory if it doesn't exist
    if !path.exists() {
        fs::create_dir_all(&path).expect("Failed to create preview cache directory");
    }

    path
}
