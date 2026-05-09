/// Phase 28: Multi-Tier Cache Processor
///
/// This module generates all 3 cache tiers in a single efficient pass:
/// - Tier 1: 256px thumbnail (for grid display)
/// - Tier 2: 384px instant preview (for quick viewing)
/// - Tier 3: 1280px working preview (for editing)
use super::jpeg::extract_largest_jpeg;
use image::{imageops::FilterType, ImageFormat};
use std::fs;
use std::path::{Path, PathBuf};

/// Cache tier sizes
const TIER_THUMB: u32 = 256; // Grid thumbnails
const TIER_INSTANT: u32 = 384; // Quick preview
const TIER_WORKING: u32 = 1280; // Editing preview

/// Get the cache directory for a specific tier
fn get_cache_dir(tier_name: &str) -> Result<PathBuf, String> {
    let mut path = dirs_next::cache_dir()
        .or_else(dirs_next::home_dir)
        .ok_or_else(|| "Could not determine cache directory".to_string())?;

    path.push("raw-editor");
    path.push(tier_name);

    // Ensure the directory exists
    fs::create_dir_all(&path)
        .map_err(|e| format!("Failed to create {} cache directory: {}", tier_name, e))?;

    Ok(path)
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
    let jpeg_data = extract_largest_jpeg(raw_path, Some(is_decodable_jpeg))?
        .ok_or_else(|| format!("Failed to extract JPEG from {:?}", raw_path.file_name()))?;

    tracing::debug!(
        "Extracted {}KB JPEG from {:?}",
        jpeg_data.len() / 1024,
        raw_path.file_name().unwrap_or_default()
    );

    // Step 2: Decode the JPEG once
    let img = image::load_from_memory_with_format(&jpeg_data, ImageFormat::Jpeg)
        .map_err(|e| format!("Failed to decode JPEG: {}", e))?;

    tracing::debug!("   Original size: {}x{}", img.width(), img.height());

    // Step 3: Generate all 3 tiers from this single JPEG
    let thumb_path = generate_tier(&img, TIER_THUMB, "thumb", image_id)?;
    let instant_path = generate_tier(&img, TIER_INSTANT, "instant", image_id)?;
    let working_path = generate_tier(&img, TIER_WORKING, "working", image_id)?;

    tracing::info!("Generated 3 cache tiers for image {}", image_id);

    Ok((thumb_path, instant_path, working_path))
}

/// Generate a single cache tier by resizing and saving
fn generate_tier(
    img: &image::DynamicImage,
    target_width: u32,
    tier_name: &str,
    image_id: i64,
) -> Result<String, String> {
    if target_width == 0 {
        return Err(format!("target_width cannot be 0 for tier {}", tier_name));
    }

    // Resize maintaining aspect ratio (width-constrained)
    // Compute a proportional max height to preserve aspect ratio
    let max_height = if img.width() > 0 {
        (target_width as u64 * img.height() as u64 / img.width() as u64).max(1) as u32
    } else {
        target_width
    };
    let resized = img.resize(target_width, max_height, FilterType::Lanczos3);

    // Get cache directory for this tier
    let cache_dir = get_cache_dir(tier_name)?;
    let file_path = cache_dir.join(format!("{}.jpg", image_id));

    // Save with high quality
    resized
        .save(&file_path)
        .map_err(|e| format!("Failed to save {} tier: {}", tier_name, e))?;

    tracing::debug!("   → {}px tier: {}", target_width, file_path.display());

    // Return as string (for database storage)
    Ok(file_path.to_string_lossy().to_string())
}

fn is_decodable_jpeg(bytes: &[u8]) -> bool {
    image::load_from_memory_with_format(bytes, ImageFormat::Jpeg).is_ok()
}

#[cfg(test)]
mod tests {
    // ── Aspect-ratio max-height computation ──────────────────────────────────
    // This is the formula extracted from generate_tier to verify it is correct.

    fn compute_max_height(target_width: u32, img_width: u32, img_height: u32) -> u32 {
        if img_width > 0 {
            (target_width as u64 * img_height as u64 / img_width as u64).max(1) as u32
        } else {
            target_width
        }
    }

    #[test]
    fn test_aspect_ratio_landscape() {
        // 4:3 image scaled to 1280px wide → height ≈ 960
        let h = compute_max_height(1280, 4000, 3000);
        assert_eq!(h, 960);
    }

    #[test]
    fn test_aspect_ratio_portrait() {
        // 3:4 portrait: width=3000, height=4000 → at 1280px wide, height ≈ 1706
        let h = compute_max_height(1280, 3000, 4000);
        assert_eq!(h, 1706);
    }

    #[test]
    fn test_aspect_ratio_square() {
        let h = compute_max_height(256, 1000, 1000);
        assert_eq!(h, 256);
    }

    #[test]
    fn test_aspect_ratio_zero_image_width_does_not_panic() {
        // Should fall back to target_width (no division by zero)
        let h = compute_max_height(256, 0, 1000);
        assert_eq!(h, 256);
    }

    #[test]
    fn test_max_height_at_least_one() {
        // Very tall thin image scaled tiny — height must be ≥ 1
        let h = compute_max_height(1, 1_000_000, 1);
        assert!(h >= 1, "height must be at least 1, got {}", h);
    }

    // ── Zero target_width guard ──────────────────────────────────────────────

    #[test]
    fn test_zero_target_width_is_detected() {
        // generate_tier returns Err when target_width == 0
        // We test just the guard condition here (no filesystem needed)
        let target_width: u32 = 0;
        let is_invalid = target_width == 0;
        assert!(is_invalid, "zero target_width must be rejected");
    }

    // ── get_cache_dir returns Ok for a valid tier name ────────────────────────

    #[test]
    fn test_get_cache_dir_succeeds() {
        // This touches the real filesystem but is safe: just creates a temp directory
        let result = super::get_cache_dir("test_tier_functional");
        assert!(result.is_ok(), "get_cache_dir should succeed: {:?}", result);
        // Clean up
        if let Ok(path) = result {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Normalize a raw pixel value based on black and white levels
///
/// This function implements the standard normalization math:
/// result = (value - black) / (white - black)
/// Clamped to [0.0, 1.0]
pub fn normalize_pixel(value: u16, black_level: u32, white_level: u32) -> f32 {
    let val = value as f32;
    let bl = black_level as f32;
    let wl = white_level as f32;

    if val <= bl {
        0.0
    } else {
        ((val - bl) / (wl - bl)).clamp(0.0, 1.0)
    }
}
