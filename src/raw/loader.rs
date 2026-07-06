/// RAW sensor data loader
///
/// Phase 119: Swapped rawloader → rawler (dnglab fork) for better EXIF support and modern sensor arrays.
/// EXIF metadata is still read via kamadak-exif since rawler's Exif struct is internal to its decoders.
use std::path::Path;
use tokio::task;
use std::path::PathBuf;

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
    /// True when `color_matrix` is D65-referenced (no D50→D65 Bradford adaptation needed)
    pub color_matrix_is_d65: bool,
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
    
    // Phase 140: DNG Camera Profile
    pub dcp_profile: Option<std::sync::Arc<crate::raw::dcp::DcpProfile>>,

    /// Normalised EXIF orientation: 1 (normal), 3 (180°), 6 (90° CW), 8 (270° CW).
    /// Read via kamadak-exif — rawler 0.7.2 hardcodes Orientation::Normal.
    pub orientation: u32,
}

/// Load raw sensor data from a RAW file
pub async fn load_raw_data(path: String) -> Result<RawDataResult, String> {
    task::spawn_blocking(move || load_raw_data_blocking(&path))
        .await
        .map_err(|e| format!("Task join error: {}", e))?
}

/// Blocking implementation of RAW data loading (Phase 119: uses rawler)
fn load_raw_data_blocking(path: &str) -> Result<RawDataResult, String> {
    let path = Path::new(path);

    if !path.exists() {
        return Err(format!("File not found: {}", path.display()));
    }

    // Phase 119: rawler::decode_file replaces RawLoader::new().decode_file()
    let raw_image = rawler::decode_file(path)
        .map_err(|e| format!("Failed to decode RAW: {:?}", e))?;

    // Extract raw sensor data (full buffer)
    let full_data: Vec<u16> = match &raw_image.data {
        rawler::RawImageData::Integer(values) => values.clone(),
        rawler::RawImageData::Float(values) => values
            .iter()
            .map(|&v| (v * 65535.0).clamp(0.0, 65535.0) as u16)
            .collect(),
    };

    // Phase 119: rawler uses active_area: Option<Rect> instead of crops: Vec<usize>
    // Rect has fields: p (Point: x, y) and d (Dim2: w, h)
    // We convert to [top, right, bottom, left] margins.
    let (data, width, height, crops_margins) = if let Some(area) = &raw_image.active_area {
        let full_width = raw_image.width;
        let full_height = raw_image.height;

        let left = area.p.x;
        let top = area.p.y;
        let right = full_width.saturating_sub(area.p.x + area.d.w);
        let bottom = full_height.saturating_sub(area.p.y + area.d.h);

        tracing::debug!(
            "Applying crop margins: top={}, right={}, bottom={}, left={}",
            top, right, bottom, left
        );

        let crop_width = area.d.w;
        let crop_height = area.d.h;

        if crop_width == 0 || crop_height == 0 {
            tracing::warn!("Crop resulted in empty image, using full sensor dump");
            (full_data, full_width as u32, full_height as u32, [0usize; 4])
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

            (
                cropped_data,
                crop_width as u32,
                crop_height as u32,
                [top, right, bottom, left],
            )
        }
    } else {
        tracing::warn!("No active_area found, using full sensor dump");
        (full_data, raw_image.width as u32, raw_image.height as u32, [0usize; 4])
    };

    // Phase 119: CFA is now at raw_image.camera.cfa (not raw_image.cfa directly)
    let cfa_name = raw_image.camera.cfa.name.to_uppercase();
    let mut cfa_pattern = match cfa_name.as_str() {
        "RGGB" => 0, "GRBG" => 1, "GBRG" => 2, "BGGR" => 3,
        _ => 0,
    };

    // PHASE 111: Apply CFA Shift based on crop parity
    let top_crop = crops_margins[0] as u32;
    let left_crop = crops_margins[3] as u32;
    cfa_pattern = cfa_pattern ^ ((top_crop % 2) << 1) ^ (left_crop % 2);

    // Phase 43: Compute per-CFA black levels from cropped mosaic (P0.1 percentile)
    let measured_black_levels = compute_cfa_black_levels_percentile(
        &data,
        width as usize,
        height as usize,
        cfa_pattern,
    );

    tracing::info!(
        "Loaded RAW data: {}x{} ({} pixels)",
        width, height, data.len()
    );

    // Camera metadata black level is the sensor's calibrated optical-black offset.
    // The P0.1 scene percentile is only a fallback: measuring "black" from image
    // content overestimates the black point in any scene without true black
    // (crushed shadows) and varies frame-to-frame (inconsistent tone between shots).
    // rawler's as_bayer_array() is documented RGBE order [R, G, B, G2];
    // our pipeline wants [R, G1, G2, B].
    let meta_black = raw_image.blacklevel.as_bayer_array();
    let black_levels = if meta_black.iter().any(|&v| v > 0.0) {
        tracing::info!(
            "Black levels from camera metadata: R={:.1} G={:.1} B={:.1} G2={:.1} (measured P0.1 was R={:.1} G1={:.1} G2={:.1} B={:.1})",
            meta_black[0], meta_black[1], meta_black[2], meta_black[3],
            measured_black_levels[0], measured_black_levels[1],
            measured_black_levels[2], measured_black_levels[3]
        );
        [
            meta_black[0] as u32, // R
            meta_black[1] as u32, // G1
            meta_black[3] as u32, // G2
            meta_black[2] as u32, // B
        ]
    } else {
        tracing::warn!("No metadata black level; falling back to measured P0.1 percentile");
        [
            measured_black_levels[0] as u32,
            measured_black_levels[1] as u32,
            measured_black_levels[2] as u32,
            measured_black_levels[3] as u32,
        ]
    };

    // White balance coefficients — guard ALL four channels against NaN/inf/zero
    // rawler sets NaN in wb_coeffs when no WB tag is found in the file.
    let raw_wb = raw_image.wb_coeffs;
    let wb_multipliers: [f32; 4] = [
        if raw_wb[0].is_finite() && raw_wb[0] > 0.0 { raw_wb[0] } else { 1.0 },
        if raw_wb[1].is_finite() && raw_wb[1] > 0.0 { raw_wb[1] } else { 1.0 },
        if raw_wb[2].is_finite() && raw_wb[2] > 0.0 { raw_wb[2] } else { 1.0 },
        if raw_wb[3].is_finite() && raw_wb[3] > 0.0 { raw_wb[3] } else {
            if raw_wb[1].is_finite() && raw_wb[1] > 0.0 { raw_wb[1] } else { 1.0 }
        },
    ];

    // Normalize white balance (G1 = 1.0)
    let g_ref = wb_multipliers[1].max(0.001);
    let wb_normalized = [
        wb_multipliers[0] / g_ref,
        wb_multipliers[1] / g_ref,
        wb_multipliers[2] / g_ref,
        wb_multipliers[3] / g_ref,
    ];

    // Phase 121: Extract the color matrix from rawler's preferred color_matrix HashMap.
    // The HashMap is keyed by Illuminant (D65, D50, etc.) and values are FlatColorMatrix (Vec<f32>)
    // with 9 or 12 elements (3 or 4 rows × 3 cols). Semantics: XYZ → camera (same as xyz_to_cam).
    // calculate_cam_to_srgb in color.rs will invert this correctly.
    let (xyz_to_cam_matrix, color_matrix_is_d65): ([f32; 9], bool) = {
        use rawler::imgop::xyz::Illuminant;

        // Prefer D65 (our display target — no chromatic adaptation needed),
        // fall back to any illuminant (Bradford-adapted in color.rs).
        let d65_mat = raw_image.color_matrix.get(&Illuminant::D65);
        let is_d65 = d65_mat.is_some();
        let native_mat = d65_mat.or_else(|| raw_image.color_matrix.values().next());

        if let Some(m) = native_mat {
            if m.len() >= 9 && (m[0] != 0.0 || m[4] != 0.0) {
                tracing::debug!("Found color_matrix ({}) from rawler — {} elements",
                    if is_d65 { "D65" } else { "non-D65" }, m.len());
                ([m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8]], is_d65)
            } else {
                tracing::warn!("color_matrix present but zero/short ({} elems), falling back to xyz_to_cam", m.len());
                let xyz_cam = &raw_image.xyz_to_cam;
                if xyz_cam[0][0] != 0.0 || xyz_cam[1][1] != 0.0 {
                    tracing::debug!("Using xyz_to_cam fallback");
                    ([xyz_cam[0][0], xyz_cam[0][1], xyz_cam[0][2],
                      xyz_cam[1][0], xyz_cam[1][1], xyz_cam[1][2],
                      xyz_cam[2][0], xyz_cam[2][1], xyz_cam[2][2]], false)
                } else {
                    tracing::warn!("No valid color matrix found, using identity");
                    ([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], true)
                }
            }
        } else {
            // color_matrix HashMap is empty — fall back to legacy xyz_to_cam field
            let xyz_cam = &raw_image.xyz_to_cam;
            if xyz_cam[0][0] != 0.0 || xyz_cam[1][1] != 0.0 {
                tracing::debug!("color_matrix empty, using xyz_to_cam legacy field");
                ([xyz_cam[0][0], xyz_cam[0][1], xyz_cam[0][2],
                  xyz_cam[1][0], xyz_cam[1][1], xyz_cam[1][2],
                  xyz_cam[2][0], xyz_cam[2][1], xyz_cam[2][2]], false)
            } else {
                tracing::warn!("No color matrix available at all, using identity");
                ([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], true)
            }
        }
    };

    tracing::debug!(
        "White Balance: R={:.3}, G={:.3}, B={:.3}, G2={:.3}",
        wb_normalized[0], wb_normalized[1], wb_normalized[2], wb_normalized[3]
    );
    tracing::debug!(
        "XYZ-to-CAM Matrix: [{:.3}, {:.3}, {:.3}]",
        xyz_to_cam_matrix[0], xyz_to_cam_matrix[1], xyz_to_cam_matrix[2]
    );
    tracing::debug!(
        "                     [{:.3}, {:.3}, {:.3}]",
        xyz_to_cam_matrix[3], xyz_to_cam_matrix[4], xyz_to_cam_matrix[5]
    );
    tracing::debug!(
        "                     [{:.3}, {:.3}, {:.3}]",
        xyz_to_cam_matrix[6], xyz_to_cam_matrix[7], xyz_to_cam_matrix[8]
    );
    // tracing::debug!("CFA Pattern Index: {}", cfa_pattern);

    // Phase 121: Prioritize metadata white level — it is the camera's actual clipping
    // point for this specific sensor (e.g. 3880 for many Canon sensors).
    // Only fall back to (1 << bps) - 1 if whitelevel.0 is completely absent.
    let white_level: u32 = {
        let bps = raw_image.bps as u32;
        // rawler's whitelevel.0 is a Vec<u32> per channel — use the first (R) channel
        let whitelevel_from_meta = raw_image.whitelevel.0.first().copied().filter(|&v| v > 0);
        let whitelevel_from_bps = if bps > 0 && bps <= 16 {
            Some((1u32 << bps) - 1)
        } else {
            None
        };

        // PRIORITY: from_meta > from_bps > pixel-scan fallback
        let resolved = whitelevel_from_meta
            .or(whitelevel_from_bps)
            .unwrap_or_else(|| {
                let max_v = *data.iter().max().unwrap_or(&0);
                if max_v > 8191 { 16383 } else if max_v > 2047 { 4095 } else { 1023 }
            });

        // tracing::debug!(
        //     "White Level: {} (from_meta={:?}, from_bps={:?}, bps={})",
        //     resolved, whitelevel_from_meta, whitelevel_from_bps, bps
        // );
        resolved
    };

    // tracing::debug!(
    //     "Black Levels: [{}, {}, {}, {}]",
    //     black_levels[0], black_levels[1], black_levels[2], black_levels[3]
    // );

    // Phase 119: EXIF metadata via kamadak-exif (rawler's Exif is internal to its decoders)
    let (iso, shutter_speed, aperture, lens, exif_orientation) =
        extract_metadata_kamadak(path.to_str().unwrap_or_default());
    let orientation = normalize_orientation(exif_orientation);

    // Phase 140: Load DCP profile if available
    let dcp_profile = crate::raw::dcp::find_profile_for_camera(&raw_image.make, &raw_image.model)
    .and_then(|p: PathBuf| {
        tracing::info!("Found DCP profile at {:?}", p);
        match crate::raw::dcp::parse_dcp(&p) {
            Ok(profile) => {
                tracing::info!(  // ← ADD THIS
                    "DCP parse OK: fm1={} hsm_dims={:?} hsm1_entries={:?}",
                    profile.forward_matrix_1.is_some(),
                    profile.hue_sat_dims,
                    profile.hue_sat_data_1.as_ref().map(|v| v.len())
                );
                Some(profile)
            }
            Err(e) => {
                tracing::error!("DCP parse FAILED: {}", e);  // ← AND THIS
                None
            }
        }
    })
    .map(std::sync::Arc::new);

    Ok(RawDataResult {
        data,
        width,
        height,
        wb_multipliers: wb_normalized,
        color_matrix: xyz_to_cam_matrix,
        color_matrix_is_d65,
        cfa_pattern,
        black_levels,
        white_level,
        crops: crops_margins,
        // Phase 119: cfa name is at camera.cfa.name
        cfa_name: raw_image.camera.cfa.name.clone(),
        measured_black_levels,
        make: raw_image.make.clone(),
        model: raw_image.model.clone(),
        iso,
        shutter_speed,
        aperture,
        lens,
        dcp_profile,
        orientation,
    })
}

/// Phase 119: EXIF extraction via kamadak-exif only (rexif removed)
fn extract_metadata_kamadak(path: &str) -> (String, String, String, String, u32) {
    let mut iso = "---".to_string();
    let mut shutter = "---".to_string();
    let mut aperture = "---".to_string();
    let mut lens = "---".to_string();
    let mut orientation: u32 = 1;

    if let Ok(file) = std::fs::File::open(path) {
        let mut bufreader = std::io::BufReader::new(&file);
        let reader = exif::Reader::new();

        if let Ok(exif_data) = reader.read_from_container(&mut bufreader) {
            if let Some(field) = exif_data.get_field(exif::Tag::PhotographicSensitivity, exif::In::PRIMARY) {
                iso = field.display_value().to_string();
            }
            if let Some(field) = exif_data.get_field(exif::Tag::ExposureTime, exif::In::PRIMARY) {
                shutter = field.display_value().to_string().replace(" s", "");
            }
            if let Some(field) = exif_data.get_field(exif::Tag::FNumber, exif::In::PRIMARY) {
                aperture = field.display_value().to_string().replace("f/", "");
            }
            if let Some(field) = exif_data.get_field(exif::Tag::LensModel, exif::In::PRIMARY) {
                lens = field.display_value().to_string().trim_matches('"').to_string();
            }
            if let Some(field) = exif_data.get_field(exif::Tag::Orientation, exif::In::PRIMARY) {
                if let Some(v) = field.value.get_uint(0) {
                    orientation = v;
                }
            }
        }
    }

    (iso, shutter, aperture, lens, orientation)
}

/// Normalise an EXIF orientation (1–8) to the subset the pipeline renders:
/// 1 (normal), 3 (180°), 6 (90° CW), 8 (270° CW). Mirrored variants map to
/// their rotation-only counterparts (mirroring is vanishingly rare in-camera).
pub fn normalize_orientation(exif_orientation: u32) -> u32 {
    match exif_orientation {
        3 | 4 => 3,
        5 | 6 => 6,
        7 | 8 => 8,
        _ => 1,
    }
}

/// Compute P0.1 percentile black level for each CFA phase from cropped mosaic
fn compute_cfa_black_levels_percentile(
    data: &[u16],
    width: usize,
    height: usize,
    cfa_pattern: u32,
) -> [f32; 4] {
    tracing::debug!(
        "Computing per-CFA black levels using P0.1 percentile on {}x{} cropped mosaic",
        width, height
    );

    const MAX_VALUE: usize = 65536;
    let mut phase_histograms: [Vec<u32>; 4] = [
        vec![0; MAX_VALUE],
        vec![0; MAX_VALUE],
        vec![0; MAX_VALUE],
        vec![0; MAX_VALUE],
    ];
    let mut phase_counts: [usize; 4] = [0, 0, 0, 0];

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

    let mut p01_values = [0.0f32; 4];
    let mut p1_values = [0.0f32; 4];
    let mut p5_values = [0.0f32; 4];
    let mut min_values = [0usize; 4];
    let mut median_values = [0usize; 4];

    for phase in 0..4 {
        let count = phase_counts[phase];
        if count == 0 {
            tracing::warn!("Phase {} has no pixels!", phase);
            continue;
        }

        let p01_threshold = (count as f64 * 0.001) as usize;
        let p1_threshold = (count as f64 * 0.01) as usize;
        let p5_threshold = (count as f64 * 0.05) as usize;
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
            if found_median { break; }
        }

        tracing::debug!(
            "Phase {}: Min={}, P0.1={:.1}, P1={:.1}, P5={:.1}, Median={} (N={})",
            phase, min_values[phase], p01_values[phase], p1_values[phase],
            p5_values[phase], median_values[phase], count
        );
    }

    // Map phase indices to [R, G1, G2, B] order based on CFA pattern
    let mut ordered_blacks = [0.0f32; 4];
    match cfa_pattern {
        0 => { // RGGB
            ordered_blacks[0] = p01_values[0]; ordered_blacks[1] = p01_values[1];
            ordered_blacks[2] = p01_values[2]; ordered_blacks[3] = p01_values[3];
        }
        1 => { // GRBG
            ordered_blacks[0] = p01_values[1]; ordered_blacks[1] = p01_values[0];
            ordered_blacks[2] = p01_values[3]; ordered_blacks[3] = p01_values[2];
        }
        2 => { // GBRG
            ordered_blacks[0] = p01_values[2]; ordered_blacks[1] = p01_values[0];
            ordered_blacks[2] = p01_values[3]; ordered_blacks[3] = p01_values[1];
        }
        _ => { // BGGR
            ordered_blacks[0] = p01_values[3]; ordered_blacks[1] = p01_values[1];
            ordered_blacks[2] = p01_values[2]; ordered_blacks[3] = p01_values[0];
        }
    }

    tracing::debug!(
        "Measured Black Levels (P0.1): R={:.1}, G1={:.1}, G2={:.1}, B={:.1}",
        ordered_blacks[0], ordered_blacks[1], ordered_blacks[2], ordered_blacks[3]
    );

    ordered_blacks
}
