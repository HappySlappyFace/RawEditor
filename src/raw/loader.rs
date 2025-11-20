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
    
    // Phase 40: Apply Crop (Active Area)
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
        // Ensure we don't underflow if margins are larger than image (unlikely but safe)
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
        cfa_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_load_raw_data() {
        // This test requires an actual RAW file
        // In practice, you would use a test fixture
        // For now, we just verify the function signature compiles
        let result = load_raw_data("/nonexistent/path.nef".to_string()).await;
        assert!(result.is_err());
    }
}
