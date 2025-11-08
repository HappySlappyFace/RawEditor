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
    
    // Get dimensions
    let width = raw_image.width as u32;
    let height = raw_image.height as u32;
    
    // Extract raw sensor data
    // rawloader returns data in different formats, we need to normalize to u16
    let data: Vec<u16> = match &raw_image.data {
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
    
    // Extract camera to sRGB color matrix (3x3)
    // rawloader provides xyz_to_cam [3][4], but we need cam_to_xyz → sRGB
    // For Phase 14, use identity matrix (white balance is the main correction)
    // TODO Phase 15: Implement proper color matrix from xyz_to_cam
    let xyz_cam = &raw_image.xyz_to_cam;
    let has_matrix = xyz_cam[0][0] != 0.0 || xyz_cam[1][1] != 0.0;
    
    let color_matrix: [f32; 9] = if has_matrix {
        // xyz_to_cam exists but needs inversion + sRGB conversion
        // This is complex math, so for now use identity
        println!("⚠️  Found xyz_to_cam matrix (will implement proper conversion in Phase 15)");
        [
            1.0, 0.0, 0.0,
            0.0, 1.0, 0.0,
            0.0, 0.0, 1.0,
        ]
    } else {
        // No matrix available
        println!("⚠️  No color matrix found, using identity matrix");
        [
            1.0, 0.0, 0.0,
            0.0, 1.0, 0.0,
            0.0, 0.0, 1.0,
        ]
    };
    
    println!("🎨 White Balance: R={:.3}, G={:.3}, B={:.3}, G2={:.3}", 
        wb_normalized[0], wb_normalized[1], wb_normalized[2], wb_normalized[3]);
    println!("🎨 Color Matrix: [{:.3}, {:.3}, {:.3}]", 
        color_matrix[0], color_matrix[1], color_matrix[2]);
    println!("                [{:.3}, {:.3}, {:.3}]", 
        color_matrix[3], color_matrix[4], color_matrix[5]);
    println!("                [{:.3}, {:.3}, {:.3}]", 
        color_matrix[6], color_matrix[7], color_matrix[8]);
    
    Ok(RawDataResult {
        data,
        width,
        height,
        wb_multipliers: wb_normalized,
        color_matrix,
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
