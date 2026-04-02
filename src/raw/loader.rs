/// RAW sensor data loader
///
/// This module loads the actual sensor data from RAW files (not embedded JPEGs).
/// The data is returned as raw u16 values which will be processed by the GPU.
use std::path::Path;
use tokio::task;

/// Result type for RAW data loading
#[derive(Debug, Clone)]
pub struct RawDataResult {
    pub data: Vec<u16>,
    pub width: u32,
    pub height: u32,
    /// White balance multipliers [R, G, B, G2] from camera
    pub wb_multipliers: [f32; 4],
    /// Color matrix (3x3) for camera RGB to sRGB conversion
    pub color_matrix: [f32; 9],
    /// CFA Pattern (0=RGGB, 1=GRBG, 2=GBRG, 3=BGGR)
    pub cfa_pattern: u32,
    /// Black Levels (optical black) [R, G, B, G2] or similar
    pub black_levels: [u32; 4],
    /// White Level (saturation point)
    pub white_level: u32,
    /// Crop Margins [Top, Right, Bottom, Left]
    pub crops: [usize; 4],
    /// CFA Pattern Name (e.g. "RGGB")
    pub cfa_name: String,
    /// Measured Black Levels from Optical Black [f32; 4]
    pub measured_black_levels: [f32; 4],

    // Phase 60: Metadata
    pub make: String,
    pub model: String,
    pub iso: String,
    pub shutter_speed: String,
    pub aperture: String,
    pub lens: String,
}

/// Load raw sensor data from a RAW file
///
/// This function uses rawloader to extract the actual sensor data (not embedded JPEG).
/// The data is returned as a Vec<u16> of raw sensor values.
///
/// # Arguments
/// * `path` - Path to the RAW file
///
/// # Returns
/// * `Ok((data, width, height))` - Raw sensor data and dimensions
/// * `Err(String)` - Error message if loading fails
pub async fn load_raw_data(path: String) -> Result<RawDataResult, String> {
    // Spawn blocking because rawloader is CPU-intensive
    task::spawn_blocking(move || load_raw_data_blocking(&path))
        .await
        .map_err(|e| format!("Task join error: {}", e))?
}

/// Blocking implementation of RAW data loading
fn load_raw_data_blocking(path: &str) -> Result<RawDataResult, String> {
    let path = Path::new(path);

    // Verify file exists
    if !path.exists() {
        return Err(format!("File not found: {}", path.display()));
    }

    let decoder = rawloader::RawLoader::new();

    // Decode the RAW file (rawloader expects &Path)
    let raw_image = decoder
        .decode_file(path)
        .map_err(|e| format!("Failed to decode RAW: {:?}", e))?;

    // Extract raw sensor data (full buffer)
    // rawloader returns data in different formats, we need to normalize to u16
    let full_data: Vec<u16> = match &raw_image.data {
        rawloader::RawImageData::Integer(values) => {
            // Already u16, perfect!
            values.clone()
        }
        rawloader::RawImageData::Float(values) => {
            // Convert f32 (0.0-1.0) to u16 (0-65535)
            values
                .iter()
                .map(|&v| (v * 65535.0).clamp(0.0, 65535.0) as u16)
                .collect()
        }
    };

    // Phase 40: Apply Crop (Active Area) FIRST
    // rawloader.crops is [top, right, bottom, left] in pixels to be removed
    let (data, width, height) = if raw_image.crops.len() == 4 {
        let top = raw_image.crops[0];
        let right = raw_image.crops[1];
        let bottom = raw_image.crops[2];
        let left = raw_image.crops[3];

        tracing::debug!(
            "Applying crop margins: top={}, right={}, bottom={}, left={}",
            top,
            right,
            bottom,
            left
        );

        let full_width = raw_image.width;
        let full_height = raw_image.height;

        // Calculate active area dimensions
        let crop_width = full_width.saturating_sub(left + right);
        let crop_height = full_height.saturating_sub(top + bottom);

        if crop_width == 0 || crop_height == 0 {
            tracing::warn!("Crop resulted in empty image, using full sensor dump");
            (full_data, full_width as u32, full_height as u32)
        } else {
            let mut cropped_data = Vec::with_capacity(crop_width * crop_height);

            'crop: for y in 0..crop_height {
                let src_y = top + y;
                if src_y >= full_height {
                    break 'crop;
                }

                let src_start = src_y * full_width + left;
                let src_end = src_start + crop_width;

                if src_start < full_data.len() && src_end <= full_data.len() {
                    cropped_data.extend_from_slice(&full_data[src_start..src_end]);
                }
            }

            (cropped_data, crop_width as u32, crop_height as u32)
        }
    } else {
        // No crop, use full image
        tracing::warn!("No crop data found, using full sensor dump");
        (full_data, raw_image.width as u32, raw_image.height as u32)
    };

    // Extract base CFA pattern
    let cfa_name = raw_image.cfa.name.to_uppercase();
    let mut cfa_pattern = match cfa_name.as_str() {
        "RGGB" => 0, "GRBG" => 1, "GBRG" => 2, "BGGR" => 3,
        _ => 0,
    };

    // PHASE 111: Apply CFA Shift based on crop parity
    // Bit 1 (value 2) controls the Row shift (top crop). Bit 0 (value 1) controls Col shift (left crop).
    if raw_image.crops.len() == 4 {
        let top_crop = raw_image.crops[0] as u32;
        let left_crop = raw_image.crops[3] as u32;
        cfa_pattern = cfa_pattern ^ ((top_crop % 2) << 1) ^ (left_crop % 2);
    }
    
    // Phase 43: Compute per-CFA black levels from cropped mosaic (Step 3 of checklist)
    // We do this AFTER cropping to analyze the actual active image data
    let measured_black_levels = compute_cfa_black_levels_percentile(
        &data,
        width as usize,
        height as usize,
        cfa_pattern, // Pass the shifted integer, NOT the raw_image.cfa string!
    );

    tracing::info!(
        "Loaded RAW data: {}x{} ({} pixels)",
        width,
        height,
        data.len()
    );

    // Use the measured black levels for the pipeline (converted to u32)
    // This ensures they are in the correct [R, G1, G2, B] order and match the actual data
    let black_levels = [
        measured_black_levels[0] as u32,
        measured_black_levels[1] as u32,
        measured_black_levels[2] as u32,
        measured_black_levels[3] as u32,
    ];

    // Extract white balance coefficients (as-shot from camera)
    let wb_multipliers: [f32; 4] = if raw_image.wb_coeffs.len() >= 4 {
        [
            raw_image.wb_coeffs[0],
            raw_image.wb_coeffs[1],
            raw_image.wb_coeffs[2],
            raw_image.wb_coeffs[3],
        ]
    } else if raw_image.wb_coeffs.len() >= 3 {
        // Some cameras only have 3 coefficients (R, G, B)
        [
            raw_image.wb_coeffs[0],
            raw_image.wb_coeffs[1],
            raw_image.wb_coeffs[2],
            raw_image.wb_coeffs[1], // Use same G for both green pixels
        ]
    } else {
        // Fallback: neutral (no correction)
        tracing::warn!("No white balance data found, using neutral [1.0, 1.0, 1.0, 1.0]");
        [1.0, 1.0, 1.0, 1.0]
    };

    // Normalize white balance (divide by green to make green = 1.0)
    let g_ref = wb_multipliers[1].max(0.001); // Avoid division by zero
    let wb_normalized = [
        wb_multipliers[0] / g_ref,
        wb_multipliers[1] / g_ref,
        wb_multipliers[2] / g_ref,
        if wb_multipliers[3].is_finite() && wb_multipliers[3] > 0.0 {
            wb_multipliers[3] / g_ref
        } else {
            wb_multipliers[1] / g_ref // Use same as G1 if G2 is invalid
        },
    ];

    // Extract xyz_to_cam matrix (3x3) from camera metadata
    // Phase 15: Return the actual matrix, will be converted to cam_to_srgb in main.rs
    // rawloader provides xyz_to_cam as [3][4], we only need first 3 columns
    let xyz_cam = &raw_image.xyz_to_cam;
    let has_matrix = xyz_cam[0][0] != 0.0 || xyz_cam[1][1] != 0.0;

    let xyz_to_cam_matrix: [f32; 9] = if has_matrix {
        // Extract first 3 columns (4th column is usually white point info)
        tracing::debug!("Found xyz_to_cam matrix from camera");
        [
            xyz_cam[0][0],
            xyz_cam[0][1],
            xyz_cam[0][2], // Row 0
            xyz_cam[1][0],
            xyz_cam[1][1],
            xyz_cam[1][2], // Row 1
            xyz_cam[2][0],
            xyz_cam[2][1],
            xyz_cam[2][2], // Row 2
        ]
    } else {
        // No matrix available, use identity
        tracing::warn!("No xyz_to_cam matrix found, using identity");
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
    };

    tracing::debug!(
        "White Balance: R={:.3}, G={:.3}, B={:.3}, G2={:.3}",
        wb_normalized[0],
        wb_normalized[1],
        wb_normalized[2],
        wb_normalized[3]
    );
    tracing::debug!(
        "XYZ-to-CAM Matrix: [{:.3}, {:.3}, {:.3}]",
        xyz_to_cam_matrix[0],
        xyz_to_cam_matrix[1],
        xyz_to_cam_matrix[2]
    );
    tracing::debug!(
        "                     [{:.3}, {:.3}, {:.3}]",
        xyz_to_cam_matrix[3],
        xyz_to_cam_matrix[4],
        xyz_to_cam_matrix[5]
    );
    tracing::debug!(
        "                     [{:.3}, {:.3}, {:.3}]",
        xyz_to_cam_matrix[6],
        xyz_to_cam_matrix[7],
        xyz_to_cam_matrix[8]
    );

    tracing::debug!("CFA Pattern Index: {}", cfa_pattern);

    // Extract Black and White Levels
    // Extract Black and White Levels
    // Old black level extraction logic removed in favor of measured black levels
    // which are guaranteed to be in correct [R, G1, G2, B] order.

    // CRITICAL FIX: Use bit depth-based white level, not metadata white point
    // The metadata whitelevel is often the "clipping point" for this specific exposure,
    // but we need the full sensor range for proper normalization.
    // Detect bit depth by finding the maximum value in the RAW data
    let max_value = *data.iter().max().unwrap_or(&0);
    let white_level = if max_value > 8191 {
        // 14-bit: max is 16383 (2^14 - 1)
        tracing::debug!("Detected 14-bit RAW data (max value: {})", max_value);
        16383
    } else if max_value > 2047 {
        // 12-bit: max is 4095 (2^12 - 1)
        tracing::debug!("Detected 12-bit RAW data (max value: {})", max_value);
        4095
    } else {
        // 10-bit: max is 1023 (2^10 - 1)
        tracing::debug!("Detected 10-bit RAW data (max value: {})", max_value);
        1023
    };

    tracing::debug!(
        "Black Levels: [{}, {}, {}, {}]",
        black_levels[0],
        black_levels[1],
        black_levels[2],
        black_levels[3]
    );
    tracing::debug!(
        "White Level: {} (sensor max, ignoring metadata white point {})",
        white_level,
        if !raw_image.whitelevels.is_empty() {
            raw_image.whitelevels[0]
        } else {
            0
        }
    );

    // Phase 60: Extract EXIF metadata
    let metadata = extract_metadata(path.to_str().unwrap_or_default());

    Ok(RawDataResult {
        data,
        width,
        height,
        wb_multipliers: wb_normalized,
        color_matrix: xyz_to_cam_matrix, // Return xyz_to_cam, will convert in main.rs
        cfa_pattern,
        black_levels,
        white_level,
        crops: if raw_image.crops.len() == 4 {
            [
                raw_image.crops[0],
                raw_image.crops[1],
                raw_image.crops[2],
                raw_image.crops[3],
            ]
        } else {
            [0, 0, 0, 0]
        },
        cfa_name: raw_image.cfa.name.clone(),
        measured_black_levels,

        // Phase 60: Metadata extraction
        make: raw_image.make.clone(),
        model: raw_image.model.clone(),
        iso: metadata.0,
        shutter_speed: metadata.1,
        aperture: metadata.2,
        lens: metadata.3,
    })
}

/// Extract EXIF metadata (ISO, Shutter, Aperture, Lens)
fn extract_metadata(path: &str) -> (String, String, String, String) {
    let mut iso = "---".to_string();
    let mut shutter = "---".to_string();
    let mut aperture = "---".to_string();
    let mut lens = "---".to_string();

    // Try using the 'exif' crate first (better MakerNote support)
    if let Ok(file) = std::fs::File::open(path) {
        let mut bufreader = std::io::BufReader::new(&file);
        let reader = exif::Reader::new();

        if let Ok(exif_data) = reader.read_from_container(&mut bufreader) {
            // ISO
            if let Some(field) =
                exif_data.get_field(exif::Tag::PhotographicSensitivity, exif::In::PRIMARY)
            {
                iso = field.display_value().to_string();
            }

            // Shutter Speed
            if let Some(field) = exif_data.get_field(exif::Tag::ExposureTime, exif::In::PRIMARY) {
                let val = field.display_value().to_string();
                // Remove " s" if present, but don't add "s" (HUD might add it, or we standardize)
                // Actually, let's just keep the number/fraction.
                shutter = val.replace(" s", "");
            }

            // Aperture
            if let Some(field) = exif_data.get_field(exif::Tag::FNumber, exif::In::PRIMARY) {
                let val = field.display_value().to_string();
                // Remove "f/" if present
                aperture = val.replace("f/", "");
            }

            // Lens Model
            // Search in all IFDs (including MakerNote)
            // for f in exif_data.fields() {
            //     if f.tag == exif::Tag::LensModel || f.tag.to_string().contains("Lens") {
            //          println!("Possible Lens Tag: {:?} = {}", f.tag, f.display_value());
            //          if lens == "---" || lens.is_empty() {
            //              lens = f.display_value().to_string();
            //          }
            //     }
            // }

            // Clean up quotes if present
            // lens = lens.trim_matches('"').to_string();
        }
    }

    // Fallback to rexif if 'exif' failed or returned empty values (legacy support)
    if iso == "---" || shutter == "---" || aperture == "---" || lens == "---" {
        if let Ok(exif) = rexif::parse_file(path) {
            for entry in exif.entries {
                match entry.tag {
                    rexif::ExifTag::ISOSpeedRatings => {
                        if iso == "---" {
                            iso = entry.value.to_string()
                        }
                    }
                    rexif::ExifTag::ExposureTime => {
                        if shutter == "---" {
                            match entry.value {
                                rexif::TagValue::URational(ref v) => {
                                    if let Some(r) = v.first() {
                                        if r.numerator > 0 {
                                            if r.numerator >= r.denominator {
                                                let val = r.numerator as f64 / r.denominator as f64;
                                                shutter = format!("{:.1}s", val);
                                            } else {
                                                let den = r.denominator as f64 / r.numerator as f64;
                                                shutter = format!("1/{:.0}s", den);
                                            }
                                        }
                                    }
                                }
                                _ => shutter = entry.value.to_string(),
                            }
                        }
                    }
                    rexif::ExifTag::FNumber => {
                        if aperture == "---" {
                            match entry.value {
                                rexif::TagValue::URational(ref v) => {
                                    if let Some(r) = v.first() {
                                        let val = r.numerator as f64 / r.denominator as f64;
                                        aperture = format!("{:.1}", val).replace(".0", "");
                                    }
                                }
                                _ => aperture = entry.value.to_string(),
                            }
                        }
                    }
                    rexif::ExifTag::LensModel => {
                        if lens == "---" {
                            lens = entry.value.to_string()
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    (iso, shutter, aperture, lens)
}

/// Compute P0.1 percentile black level for each CFA phase from cropped mosaic
/// This follows Step 3 of the diagnostic checklist exactly
fn compute_cfa_black_levels_percentile(
    data: &[u16],
    width: usize,
    height: usize,
    cfa_pattern: u32,
) -> [f32; 4] {
    tracing::debug!(
        "Computing per-CFA black levels using P0.1 percentile on {}x{} cropped mosaic",
        width,
        height
    );

    // Build histograms for each CFA phase (0-65535 range, but we'll use bins)
    const MAX_VALUE: usize = 65536;
    let mut phase_histograms: [Vec<u32>; 4] = [
        vec![0; MAX_VALUE],
        vec![0; MAX_VALUE],
        vec![0; MAX_VALUE],
        vec![0; MAX_VALUE],
    ];

    let mut phase_counts: [usize; 4] = [0, 0, 0, 0];

    // Build histogram for each phase
    'outer: for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if idx >= data.len() {
                break 'outer;
            }

            let value = data[idx] as usize;
            let phase_idx = ((y & 1) << 1) | (x & 1);

            if value < MAX_VALUE {
                phase_histograms[phase_idx][value] += 1;
                phase_counts[phase_idx] += 1;
            }
        }
    }

    // Compute percentiles for each phase
    let mut p01_values = [0.0; 4]; // 0.1%
    let mut p1_values = [0.0; 4]; // 1%
    let mut p5_values = [0.0; 4]; // 5%
    let mut min_values = [0; 4];
    let mut median_values = [0; 4];

    for phase in 0..4 {
        let count = phase_counts[phase];
        if count == 0 {
            tracing::warn!("Phase {} has no pixels!", phase);
            continue;
        }

        let p01_threshold = (count as f64 * 0.001) as usize; // 0.1%
        let p1_threshold = (count as f64 * 0.01) as usize; // 1%
        let p5_threshold = (count as f64 * 0.05) as usize; // 5%
        let median_threshold = count / 2;

        let mut cumsum = 0usize;
        let mut found_min = false;
        let mut found_p01 = false;
        let mut found_p1 = false;
        let mut found_p5 = false;
        let mut found_median = false;

        #[allow(clippy::needless_range_loop)]
        for value in 0..MAX_VALUE {
            cumsum += phase_histograms[phase][value] as usize;

            if !found_min && phase_histograms[phase][value] > 0 {
                min_values[phase] = value;
                found_min = true;
            }
            if !found_p01 && cumsum >= p01_threshold {
                p01_values[phase] = value as f32;
                found_p01 = true;
            }
            if !found_p1 && cumsum >= p1_threshold {
                p1_values[phase] = value as f32;
                found_p1 = true;
            }
            if !found_p5 && cumsum >= p5_threshold {
                p5_values[phase] = value as f32;
                found_p5 = true;
            }
            if !found_median && cumsum >= median_threshold {
                median_values[phase] = value;
                found_median = true;
            }

            if found_median {
                break;
            }
        }

        tracing::debug!(
            "Phase {}: Min={}, P0.1={:.1}, P1={:.1}, P5={:.1}, Median={} (N={})",
            phase,
            min_values[phase],
            p01_values[phase],
            p1_values[phase],
            p5_values[phase],
            median_values[phase],
            count
        );
    }

    // Map using the shifted integer: 0=RGGB, 1=GRBG, 2=GBRG, 3=BGGR
    let mut ordered_blacks = [0.0; 4];
    if cfa_pattern == 0 {
        ordered_blacks[0] = p01_values[0]; ordered_blacks[1] = p01_values[1];
        ordered_blacks[2] = p01_values[2]; ordered_blacks[3] = p01_values[3];
    } else if cfa_pattern == 1 {
        ordered_blacks[0] = p01_values[1]; ordered_blacks[1] = p01_values[0];
        ordered_blacks[2] = p01_values[3]; ordered_blacks[3] = p01_values[2];
    } else if cfa_pattern == 2 {
        ordered_blacks[0] = p01_values[2]; ordered_blacks[1] = p01_values[0];
        ordered_blacks[2] = p01_values[3]; ordered_blacks[3] = p01_values[1];
    } else { // 3
        ordered_blacks[0] = p01_values[3]; ordered_blacks[1] = p01_values[1];
        ordered_blacks[2] = p01_values[2]; ordered_blacks[3] = p01_values[0];
    }

    tracing::debug!(
        "Measured Black Levels (P0.1): R={:.1}, G1={:.1}, G2={:.1}, B={:.1}",
        ordered_blacks[0],
        ordered_blacks[1],
        ordered_blacks[2],
        ordered_blacks[3]
    );

    ordered_blacks
}
