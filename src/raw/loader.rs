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
    task::spawn_blocking(move || {
        load_raw_data_blocking(&path)
    })
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
    
    let mut decoder = rawloader::RawLoader::new();
    
    // Decode the RAW file (rawloader expects &Path)
    let raw_image = decoder.decode_file(path)
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
            values.iter()
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
        
        println!("✂️  Applying crop margins: top={}, right={}, bottom={}, left={}", top, right, bottom, left);
        
        let full_width = raw_image.width;
        let full_height = raw_image.height;
        
        // Calculate active area dimensions
        let crop_width = full_width.saturating_sub(left + right);
        let crop_height = full_height.saturating_sub(top + bottom);
        
        if crop_width == 0 || crop_height == 0 {
            println!("⚠️  Crop resulted in empty image, using full sensor dump");
            (full_data, full_width as u32, full_height as u32)
        } else {
            let mut cropped_data = Vec::with_capacity(crop_width * crop_height);
            
            for y in 0..crop_height {
                let src_y = top + y;
                if src_y >= full_height { break; }
                
                let src_start = src_y * full_width + left;
                let src_end = src_start + crop_width;
                
                if src_end <= full_data.len() {
                    cropped_data.extend_from_slice(&full_data[src_start..src_end]);
                }
            }
            
            (cropped_data, crop_width as u32, crop_height as u32)
        }
    } else {
        // No crop, use full image
        println!("⚠️  No crop data found, using full sensor dump");
        (full_data, raw_image.width as u32, raw_image.height as u32)
    };
    
    // Phase 43: Compute per-CFA black levels from cropped mosaic (Step 3 of checklist)
    // We do this AFTER cropping to analyze the active image data
    let measured_black_levels = compute_cfa_black_levels_percentile(
        &data, 
        width as usize, 
        height as usize, 
        raw_image.cfa.clone() // Pass rawloader's CFA pattern
    );
    
    println!("📷 Loaded RAW data: {}x{} ({} pixels)", width, height, data.len());
    
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
        println!("⚠️  No white balance data found, using neutral [1.0, 1.0, 1.0, 1.0]");
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
            wb_multipliers[1] / g_ref  // Use same as G1 if G2 is invalid
        },
    ];
    
    // Extract xyz_to_cam matrix (3x3) from camera metadata
    // Phase 15: Return the actual matrix, will be converted to cam_to_srgb in main.rs
    // rawloader provides xyz_to_cam as [3][4], we only need first 3 columns
    let xyz_cam = &raw_image.xyz_to_cam;
    let has_matrix = xyz_cam[0][0] != 0.0 || xyz_cam[1][1] != 0.0;
    
    let xyz_to_cam_matrix: [f32; 9] = if has_matrix {
        // Extract first 3 columns (4th column is usually white point info)
        println!("🎨 Found xyz_to_cam matrix from camera");
        [
            xyz_cam[0][0], xyz_cam[0][1], xyz_cam[0][2],  // Row 0
            xyz_cam[1][0], xyz_cam[1][1], xyz_cam[1][2],  // Row 1
            xyz_cam[2][0], xyz_cam[2][1], xyz_cam[2][2],  // Row 2
        ]
    } else {
        // No matrix available, use identity
        println!("⚠️  No xyz_to_cam matrix found, using identity");
        [
            1.0, 0.0, 0.0,
            0.0, 1.0, 0.0,
            0.0, 0.0, 1.0,
        ]
    };
    
    println!("🎨 White Balance: R={:.3}, G={:.3}, B={:.3}, G2={:.3}", 
        wb_normalized[0], wb_normalized[1], wb_normalized[2], wb_normalized[3]);
    println!("🎨 XYZ-to-CAM Matrix: [{:.3}, {:.3}, {:.3}]", 
        xyz_to_cam_matrix[0], xyz_to_cam_matrix[1], xyz_to_cam_matrix[2]);
    println!("                     [{:.3}, {:.3}, {:.3}]", 
        xyz_to_cam_matrix[3], xyz_to_cam_matrix[4], xyz_to_cam_matrix[5]);
    println!("                     [{:.3}, {:.3}, {:.3}]", 
        xyz_to_cam_matrix[6], xyz_to_cam_matrix[7], xyz_to_cam_matrix[8]);
    
    // Extract CFA pattern
    // rawloader provides a string name like "RGGB", "GRBG", etc.
    // We map this to an integer for the GPU shader:
    // 0 = RGGB
    // 1 = GRBG
    // 2 = GBRG
    // 3 = BGGR
    let cfa_name = raw_image.cfa.name.to_uppercase();
    let cfa_pattern = match cfa_name.as_str() {
        "RGGB" => 0,
        "GRBG" => 1,
        "GBRG" => 2,
        "BGGR" => 3,
        _ => {
            println!("⚠️  Unknown CFA pattern '{}', defaulting to RGGB (0)", cfa_name);
            0
        }
    };
    
    println!("🎨 CFA Pattern: {} (Index: {})", cfa_name, cfa_pattern);
    
    // Extract Black and White Levels
    // Extract Black and White Levels
    // rawloader provides these as u16 arrays (per channel)
    let black_levels: [u32; 4] = if raw_image.blacklevels.len() >= 4 {
        [
            raw_image.blacklevels[0] as u32,
            raw_image.blacklevels[1] as u32,
            raw_image.blacklevels[2] as u32,
            raw_image.blacklevels[3] as u32,
        ]
    } else if !raw_image.blacklevels.is_empty() {
        // If fewer than 4, repeat the first one
        let val = raw_image.blacklevels[0] as u32;
        [val, val, val, val]
    } else {
        [0, 0, 0, 0]
    };
    
    let white_level = if !raw_image.whitelevels.is_empty() {
        raw_image.whitelevels[0] as u32
    } else {
        65535 // Default to full 16-bit range if unknown
    };
    
    println!("⚫ Black Levels: [{}, {}, {}, {}]", 
        black_levels[0], black_levels[1], black_levels[2], black_levels[3]);
    println!("⚪ White Level: {}", white_level);
    
    Ok(RawDataResult {
        data,
        width,
        height,
        wb_multipliers: wb_normalized,
        color_matrix: xyz_to_cam_matrix,  // Return xyz_to_cam, will convert in main.rs
        cfa_pattern,
        black_levels,
        white_level,
        crops: if raw_image.crops.len() == 4 {
            [raw_image.crops[0], raw_image.crops[1], raw_image.crops[2], raw_image.crops[3]]
        } else {
            [0, 0, 0, 0]
        },
        cfa_name: raw_image.cfa.name.clone(),
        measured_black_levels,
    })
}

/// Compute P0.1 percentile black level for each CFA phase from cropped mosaic
/// This follows Step 3 of the diagnostic checklist exactly
fn compute_cfa_black_levels_percentile(
    data: &[u16], 
    width: usize, 
    height: usize, 
    cfa: rawloader::CFA
) -> [f32; 4] {
    println!("📊 Computing per-CFA black levels using P0.1 percentile on {}x{} cropped mosaic", width, height);
    
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
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if idx >= data.len() { break; }
            
            let value = data[idx] as usize;
            let phase_idx = ((y & 1) << 1) | (x & 1);
            
            if value < MAX_VALUE {
                phase_histograms[phase_idx][value] += 1;
                phase_counts[phase_idx] += 1;
            }
        }
    }
    
    // Compute percentiles for each phase
    let mut p01_values = [0.0; 4];  // 0.1%
    let mut p1_values = [0.0; 4];   // 1%
    let mut p5_values = [0.0; 4];   // 5%
    let mut min_values = [0; 4];
    let mut median_values = [0; 4];
    
    for phase in 0..4 {
        let count = phase_counts[phase];
        if count == 0 {
            println!("⚠️  Phase {} has no pixels!", phase);
            continue;
        }
        
        let p01_threshold = (count as f64 * 0.001) as usize;  // 0.1%
        let p1_threshold = (count as f64 * 0.01) as usize;    // 1%
        let p5_threshold = (count as f64 * 0.05) as usize;    // 5%
        let median_threshold = count / 2;
        
        let mut cumsum = 0usize;
        let mut found_min = false;
        let mut found_p01 = false;
        let mut found_p1 = false;
        let mut found_p5 = false;
        let mut found_median = false;
        
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
        
        println!("Phase {}: Min={}, P0.1={:.1}, P1={:.1}, P5={:.1}, Median={} (N={})", 
            phase, min_values[phase], p01_values[phase], p1_values[phase], 
            p5_values[phase], median_values[phase], count);
    }
    
    // Map phases to CFA colors based on pattern
    // rawloader CFA pattern names: "RGGB", "GRBG", "GBRG", "BGGR"
    let pattern_name = cfa.name.as_str();
    let mut ordered_blacks = [0.0; 4];
    
    // Use P0.1 as the black level estimate
    if pattern_name == "RGGB" {
        ordered_blacks[0] = p01_values[0]; // R  (0,0)
        ordered_blacks[1] = p01_values[1]; // G1 (0,1)
        ordered_blacks[2] = p01_values[2]; // G2 (1,0)
        ordered_blacks[3] = p01_values[3]; // B  (1,1)
    } else if pattern_name == "GRBG" {
        ordered_blacks[0] = p01_values[1]; // R  (0,1)
        ordered_blacks[1] = p01_values[0]; // G1 (0,0)
        ordered_blacks[2] = p01_values[3]; // G2 (1,1)
        ordered_blacks[3] = p01_values[2]; // B  (1,0)
    } else if pattern_name == "GBRG" {
        ordered_blacks[0] = p01_values[2]; // R  (1,0)
        ordered_blacks[1] = p01_values[0]; // G1 (0,0)
        ordered_blacks[2] = p01_values[3]; // G2 (1,1)
        ordered_blacks[3] = p01_values[1]; // B  (0,1)
    } else if pattern_name == "BGGR" {
        ordered_blacks[0] = p01_values[3]; // R  (1,1)
        ordered_blacks[1] = p01_values[1]; // G1 (0,1)
        ordered_blacks[2] = p01_values[2]; // G2 (1,0)
        ordered_blacks[3] = p01_values[0]; // B  (0,0)
    } else {
        println!("⚠️  Unknown CFA pattern '{}', assuming RGGB mapping", pattern_name);
        ordered_blacks = p01_values;
    }
    
    println!("📊 Measured Black Levels (P0.1): R={:.1}, G1={:.1}, G2={:.1}, B={:.1}", 
        ordered_blacks[0], ordered_blacks[1], ordered_blacks[2], ordered_blacks[3]);
        
    ordered_blacks
}

/// Old function - kept for reference but not used
#[allow(dead_code)]
fn compute_cfa_black_levels_old(
    data: &[u16], 
    width: usize, 
    height: usize, 
    crops: &[usize], 
    cfa: rawloader::CFA
) -> [f32; 4] {
    // crops: [top, right, bottom, left]
    let top_margin = crops[0];
    let right_margin = crops[1];
    let bottom_margin = crops[2];
    let left_margin = crops[3];
    
    // We will collect pixels for each of the 4 CFA phases:
    // Phase 0: (even row, even col) -> (0,0)
    // Phase 1: (even row, odd col)  -> (0,1)
    // Phase 2: (odd row, even col)  -> (1,0)
    // Phase 3: (odd row, odd col)   -> (1,1)
    let mut phase_pixels: [Vec<u16>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    
    // Helper to add pixel to correct phase bucket
    let mut add_pixel = |x: usize, y: usize, val: u16| {
        let phase_idx = ((y & 1) << 1) | (x & 1);
        phase_pixels[phase_idx].push(val);
    };
    
    // 1. Top Margin
    for y in 0..top_margin {
        for x in 0..width {
            if y < height && x < width {
                add_pixel(x, y, data[y * width + x]);
            }
        }
    }
    
    // 2. Bottom Margin
    for y in (height - bottom_margin)..height {
        for x in 0..width {
            if y < height && x < width {
                add_pixel(x, y, data[y * width + x]);
            }
        }
    }
    
    // 3. Left Margin (excluding top/bottom corners to avoid double counting)
    for y in top_margin..(height - bottom_margin) {
        for x in 0..left_margin {
            if y < height && x < width {
                add_pixel(x, y, data[y * width + x]);
            }
        }
    }
    
    // 4. Right Margin (excluding top/bottom corners)
    for y in top_margin..(height - bottom_margin) {
        for x in (width - right_margin)..width {
            if y < height && x < width {
                add_pixel(x, y, data[y * width + x]);
            }
        }
    }
    
    // Compute median for each phase
    let mut medians = [0.0; 4];
    for i in 0..4 {
        let pixels = &mut phase_pixels[i];
        if pixels.is_empty() {
            println!("⚠️  No optical black pixels found for phase {}", i);
            continue;
        }
        
        // Sort to find median
        pixels.sort_unstable();
        let mid = pixels.len() / 2;
        medians[i] = pixels[mid] as f32;
        
        // Calculate stats for debugging
        let min = pixels[0];
        let max = pixels[pixels.len() - 1];
        let p1 = pixels[pixels.len() / 100]; // 1st percentile
        
        println!("Phase {}: Median={:.1}, Min={}, Max={}, P1={} (N={})", 
            i, medians[i], min, max, p1, pixels.len());
    }
    
    // Map phases to CFA colors based on pattern
    // We need to return [R, G1, G2, B] order for the shader
    // rawloader CFA pattern: 0=Red, 1=Green, 2=Blue
    // But we need to know the layout.
    // Let's assume standard Bayer phases for now and map them later if needed.
    // Actually, the shader expects [R, G1, G2, B] values.
    // We need to know which phase corresponds to which color.
    
    // For RGGB (Pattern 0):
    // (0,0)=R, (0,1)=G, (1,0)=G, (1,1)=B
    // So Phase 0->R, Phase 1->G1, Phase 2->G2, Phase 3->B
    
    // For GRBG (Pattern 1):
    // (0,0)=G, (0,1)=R, (1,0)=B, (1,1)=G
    // So Phase 0->G1, Phase 1->R, Phase 2->B, Phase 3->G2
    
    // For GBRG (Pattern 2):
    // (0,0)=G, (0,1)=B, (1,0)=R, (1,1)=G
    // So Phase 0->G1, Phase 1->B, Phase 2->R, Phase 3->G2
    
    // For BGGR (Pattern 3):
    // (0,0)=B, (0,1)=G, (1,0)=G, (1,1)=R
    // So Phase 0->B, Phase 1->G1, Phase 2->G2, Phase 3->R
    
    // However, our shader logic ALREADY handles the mapping from (x,y) to color index.
    // The shader expects `black_levels` to be [R, G1, G2, B].
    // So we need to map our measured phases to these color slots.
    
    let pattern_name = cfa.name.as_str();
    let mut ordered_blacks = [0.0; 4];
    
    if pattern_name == "RGGB" {
        ordered_blacks[0] = medians[0]; // R  (0,0)
        ordered_blacks[1] = medians[1]; // G1 (0,1)
        ordered_blacks[2] = medians[2]; // G2 (1,0)
        ordered_blacks[3] = medians[3]; // B  (1,1)
    } else if pattern_name == "GRBG" {
        ordered_blacks[0] = medians[1]; // R  (0,1)
        ordered_blacks[1] = medians[0]; // G1 (0,0)
        ordered_blacks[2] = medians[3]; // G2 (1,1)
        ordered_blacks[3] = medians[2]; // B  (1,0)
    } else if pattern_name == "GBRG" {
        ordered_blacks[0] = medians[2]; // R  (1,0)
        ordered_blacks[1] = medians[0]; // G1 (0,0)
        ordered_blacks[2] = medians[3]; // G2 (1,1)
        ordered_blacks[3] = medians[1]; // B  (0,1)
    } else if pattern_name == "BGGR" {
        ordered_blacks[0] = medians[3]; // R  (1,1)
        ordered_blacks[1] = medians[1]; // G1 (0,1)
        ordered_blacks[2] = medians[2]; // G2 (1,0)
        ordered_blacks[3] = medians[0]; // B  (0,0)
    } else {
        println!("⚠️  Unknown CFA pattern '{}', assuming RGGB mapping", pattern_name);
        ordered_blacks[0] = medians[0];
        ordered_blacks[1] = medians[1];
        ordered_blacks[2] = medians[2];
        ordered_blacks[3] = medians[3];
    }
    
    println!("📊 Measured Black Levels (Median): R={:.1}, G1={:.1}, G2={:.1}, B={:.1}", 
        ordered_blacks[0], ordered_blacks[1], ordered_blacks[2], ordered_blacks[3]);
        
    ordered_blacks
}
