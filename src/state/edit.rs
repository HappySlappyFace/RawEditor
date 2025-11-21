/// Non-destructive edit parameters for RAW images
/// 
/// This struct stores all adjustments made to an image.
/// It is serialized to JSON and stored in the database,
/// enabling complete non-destructive editing with undo/redo capability.

use serde::{Deserialize, Serialize};

/// All edit parameters for a RAW image
/// 
/// These values represent adjustments that will be applied to the image
/// during the rendering pipeline. All edits are non-destructive and stored
/// as JSON in the database.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct EditParams {
    // ========== Exposure & Tone ==========
    
    /// Exposure adjustment in stops (-5.0 to +5.0)
    /// - Negative values darken the image
    /// - Positive values brighten the image
    /// - 0.0 = no adjustment
    pub exposure: f32,
    
    /// Contrast adjustment (-100.0 to +100.0)
    /// - Negative values reduce contrast (flatten)
    /// - Positive values increase contrast (boost midtones)
    /// - 0.0 = no adjustment
    pub contrast: f32,
    
    /// Highlights adjustment (-100.0 to +100.0)
    /// - Negative values recover blown highlights
    /// - Positive values boost bright areas
    /// - 0.0 = no adjustment
    pub highlights: f32,
    
    /// Shadows adjustment (-100.0 to +100.0)
    /// - Negative values darken shadows
    /// - Positive values lift/recover shadows
    /// - 0.0 = no adjustment
    pub shadows: f32,
    
    /// Whites adjustment (-100.0 to +100.0)
    /// - Adjusts the white point
    /// - 0.0 = no adjustment
    pub whites: f32,
    
    /// Blacks adjustment (-100.0 to +100.0)
    /// - Adjusts the black point
    /// - 0.0 = no adjustment
    pub blacks: f32,
    
    /// Manual black level offsets per channel [R, G1, G2, B]
    /// - Range: -50.0 to +50.0
    /// - Used to fix incorrect black levels in metadata
    pub black_offsets: [f32; 4],
    
    /// Black level grid phase shift X (0 or 1)
    pub black_phase_x: u32,
    
    /// Black level grid phase shift Y (0 or 1)
    pub black_phase_y: u32,
    
    // ========== Color ==========
    
    /// Vibrance adjustment (-100.0 to +100.0)
    /// - Smart saturation that protects skin tones
    /// - 0.0 = no adjustment
    pub vibrance: f32,
    
    /// Saturation adjustment (-100.0 to +100.0)
    /// - Global saturation adjustment
    /// - -100.0 = grayscale, 0.0 = original, +100.0 = maximum saturation
    pub saturation: f32,
    
    // ========== White Balance ==========
    
    /// Temperature adjustment (-1.0 to +1.0, displayed as -100 to +100)
    /// - Negative values = cooler (more blue)
    /// - Positive values = warmer (more yellow/orange)
    /// - 0.0 = as-shot white balance
    pub temperature: f32,
    
    /// Tint adjustment (-1.0 to +1.0, displayed as -100 to +100)
    /// - Negative values = more magenta
    /// - Positive values = more green
    /// - 0.0 = as-shot
    pub tint: f32,
    
    // ========== Noise Reduction ==========
    
    /// Noise reduction strength (0.0 to 1.0)
    /// - 0.0 = no noise reduction
    /// - 1.0 = maximum noise reduction
    /// - Phase 49: Chroma denoising to eliminate color speckles
    pub noise_reduction: f32,
    
    /// Sharpening strength (0.0 to 1.0)
    /// - 0.0 = no sharpening
    /// - 1.0 = maximum sharpening
    /// - Phase 50: Unsharp mask to enhance edge detail
    pub sharpening: f32,
}

impl Default for EditParams {
    /// Create default edit parameters (no adjustments)
    fn default() -> Self {
        Self {
            // All defaults are "no adjustment"
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 1.0,   // Phase 16: Default white point (no adjustment)
            blacks: 0.0,   // Phase 16: Default black point (no adjustment)
            black_offsets: [0.0, 0.0, 0.0, 0.0], // Phase 38: Manual black level tuning
            black_phase_x: 0, // Phase 39: Black level phase correction
            black_phase_y: 0, // Phase 39: Black level phase correction
            vibrance: 0.0,
            saturation: 0.0,
            temperature: 0.0,  // Phase 18: Manual white balance (as-shot)
            tint: 0.0,         // Phase 18: Manual white balance (as-shot)
            noise_reduction: 0.0,  // Phase 49: No noise reduction by default
            sharpening: 0.0,       // Phase 50: No sharpening by default
        }
    }
}

impl EditParams {
    /// Create new default edit parameters
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Convert to JSON string for database storage
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
    
    /// Parse from JSON string (from database)
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
    
    /// Check if this represents an unedited image (all values at default)
    pub fn is_unedited(&self) -> bool {
        *self == Self::default()
    }
    
    /// Reset all adjustments to default (no edits)
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_default_is_unedited() {
        let params = EditParams::default();
        assert!(params.is_unedited());
    }
    
    #[test]
    fn test_serialization() {
        let mut params = EditParams::default();
        params.exposure = 1.5;
        params.contrast = 20.0;
        params.saturation = -10.0;
        
        // Serialize to JSON
        let json = params.to_json().unwrap();
        
        // Deserialize back
        let restored = EditParams::from_json(&json).unwrap();
        
        assert_eq!(params, restored);
        assert!(!restored.is_unedited());
    }
    
    #[test]
    fn test_reset() {
        let mut params = EditParams::default();
        params.exposure = 2.0;
        params.contrast = 50.0;
        
        assert!(!params.is_unedited());
        
        params.reset();
        
        assert!(params.is_unedited());
    }
}
