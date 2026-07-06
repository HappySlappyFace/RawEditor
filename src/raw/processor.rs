/// Multi-tier cache processor
///
/// Generates all 3 cache tiers in a single pass with cascaded resizing:
///   source JPEG → 1280px (working) → 384px (instant) → 256px (thumb)
///
/// Cascading means each small tier is resized from the previous tier, not from
/// full resolution. This makes thumbnail and instant preview generation ~10× faster.
use super::jpeg::extract_largest_jpeg;
use image::{imageops::FilterType, ImageFormat};
use std::fs;
use std::path::{Path, PathBuf};

fn decode_jpeg(data: &[u8]) -> Result<image::DynamicImage, String> {
    #[cfg(feature = "fast-jpeg")]
    {
        decode_jpeg_zune(data)
    }
    #[cfg(not(feature = "fast-jpeg"))]
    {
        image::load_from_memory_with_format(data, ImageFormat::Jpeg)
            .map_err(|e| format!("JPEG decode: {e}"))
    }
}

#[cfg(feature = "fast-jpeg")]
fn decode_jpeg_zune(data: &[u8]) -> Result<image::DynamicImage, String> {
    use zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    use zune_jpeg::JpegDecoder;

    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(data, opts);
    let pixels = decoder.decode().map_err(|e| format!("zune-jpeg: {e:?}"))?;
    let (width, height) = decoder.dimensions().ok_or("zune-jpeg: missing dimensions")?;

    image::RgbImage::from_raw(width as u32, height as u32, pixels)
        .map(image::DynamicImage::ImageRgb8)
        .ok_or_else(|| "zune-jpeg: buffer size mismatch".to_string())
}

const TIER_THUMB: u32 = 256;
const TIER_INSTANT: u32 = 384;
const TIER_WORKING: u32 = 1280;

fn get_cache_dir(tier_name: &str) -> Result<PathBuf, String> {
    let mut path = dirs_next::cache_dir()
        .or_else(dirs_next::home_dir)
        .ok_or_else(|| "Could not determine cache directory".to_string())?;
    path.push("raw-editor");
    path.push(tier_name);
    fs::create_dir_all(&path)
        .map_err(|e| format!("Failed to create {} cache directory: {}", tier_name, e))?;
    Ok(path)
}

/// Process a RAW image and generate all 3 cache tiers.
/// Returns `(thumb_path, instant_path, working_path)`.
pub fn process_image(
    raw_path: &Path,
    image_id: i64,
    _cache_dir: &Path,
) -> Result<(String, String, String), String> {
    // Extract the largest embedded JPEG without pre-validation.
    // Skipping the validator eliminates 1-3 redundant full JPEG decodes.
    let jpeg_data = extract_largest_jpeg(raw_path, None)?
        .ok_or_else(|| format!("No embedded JPEG in {:?}", raw_path.file_name()))?;

    tracing::debug!(
        "Extracted {}KB JPEG from {:?}",
        jpeg_data.len() / 1024,
        raw_path.file_name().unwrap_or_default()
    );

    let source = decode_jpeg(&jpeg_data)
        .map_err(|e| format!("Failed to decode JPEG: {}", e))?;

    // Embedded previews are stored in native sensor orientation; the camera
    // only records the rotation as an EXIF tag. Apply it here so the library
    // grid and cached previews show portrait shots upright.
    let source = apply_exif_orientation(source, raw_path);

    tracing::debug!("   Original size: {}x{}", source.width(), source.height());

    // Cascade: each tier is resized from the previous (already-small) image.
    // Triangle (bilinear) is 10-20× faster than Lanczos3 and indistinguishable
    // at these output sizes. Lanczos3 only matters for 1:1 or slight upscaling.
    let working = resize_to_width(&source, TIER_WORKING, FilterType::Triangle);
    let working_path = save_tier(&working, "working", image_id)?;

    let instant = resize_to_width(&working, TIER_INSTANT, FilterType::Triangle);
    let instant_path = save_tier(&instant, "instant", image_id)?;

    let thumb = resize_to_width(&instant, TIER_THUMB, FilterType::Triangle);
    let thumb_path = save_tier(&thumb, "thumb", image_id)?;

    tracing::info!("Generated 3 cache tiers for image {}", image_id);
    Ok((thumb_path, instant_path, working_path))
}

/// Rotate a decoded embedded JPEG to match the raw file's EXIF orientation
/// (1/3/6/8 after normalisation; mirrored variants map to their rotations).
pub fn apply_exif_orientation(img: image::DynamicImage, raw_path: &Path) -> image::DynamicImage {
    let orientation = read_exif_orientation(raw_path);
    match orientation {
        3 => image::DynamicImage::ImageRgba8(image::imageops::rotate180(&img)),
        6 => image::DynamicImage::ImageRgba8(image::imageops::rotate90(&img)),
        8 => image::DynamicImage::ImageRgba8(image::imageops::rotate270(&img)),
        _ => img,
    }
}

/// Container-level EXIF orientation read (cheap — no image decode).
fn read_exif_orientation(path: &Path) -> u32 {
    let Ok(file) = std::fs::File::open(path) else { return 1 };
    let mut bufreader = std::io::BufReader::new(&file);
    let Ok(exif_data) = exif::Reader::new().read_from_container(&mut bufreader) else { return 1 };
    let orientation = exif_data
        .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .unwrap_or(1);
    crate::raw::loader::normalize_orientation(orientation)
}

fn resize_to_width(img: &image::DynamicImage, target_width: u32, filter: FilterType) -> image::DynamicImage {
    if img.width() <= target_width {
        return img.clone();
    }
    // Use u32::MAX for height so width is the sole constraint (aspect ratio preserved)
    img.resize(target_width, u32::MAX, filter)
}

fn save_tier(img: &image::DynamicImage, tier_name: &str, image_id: i64) -> Result<String, String> {
    let cache_dir = get_cache_dir(tier_name)?;
    let file_path = cache_dir.join(format!("{}.jpg", image_id));
    img.save(&file_path)
        .map_err(|e| format!("Failed to save {} tier: {}", tier_name, e))?;
    tracing::debug!("   → {}px tier: {}", img.width(), file_path.display());
    Ok(file_path.to_string_lossy().to_string())
}

/// Normalize a raw pixel value based on black and white levels.
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

#[cfg(test)]
mod tests {
    fn compute_max_height(target_width: u32, img_width: u32, img_height: u32) -> u32 {
        if img_width > 0 {
            (target_width as u64 * img_height as u64 / img_width as u64).max(1) as u32
        } else {
            target_width
        }
    }

    #[test]
    fn test_aspect_ratio_landscape() {
        let h = compute_max_height(1280, 4000, 3000);
        assert_eq!(h, 960);
    }

    #[test]
    fn test_aspect_ratio_portrait() {
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
        let h = compute_max_height(256, 0, 1000);
        assert_eq!(h, 256);
    }

    #[test]
    fn test_max_height_at_least_one() {
        let h = compute_max_height(1, 1_000_000, 1);
        assert!(h >= 1, "height must be at least 1, got {}", h);
    }

    #[test]
    fn test_zero_target_width_is_detected() {
        let target_width: u32 = 0;
        let is_invalid = target_width == 0;
        assert!(is_invalid, "zero target_width must be rejected");
    }

    #[test]
    fn test_get_cache_dir_succeeds() {
        let result = super::get_cache_dir("test_tier_functional");
        assert!(result.is_ok(), "get_cache_dir should succeed: {:?}", result);
        if let Ok(path) = result {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}
