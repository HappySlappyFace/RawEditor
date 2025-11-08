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
    
    Ok(RawDataResult {
        data,
        width,
        height,
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
