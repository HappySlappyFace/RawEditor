// Non-destructive edit parameters for RAW images
//
// This struct stores all adjustments made to an image.
// It is serialized to JSON and stored in the database,
// enabling complete non-destructive editing with undo/redo capability.
use serde::{Deserialize, Serialize};

// All edit parameters for a RAW image
//
// These values represent adjustments that will be applied to the image
// during the rendering pipeline. All edits are non-destructive and stored
// as JSON in the database.
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
    /// Temperature adjustment (-1.0 to +1.0)
    /// - Negative = cooler (bluer)
    /// - Positive = warmer (yellower)
    /// - 0.0 = as-shot
    /// For DCP interpolation, this is converted to Kelvin at the call site.
    pub temperature: f32,

    /// Tint adjustment (-1.0 to +1.0, displayed as -100 to +100)
    /// - Negative values = more magenta
    /// - Positive values = more green
    /// - 0.0 = as-shot
    pub tint: f32,

    // ========== Noise Reduction ==========
    /// Luma (brightness) noise reduction strength (0.0 to 1.0)
    pub luma_noise: f32,

    /// Chroma (color) noise reduction strength (0.0 to 1.0)
    pub color_noise: f32,

    /// Sharpening strength (0.0 to 1.0)
    /// - 0.0 = no sharpening
    /// - 1.0 = maximum sharpening
    /// - Phase 50: Unsharp mask to enhance edge detail
    pub sharpening: f32,

    /// Sharpening mask threshold (0.0 to 1.0)
    /// - 0.0 = sharpen everything (no masking)
    /// - 1.0 = sharpen only strong edges (protect smooth areas)
    /// - Phase 51: Edge-weighted sharpening to prevent noise amplification
    pub sharpen_masking: f32,

    // ========== Geometry ==========
    /// Rotation angle in degrees (-45.0 to +45.0)
    /// - Negative = counter-clockwise
    /// - Positive = clockwise
    /// - Phase 52: Straighten horizons
    pub rotation: f32,

    // Phase 66: Crop
    /// Crop rectangle [x, y, width, height]
    /// - Normalized coordinates (0.0 to 1.0)
    /// - Defines the visible sub-rectangle of the image
    pub crop: [f32; 4],

    // Phase 135: Non-destructive crop visibility
    /// Flag to indicate if the user is currently cropping
    /// If 1, the shader will show the full image with the crop dimmed outside.
    pub is_cropping: u32,

    // ========== Color Profile ==========
    /// DCP profile tone-curve strength (0.0 to 1.0)
    /// - 1.0 = full ProfileToneCurve (profile's intended contrast)
    /// - 0.0 = no curve (flat linear rendering + sRGB gamma)
    /// Blended in display space at LUT bake time; the shader is unaffected.
    /// Default 0.33: full Adobe camera-matching curves render notably more
    /// contrasty/saturated than the in-camera JPEG; ~1/3 strength was verified
    /// against the D3300's own rendering.
    /// serde default keeps old saved edits (without this field) loadable.
    #[serde(default = "default_profile_curve")]
    pub profile_curve: f32,
}

fn default_profile_curve() -> f32 {
    0.33
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
            whites: 1.0, // Phase 16: Default white point - must match slider center (0.8..1.2)
            blacks: 0.0, // Phase 16: Default black point (no adjustment)
            black_offsets: [0.0, 0.0, 0.0, 0.0], // Phase 38: Manual black level tuning
            black_phase_x: 0, // Phase 39: Black level phase correction
            black_phase_y: 0, // Phase 39: Black level phase correction
            vibrance: 0.0,
            saturation: 0.0,
            temperature: 0.0,           // Phase 140: As-shot (maps to ~5000K for DCP)
            tint: 0.0,                  // Phase 18: Manual white balance (as-shot)
            luma_noise: 0.0,            // Phase 133: No luma noise reduction by default
            color_noise: 0.3,           // Phase 133: Default color noise reduction to eliminate speckles
            sharpening: 0.0,            // Phase 50: No sharpening by default
            sharpen_masking: 0.0,       // Phase 51: No masking by default
            rotation: 0.0,              // Phase 52: No rotation by default
            crop: [0.0, 0.0, 1.0, 1.0], // Phase 66: Full image by default
            is_cropping: 0,             // Phase 135: Not cropping by default
            profile_curve: 0.33,        // 1/3 DCP tone curve (matches in-camera JPEG contrast)
        }
    }
}

impl EditParams {
    /// Create new default edit parameters
    pub fn new() -> Self {
        Self::default()
    }

    /// Convert to JSON string for database storage
    pub fn to_json(self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self)
    }

    /// Parse from JSON string (from database)
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let mut params: Self = serde_json::from_str(json)?;
        // Phase 140: Migrate old Kelvin temperature back to -1..1 range
        if params.temperature > 100.0 {
            // Old Kelvin value stored — convert back to normalized range
            params.temperature = ((params.temperature - 5000.0) / 5000.0).clamp(-1.0, 1.0);
        }
        Ok(params)
    }

    /// Check if this represents an unedited image (all values at default)
    pub fn is_unedited(&self) -> bool {
        *self == Self::default()
    }

    /// Reset all adjustments to default (no edits)
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Convert the normalized temperature slider value (-1.0..1.0) to Kelvin
    /// for DCP dual-illuminant interpolation.
    /// Maps: -1.0 → 2000K, 0.0 → 5000K, 1.0 → 10000K
    /// Uses reciprocal interpolation (1/T) which is physically correct for
    /// color temperature blending (Planckian locus is nearly linear in 1/T space).
    pub fn temperature_to_kelvin(&self) -> f32 {
        // Reciprocal interpolation: blend in 1/T space
        let inv_low = 1.0 / 2000.0_f32;   // warm end (t = -1)
        let inv_mid = 1.0 / 5000.0_f32;   // neutral  (t =  0)
        let inv_high = 1.0 / 10000.0_f32; // cool end (t = +1)
        let t = self.temperature;
        let inv_t = if t <= 0.0 {
            inv_mid + t * (inv_mid - inv_low) // lerp inv_low..inv_mid
        } else {
            inv_mid + t * (inv_high - inv_mid) // lerp inv_mid..inv_high
        };
        (1.0 / inv_t).clamp(2000.0, 10000.0)
    }

    /// Absolute WB temperature in Kelvin, anchored at the image's as-shot CCT.
    /// Slider 0 = as-shot; ±1 = ∓120 mired (mired offsets are perceptually
    /// uniform, unlike Kelvin offsets). Positive slider → higher Kelvin →
    /// warmer render, matching the Lightroom convention.
    pub fn kelvin_from_anchor(&self, anchor_kelvin: f32) -> f32 {
        let anchor_mired = 1.0e6 / anchor_kelvin.clamp(1500.0, 50000.0);
        let mired = (anchor_mired - self.temperature * 120.0).max(20.0);
        (1.0e6 / mired).clamp(1500.0, 50000.0)
    }

    /// Absolute Adobe-scale tint, anchored at the image's as-shot tint.
    /// Slider ±1 = ±50 tint units; positive = magenta (ACR convention).
    pub fn tint_from_anchor(&self, anchor_tint: f32) -> f32 {
        anchor_tint + self.tint * 50.0
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
