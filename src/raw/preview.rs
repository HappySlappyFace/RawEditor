/// Full-size preview generation from RAW files
/// Extracts the largest embedded JPEG without resizing
use std::path::{Path, PathBuf};
use std::fs::{self, File};
use std::io::{Read, Write};

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
    if let Some(jpeg_data) = extract_largest_jpeg(raw_path)? {
        // Save to cache
        let preview_path = preview_cache_dir.join(format!("{}.jpg", image_id));
        
        let mut file = File::create(&preview_path)
            .map_err(|e| format!("Failed to create preview file: {}", e))?;
        
        file.write_all(&jpeg_data)
            .map_err(|e| format!("Failed to write preview: {}", e))?;
        
        println!("📸 Generated full preview: {}", preview_path.display());
        Ok(preview_path.to_string_lossy().to_string())
    } else {
        Err(format!("No embedded JPEG found in: {:?}", raw_path.file_name()))
    }
}

/// Extract the largest embedded JPEG from a RAW file
/// Returns the JPEG data without any resizing
fn extract_largest_jpeg(raw_path: &Path) -> Result<Option<Vec<u8>>, String> {
    // Read the entire file
    let mut file = File::open(raw_path)
        .map_err(|e| format!("Failed to open RAW file: {}", e))?;
    
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(|e| format!("Failed to read RAW file: {}", e))?;
    
    // Try rawloader first (extracts largest JPEG)
    if let Some(jpeg) = extract_with_rawloader(raw_path)? {
        println!("🔥 Extracted {:.1}MB JPEG using rawloader", jpeg.len() as f64 / 1024.0 / 1024.0);
        return Ok(Some(jpeg));
    }
    
    // Fallback: scan for JPEG markers
    if let Some(jpeg) = scan_for_largest_jpeg(&buffer) {
        println!("🔍 Found {:.1}MB JPEG via marker scan", jpeg.len() as f64 / 1024.0 / 1024.0);
        return Ok(Some(jpeg));
    }
    
    Ok(None)
}

/// Use rawloader to extract the embedded JPEG
fn extract_with_rawloader(raw_path: &Path) -> Result<Option<Vec<u8>>, String> {
    // rawloader's API doesn't expose thumbnails directly in 0.37
    // We'll rely on the marker scan method instead
    Ok(None)
}

/// Scan file for JPEG markers and extract the largest JPEG
fn scan_for_largest_jpeg(buffer: &[u8]) -> Option<Vec<u8>> {
    let jpeg_start = b"\xff\xd8\xff";  // JPEG Start Of Image (SOI)
    let jpeg_end = b"\xff\xd9";         // JPEG End Of Image (EOI)
    
    let mut largest_jpeg: Option<Vec<u8>> = None;
    let mut largest_size = 0;
    
    // Find all JPEG sequences
    let mut pos = 0;
    while pos < buffer.len() - 3 {
        // Look for SOI marker
        if buffer[pos..].starts_with(jpeg_start) {
            // Find the corresponding EOI
            if let Some(end_pos) = buffer[pos..].windows(2)
                .position(|w| w == jpeg_end)
                .map(|p| pos + p + 2) {
                
                let jpeg_data = buffer[pos..end_pos].to_vec();
                let size = jpeg_data.len();
                
                // Keep track of the largest JPEG found
                if size > largest_size {
                    largest_size = size;
                    largest_jpeg = Some(jpeg_data);
                }
                
                pos = end_pos;
            } else {
                pos += 1;
            }
        } else {
            pos += 1;
        }
    }
    
    largest_jpeg
}

/// Get the cache directory for preview JPEGs
pub fn get_preview_cache_dir() -> PathBuf {
    let mut path = dirs::cache_dir()
        .or_else(|| dirs::home_dir())
        .expect("Could not determine cache directory");
    
    path.push("raw-editor");
    path.push("previews");
    
    // Create directory if it doesn't exist
    if !path.exists() {
        fs::create_dir_all(&path)
            .expect("Failed to create preview cache directory");
    }
    
    path
}
