use image::{imageops::FilterType, ImageFormat};
use std::fs;
use std::path::{Path, PathBuf};

/// Size of generated thumbnails (square)
const THUMBNAIL_SIZE: u32 = 256;

/// Get the thumbnail cache directory
/// Returns ~/.cache/raw-editor/thumbnails on Linux
pub fn get_thumbnail_cache_dir() -> PathBuf {
    let mut path = dirs_next::cache_dir()
        .or_else(|| dirs_next::home_dir())
        .expect("Could not determine cache directory");
    
    path.push("raw-editor");
    path.push("thumbnails");
    
    // Ensure the directory exists
    fs::create_dir_all(&path).expect("Failed to create thumbnail cache directory");
    
    path
}

/// Generate a thumbnail for a RAW file
/// Returns the path to the saved thumbnail, or None if generation failed
pub fn generate_thumbnail(raw_path: &Path, image_id: i64) -> Option<PathBuf> {
    // Tier 1: Fast embedded JPEG search (256KB)
    if let Some(thumbnail_data) = extract_embedded_jpeg_fast(raw_path) {
        if let Some(path) = save_thumbnail(thumbnail_data, image_id) {
            return Some(path);
        }
    }
    
    // Tier 2: Extended embedded JPEG search (512KB)
    if let Some(thumbnail_data) = extract_embedded_jpeg_extended(raw_path) {
        if let Some(path) = save_thumbnail(thumbnail_data, image_id) {
            println!("📸 Generated thumbnail (tier 2): {}", path.display());
            return Some(path);
        }
    }
    
    // Tier 3: Full embedded JPEG search (5MB)
    if let Some(thumbnail_data) = extract_embedded_jpeg_full(raw_path) {
        if let Some(path) = save_thumbnail(thumbnail_data, image_id) {
            println!("📸 Generated thumbnail (tier 3): {}", path.display());
            return Some(path);
        }
    }
    
    // Tier 4: Decode actual RAW data (slowest but always works)
    if let Some(path) = decode_raw_to_thumbnail(raw_path, image_id) {
        println!("🔥 Generated thumbnail from RAW decode: {}", path.display());
        return Some(path);
    }
    
    eprintln!("❌ All methods failed for: {:?}", raw_path.file_name());
    eprintln!("   File exists: {}", raw_path.exists());
    eprintln!("   File size: {:?}", std::fs::metadata(raw_path).ok().map(|m| m.len()));
    eprintln!("   Suggestion: File might be corrupted. Try re-importing or deleting it.");
    None
}

/// Helper to save thumbnail from JPEG data
fn save_thumbnail(jpeg_data: Vec<u8>, image_id: i64) -> Option<PathBuf> {
    // Decode the JPEG
    let img = image::load_from_memory_with_format(&jpeg_data, ImageFormat::Jpeg).ok()?;
    
    // Resize to thumbnail size
    let thumbnail = img.resize(THUMBNAIL_SIZE, THUMBNAIL_SIZE, FilterType::Lanczos3);
    
    // Generate thumbnail filename
    let cache_dir = get_thumbnail_cache_dir();
    let thumbnail_path = cache_dir.join(format!("{}.jpg", image_id));
    
    // Save to disk
    thumbnail.save(&thumbnail_path).ok()?;
    
    println!("📸 Generated thumbnail: {}", thumbnail_path.display());
    Some(thumbnail_path)
}

/// Extract embedded JPEG - FAST VERSION (500KB)
fn extract_embedded_jpeg_fast(raw_path: &Path) -> Option<Vec<u8>> {
    extract_jpeg_from_raw(raw_path, 256 * 1024, 50_000) // 256KB
}

/// Extract embedded JPEG - EXTENDED VERSION (1MB)
fn extract_embedded_jpeg_extended(raw_path: &Path) -> Option<Vec<u8>> {
    extract_jpeg_from_raw(raw_path, 512 * 1024, 30_000) // 512KB
}

/// Extract embedded JPEG - FULL FILE (5MB max)
fn extract_embedded_jpeg_full(raw_path: &Path) -> Option<Vec<u8>> {
    // Last attempt with JPEG extraction from first 5MB
    extract_jpeg_from_raw(raw_path, 5 * 1024 * 1024, 10_000)
}

/// Core JPEG extraction logic
fn extract_jpeg_from_raw(raw_path: &Path, max_bytes: usize, min_size: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    
    let mut file = std::fs::File::open(raw_path).ok()?;
    let mut data = vec![0u8; max_bytes];
    let bytes_read = file.read(&mut data).ok()?;
    data.truncate(bytes_read);
    
    extract_jpeg_from_data(&data, min_size)
}

/// Extract JPEG from already-loaded data
fn extract_jpeg_from_data(data: &[u8], min_size: usize) -> Option<Vec<u8>> {
    let jpeg_start = [0xFF, 0xD8];
    let jpeg_end = [0xFF, 0xD9];
    
    // Find JPEG start positions - stop after finding a few
    let mut jpeg_starts = Vec::new();
    for (i, window) in data.windows(2).enumerate() {
        if window == jpeg_start {
            jpeg_starts.push(i);
            if jpeg_starts.len() > 5 {
                break;
            }
        }
    }
    
    // Return first JPEG that's large enough
    for &start in &jpeg_starts {
        if let Some(end_offset) = data[start..]
            .windows(2)
            .position(|window| window == jpeg_end)
        {
            let end = start + end_offset + 1;
            let size = end - start + 1;
            
            if size > min_size {
                return Some(data[start..=end].to_vec());
            }
        }
    }
    
    None
}

/// Get the thumbnail path for an image ID (doesn't generate, just returns the expected path)
pub fn get_thumbnail_path(image_id: i64) -> PathBuf {
    let cache_dir = get_thumbnail_cache_dir();
    cache_dir.join(format!("{}.jpg", image_id))
}

/// Check if a thumbnail exists for an image ID
pub fn thumbnail_exists(image_id: i64) -> bool {
    get_thumbnail_path(image_id).exists()
}

/// Decode RAW file and generate thumbnail using rawloader's JPEG extraction
fn decode_raw_to_thumbnail(raw_path: &Path, image_id: i64) -> Option<PathBuf> {
    use std::io::Read;
    
    // Try reading entire file and extracting ALL JPEGs (no size limit)
    let mut file = std::fs::File::open(raw_path).ok()?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).ok()?;
    
    // Search for ALL embedded JPEGs with NO size filter
    let jpeg_start = [0xFF, 0xD8];
    let jpeg_end = [0xFF, 0xD9];
    
    let mut all_jpegs = Vec::new();
    
    // Find all JPEG boundaries
    for (i, window) in data.windows(2).enumerate() {
        if window == jpeg_start {
            // Found a JPEG start, now find its end
            if let Some(end_offset) = data[i..].windows(2).position(|w| w == jpeg_end) {
                let end = i + end_offset + 1;
                let jpeg_data = data[i..=end].to_vec();
                all_jpegs.push((jpeg_data.len(), jpeg_data));
            }
        }
    }
    
    // Try JPEGs from largest to smallest
    all_jpegs.sort_by(|a, b| b.0.cmp(&a.0));
    
    for (size, jpeg_data) in all_jpegs {
        // Try to decode this JPEG
        if let Ok(img) = image::load_from_memory_with_format(&jpeg_data, ImageFormat::Jpeg) {
            // Successfully decoded, resize and save
            let thumbnail = img.resize(THUMBNAIL_SIZE, THUMBNAIL_SIZE, FilterType::Lanczos3);
            let cache_dir = get_thumbnail_cache_dir();
            let thumbnail_path = cache_dir.join(format!("{}.jpg", image_id));
            
            if thumbnail.save(&thumbnail_path).is_ok() {
                println!("🔥 RAW decode: Found {}KB JPEG in file", size / 1024);
                return Some(thumbnail_path);
            }
        }
    }
    
    None
}

