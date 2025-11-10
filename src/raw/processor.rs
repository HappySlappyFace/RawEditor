/// Phase 28: Multi-Tier Cache Processor
/// 
/// This module generates all 3 cache tiers in a single efficient pass:
/// - Tier 1: 256px thumbnail (for grid display)
/// - Tier 2: 384px instant preview (for quick viewing)
/// - Tier 3: 1280px working preview (for editing)

use image::{imageops::FilterType, ImageFormat};
use std::fs;
use std::path::{Path, PathBuf};

/// Cache tier sizes
const TIER_THUMB: u32 = 256;    // Grid thumbnails
const TIER_INSTANT: u32 = 384;  // Quick preview
const TIER_WORKING: u32 = 1280; // Editing preview

/// Get the cache directory for a specific tier
fn get_cache_dir(tier_name: &str) -> PathBuf {
    let mut path = dirs_next::cache_dir()
        .or_else(|| dirs_next::home_dir())
        .expect("Could not determine cache directory");
    
    path.push("raw-editor");
    path.push(tier_name);
    
    // Ensure the directory exists
    fs::create_dir_all(&path)
        .expect(&format!("Failed to create {} cache directory", tier_name));
    
    path
}

/// Process a RAW image and generate all 3 cache tiers
/// 
/// Returns Ok((thumb_path, instant_path, working_path)) on success
/// Returns Err(error_message) on failure
pub fn process_image(
    raw_path: &Path,
    image_id: i64,
    _cache_dir: &Path, // Not used, we use tier-specific dirs
) -> Result<(String, String, String), String> {
    // Step 1: Extract the largest embedded JPEG from the RAW file
    let jpeg_data = extract_largest_jpeg(raw_path)
        .ok_or_else(|| format!("Failed to extract JPEG from {:?}", raw_path.file_name()))?;
    
    println!("📦 Extracted {}KB JPEG from {:?}", 
             jpeg_data.len() / 1024, 
             raw_path.file_name().unwrap_or_default());
    
    // Step 2: Decode the JPEG once
    let img = image::load_from_memory_with_format(&jpeg_data, ImageFormat::Jpeg)
        .map_err(|e| format!("Failed to decode JPEG: {}", e))?;
    
    println!("   Original size: {}x{}", img.width(), img.height());
    
    // Step 3: Generate all 3 tiers from this single JPEG
    let thumb_path = generate_tier(&img, TIER_THUMB, "thumb", image_id)?;
    let instant_path = generate_tier(&img, TIER_INSTANT, "instant", image_id)?;
    let working_path = generate_tier(&img, TIER_WORKING, "working", image_id)?;
    
    println!("✅ Generated 3 cache tiers for image {}", image_id);
    
    Ok((thumb_path, instant_path, working_path))
}

/// Generate a single cache tier by resizing and saving
fn generate_tier(
    img: &image::DynamicImage,
    target_width: u32,
    tier_name: &str,
    image_id: i64,
) -> Result<String, String> {
    // Resize maintaining aspect ratio (width-constrained)
    let resized = img.resize(target_width, target_width * 10, FilterType::Lanczos3);
    
    // Get cache directory for this tier
    let cache_dir = get_cache_dir(tier_name);
    let file_path = cache_dir.join(format!("{}.jpg", image_id));
    
    // Save with high quality
    resized.save(&file_path)
        .map_err(|e| format!("Failed to save {} tier: {}", tier_name, e))?;
    
    println!("   → {}px tier: {}", target_width, file_path.display());
    
    // Return as string (for database storage)
    Ok(file_path.to_string_lossy().to_string())
}

/// Extract the largest embedded JPEG from a RAW file
/// This searches the entire file for all JPEG markers and returns the biggest one
fn extract_largest_jpeg(raw_path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    
    // Read entire RAW file
    let mut file = std::fs::File::open(raw_path).ok()?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).ok()?;
    
    // JPEG markers
    let jpeg_start = [0xFF, 0xD8];
    let jpeg_end = [0xFF, 0xD9];
    
    let mut all_jpegs = Vec::new();
    
    // Find all embedded JPEGs
    for (i, window) in data.windows(2).enumerate() {
        if window == jpeg_start {
            // Found JPEG start, find its end
            if let Some(end_offset) = data[i..].windows(2).position(|w| w == jpeg_end) {
                let end = i + end_offset + 1;
                let jpeg_data = data[i..=end].to_vec();
                
                // Validate it's decodable
                if image::load_from_memory_with_format(&jpeg_data, ImageFormat::Jpeg).is_ok() {
                    all_jpegs.push((jpeg_data.len(), jpeg_data));
                }
            }
        }
    }
    
    // Return the largest valid JPEG
    all_jpegs.sort_by(|a, b| b.0.cmp(&a.0)); // Sort descending by size
    all_jpegs.into_iter().next().map(|(_, data)| data)
}
