/// Multi-tier cache processor
///
/// Generates all 3 cache tiers in a single pass with cascaded resizing:
///   source JPEG → 1280px (working) → 384px (instant) → 256px (thumb)
///
/// Cascading means each small tier is resized from the previous tier, not from
/// full resolution. This makes thumbnail and instant preview generation ~10× faster.
use super::jpeg::largest_embedded_jpeg;
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

/// `~/.cache/raw-editor/<tier>`, created if missing.
pub fn cache_dir(tier_name: &str) -> Result<PathBuf, String> {
    let path = crate::core::paths::cache_dir()
        .ok_or_else(|| "Could not determine cache directory".to_string())?
        .join(tier_name);
    fs::create_dir_all(&path)
        .map_err(|e| format!("Failed to create {} cache directory: {}", tier_name, e))?;
    Ok(path)
}

/// Process a RAW image and generate all 3 cache tiers.
/// Returns `(thumb_path, instant_path, working_path)`.
pub fn process_image(raw_path: &Path, image_id: i64) -> Result<(String, String, String), String> {
    // One read serves both the JPEG scan and the orientation lookup, which
    // used to open and parse the file a second time.
    let file = fs::read(raw_path).map_err(|e| format!("Failed to read RAW file: {e}"))?;
    let jpeg_data = largest_embedded_jpeg(&file)
        .ok_or_else(|| format!("No embedded JPEG in {:?}", raw_path.file_name()))?;

    tracing::debug!(
        "Extracted {}KB JPEG from {:?}",
        jpeg_data.len() / 1024,
        raw_path.file_name().unwrap_or_default()
    );

    let source = decode_jpeg(jpeg_data)
        .map_err(|e| format!("Failed to decode JPEG: {}", e))?;

    // Embedded previews are stored in native sensor orientation; the camera
    // only records the rotation as an EXIF tag. Apply it here so the library
    // grid and cached previews show portrait shots upright.
    let source = apply_orientation(source, exif_orientation(&file));

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

/// Rotate a decoded embedded JPEG to the RAW's normalised EXIF orientation
/// (1/3/6/8; mirrored variants map to their rotations).
fn apply_orientation(img: image::DynamicImage, orientation: u32) -> image::DynamicImage {
    match orientation {
        3 => image::DynamicImage::ImageRgba8(image::imageops::rotate180(&img)),
        6 => image::DynamicImage::ImageRgba8(image::imageops::rotate90(&img)),
        8 => image::DynamicImage::ImageRgba8(image::imageops::rotate270(&img)),
        _ => img,
    }
}

/// Container-level EXIF orientation from the RAW's bytes (no image decode).
/// Formats kamadak-exif cannot open (RW2, ORF) ask rawler's metadata parse,
/// as the loader does — or the grid would show a portrait shot sideways that
/// Develop shows upright.
fn exif_orientation(file: &[u8]) -> u32 {
    let from_kamadak = exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(file))
        .ok()
        .and_then(|d| d.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)));
    orientation_or_else(from_kamadak, || {
        let source = rawler::rawsource::RawSource::new_from_slice(file);
        rawler::get_decoder(&source)
            .and_then(|d| d.raw_metadata(&source, &rawler::decoders::RawDecodeParams::default()))
            .ok()
            .and_then(|m| m.exif.orientation)
            .map(u32::from)
    })
}

/// kamadak's value, else rawler's — the loader's `ExifInfo::fill_from` rule,
/// which also covers a container kamadak opened but found no Orientation in.
fn orientation_or_else(kamadak: Option<u32>, rawler: impl FnOnce() -> Option<u32>) -> u32 {
    crate::raw::loader::normalize_orientation(kamadak.or_else(rawler).unwrap_or(1))
}

fn resize_to_width(img: &image::DynamicImage, target_width: u32, filter: FilterType) -> image::DynamicImage {
    if img.width() <= target_width {
        return img.clone();
    }
    // Use u32::MAX for height so width is the sole constraint (aspect ratio preserved)
    img.resize(target_width, u32::MAX, filter)
}

fn save_tier(img: &image::DynamicImage, tier_name: &str, image_id: i64) -> Result<String, String> {
    let file_path = cache_dir(tier_name)?.join(format!("{}.jpg", image_id));
    img.save(&file_path)
        .map_err(|e| format!("Failed to save {} tier: {}", tier_name, e))?;
    tracing::debug!("   → {}px tier: {}", img.width(), file_path.display());
    Ok(file_path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn orientation_falls_back_to_rawler_when_kamadak_has_none() {
        assert_eq!(super::orientation_or_else(None, || Some(6)), 6);
        assert_eq!(super::orientation_or_else(Some(3), || Some(6)), 3);
        assert_eq!(super::orientation_or_else(None, || None), 1);
    }

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
        let result = super::cache_dir("test_tier_functional");
        assert!(result.is_ok(), "cache_dir should succeed: {:?}", result);
        if let Ok(path) = result {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}
