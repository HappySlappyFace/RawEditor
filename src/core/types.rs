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
    ///
    /// Blended in display space at LUT bake time; the shader is unaffected.
    /// Default 0.33: full Adobe camera-matching curves render notably more
    /// contrasty/saturated than the in-camera JPEG; ~1/3 strength was verified
    /// against the D3300's own rendering.
    /// serde default keeps old saved edits (without this field) loadable.
    /// Only meaningful while `tone_mapper` is `Camera` — every other operator
    /// replaces the profile's curve outright rather than blending with it.
    #[serde(default = "default_profile_curve")]
    pub profile_curve: f32,

    // ========== Tone Rendering ==========
    /// Which tone mapping operator turns scene-linear into display-linear.
    ///
    /// `Camera` (the default) is the calibrated path — the DCP's own
    /// ProfileToneCurve, or the built-in filmic curve for profiles that embed
    /// none. Selecting anything else replaces that stage; the profile's
    /// COLOUR rendering (ForwardMatrix + HueSatMap) stays in force either way.
    /// serde default keeps old saved edits loadable, and keeps them on the
    /// calibrated path.
    #[serde(default)]
    pub tone_mapper: crate::core::tonemap::ToneMapper,

    /// Shape of the GT (Uchimura) curve. Inert unless `tone_mapper == Gt`.
    /// serde default keeps old saved edits loadable.
    #[serde(default)]
    pub gt: crate::core::tonemap::GtParams,

    // ========== Local Adjustment Masks ==========
    /// Fixed-size mask slots; only the first `mask_count` are meaningful.
    /// Fixed array (not Vec) keeps EditParams `Copy` for the undo stack and
    /// edit clipboard. serde default keeps old saved edits loadable.
    #[serde(default)]
    pub masks: [MaskParams; MAX_MASKS],

    /// Number of active masks (0..=MAX_MASKS)
    #[serde(default)]
    pub mask_count: u32,
}

/// Maximum number of local adjustment masks per image.
/// Mirrored in the WGSL shaders' `masks: array<Mask, 8>` — keep in sync.
pub const MAX_MASKS: usize = 8;

fn default_profile_curve() -> f32 {
    0.33
}

/// One local adjustment mask. Geometry is stored in full-image oriented
/// display UV — the same normalized space as `EditParams::crop` — so masks
/// stay anchored to image content when the crop changes. Like the crop
/// rectangle, masks do not follow the straighten `rotation` slider.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Default)]
pub struct MaskParams {
    /// 0 = linear gradient, 1 = radial ellipse
    pub mask_type: u32,
    /// 1 = mask contributes to the render
    pub enabled: u32,
    /// 1 = weight is inverted (affect everything outside the shape)
    pub invert: u32,
    /// Radial only: 0..1 fraction of the radius over which the edge feathers
    /// (0 = hard edge, 1 = falloff starts at the center). Unused for linear.
    pub feather: f32,
    /// Linear: ramp start (weight 1). Radial: ellipse center.
    pub ax: f32,
    pub ay: f32,
    /// Linear: ramp end (weight 0). Radial: radii along u/v.
    pub bx: f32,
    pub by: f32,
    /// Local exposure in stops (-4.0 to +4.0)
    pub exposure: f32,
    /// Local contrast (-1.0 to +1.0), pivoted at middle grey like Step 11
    pub contrast: f32,
    /// Local saturation (-1.0 to +1.0)
    pub saturation: f32,
    /// Local warm/cool shift (-1.0 to +1.0). A gentle sRGB-linear channel
    /// tilt — real WB is uniform-wide, so this is the only per-pixel option.
    pub warmth: f32,
    /// Local highlights (-1.0 to +1.0), same units as the global slider
    pub highlights: f32,
    /// Local shadows (-1.0 to +1.0), same units as the global slider
    pub shadows: f32,
    /// Local green/magenta shift (-1.0 to +1.0), both mask types. Pairs
    /// with `warmth` (blue/amber) for full local white-balance-style
    /// control — same gentle sRGB-linear channel-tilt technique.
    /// serde default keeps masks saved before this field existed loadable.
    #[serde(default)]
    pub tint: f32,
    /// Radial only: ellipse rotation in degrees (-180.0 to +180.0).
    /// Unused for linear (a gradient's direction is already fully
    /// described by its two endpoints).
    /// serde default keeps masks saved before this field existed loadable.
    #[serde(default)]
    pub rotation: f32,
}

impl MaskParams {
    /// New enabled linear gradient: weight 1 at (ax, ay) falling to 0 at (bx, by).
    pub fn new_linear(ax: f32, ay: f32, bx: f32, by: f32) -> Self {
        Self {
            mask_type: 0,
            enabled: 1,
            ax,
            ay,
            bx,
            by,
            ..Self::default()
        }
    }

    /// New enabled radial ellipse centered at (cx, cy) with radii (rx, ry).
    pub fn new_radial(cx: f32, cy: f32, rx: f32, ry: f32) -> Self {
        Self {
            mask_type: 1,
            enabled: 1,
            feather: 0.5,
            ax: cx,
            ay: cy,
            bx: rx,
            by: ry,
            ..Self::default()
        }
    }
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
            tone_mapper: crate::core::tonemap::ToneMapper::Camera, // calibrated path
            gt: crate::core::tonemap::GtParams::default(),         // inert unless Gt selected
            masks: [MaskParams::default(); MAX_MASKS], // No local masks by default
            mask_count: 0,
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

    /// The reference state for the before/after peek: every look-affecting
    /// adjustment reset, but GEOMETRY and SENSOR CALIBRATION preserved.
    ///
    /// Resetting `crop`/`rotation` as well would reframe the image the instant
    /// the key goes down, so you would be comparing two different photographs
    /// rather than two renderings of the same one — which is useless for
    /// judging an edit. Black levels are a sensor correction, not a look, for
    /// the same reason.
    ///
    /// Change the preserved list here (and nowhere else) if you would rather
    /// see the truly untouched frame.
    pub fn before_reference(&self) -> Self {
        Self {
            crop: self.crop,
            rotation: self.rotation,
            is_cropping: self.is_cropping,
            black_offsets: self.black_offsets,
            black_phase_x: self.black_phase_x,
            black_phase_y: self.black_phase_y,
            ..Self::default()
        }
    }
}

/// Which groups of adjustments a copy/paste operation carries.
///
/// Stored ALONGSIDE the copied `EditParams` rather than being applied at copy
/// time — see [`EditClipboard`] for why that distinction matters.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyCategories {
    pub tone: bool,
    pub color: bool,
    pub white_balance: bool,
    pub noise: bool,
    pub detail: bool,
    pub geometry: bool,
    pub profile: bool,
    pub masks: bool,
    /// Per-channel black level offsets. Sensor calibration rather than a look,
    /// and camera-specific — off by default, and only offered in the UI when
    /// `debug::SHOW_SENSOR_CORRECTION` is on, matching the develop sidebar.
    pub black_levels: bool,
}

impl Default for CopyCategories {
    /// Everything that describes a *look*, which is what "copy settings"
    /// means to a photographer. Geometry is excluded because a crop is
    /// composed for one specific frame, and black levels because they are
    /// sensor calibration.
    fn default() -> Self {
        Self {
            tone: true,
            color: true,
            white_balance: true,
            noise: true,
            detail: true,
            geometry: false,
            profile: true,
            masks: false,
            black_levels: false,
        }
    }
}

impl CopyCategories {
    /// All on — the "copy everything" shortcut (Ctrl+Shift+C).
    pub fn all() -> Self {
        Self {
            tone: true,
            color: true,
            white_balance: true,
            noise: true,
            detail: true,
            geometry: true,
            profile: true,
            masks: true,
            black_levels: true,
        }
    }

    pub fn none() -> Self {
        Self {
            tone: false,
            color: false,
            white_balance: false,
            noise: false,
            detail: false,
            geometry: false,
            profile: false,
            masks: false,
            black_levels: false,
        }
    }

    /// True when a paste would do nothing — used to grey out the confirm
    /// button rather than let the user perform a no-op.
    pub fn is_empty(&self) -> bool {
        *self == Self::none()
    }
}

/// A copied set of edits: the full snapshot PLUS which categories were chosen.
///
/// Both halves are required. Filtering at copy time — storing only the
/// selected fields — would leave everything else sitting at its default, and
/// paste could not tell "the user chose not to copy exposure" apart from "the
/// user copied an exposure of 0.0". It would write those defaults over the
/// target's real values. Keeping the selection lets paste MERGE instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditClipboard {
    pub params: EditParams,
    pub categories: CopyCategories,
}

/// Copy the fields belonging to `cats` from `src` onto `dst`, leaving every
/// other field of `dst` untouched.
///
/// **The single source of truth for the category→field mapping.** A new
/// `EditParams` field must be added to exactly one category here; the
/// `apply_categories_covers_every_field` test fails loudly if one is missed.
///
/// `is_cropping` is deliberately never copied: it is a transient UI mode flag,
/// and pasting a `1` would drop the target image into crop-overlay mode.
pub fn apply_categories(dst: &mut EditParams, src: &EditParams, cats: CopyCategories) {
    if cats.tone {
        dst.exposure = src.exposure;
        dst.contrast = src.contrast;
        dst.highlights = src.highlights;
        dst.shadows = src.shadows;
        dst.whites = src.whites;
        dst.blacks = src.blacks;
    }
    if cats.color {
        dst.vibrance = src.vibrance;
        dst.saturation = src.saturation;
    }
    if cats.white_balance {
        dst.temperature = src.temperature;
        dst.tint = src.tint;
    }
    if cats.noise {
        dst.luma_noise = src.luma_noise;
        dst.color_noise = src.color_noise;
    }
    if cats.detail {
        dst.sharpening = src.sharpening;
        dst.sharpen_masking = src.sharpen_masking;
    }
    if cats.geometry {
        dst.rotation = src.rotation;
        dst.crop = src.crop;
    }
    if cats.profile {
        dst.profile_curve = src.profile_curve;
        dst.tone_mapper = src.tone_mapper;
        dst.gt = src.gt;
    }
    if cats.masks {
        // Together, always: `masks` without `mask_count` (or vice versa)
        // leaves the render evaluating slots the user never placed.
        dst.masks = src.masks;
        dst.mask_count = src.mask_count;
    }
    if cats.black_levels {
        dst.black_offsets = src.black_offsets;
        dst.black_phase_x = src.black_phase_x;
        dst.black_phase_y = src.black_phase_y;
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
        params.masks[0] = MaskParams::new_radial(0.5, 0.4, 0.25, 0.2);
        params.masks[0].exposure = -1.0;
        params.masks[1] = MaskParams::new_linear(0.0, 0.0, 0.0, 0.5);
        params.masks[1].invert = 1;
        params.mask_count = 2;

        // Serialize to JSON
        let json = params.to_json().unwrap();

        // Deserialize back
        let restored = EditParams::from_json(&json).unwrap();

        assert_eq!(params, restored);
        assert!(!restored.is_unedited());
    }

    #[test]
    fn test_legacy_json_without_masks() {
        // Saved edits from before the masking feature have no mask fields;
        // serde defaults must fill them in (empty mask list).
        let params = EditParams::default();
        let json = params.to_json().unwrap();
        let legacy: String = json
            .replace(&format!(",\"masks\":{}", serde_json::to_string(&params.masks).unwrap()), "")
            .replace(",\"mask_count\":0", "");
        assert!(!legacy.contains("masks"), "legacy JSON should lack mask fields");

        let restored = EditParams::from_json(&legacy).unwrap();
        assert_eq!(restored.mask_count, 0);
        assert!(restored.is_unedited());
    }

    /// Saved edits from before tone mapping existed must load onto the
    /// CALIBRATED path — landing anyone's back catalogue on a different look
    /// would be a silent, image-wide regression.
    #[test]
    fn test_legacy_json_without_tone_mapper_defaults_to_camera() {
        let params = EditParams::default();
        let json = params.to_json().unwrap();
        let legacy = json
            .replace(",\"tone_mapper\":\"Camera\"", "")
            .replace(
                &format!(",\"gt\":{}", serde_json::to_string(&params.gt).unwrap()),
                "",
            );
        assert!(!legacy.contains("tone_mapper"), "legacy JSON still has the field");
        assert!(!legacy.contains("\"gt\""), "legacy JSON still has the field");

        let restored = EditParams::from_json(&legacy).unwrap();
        assert_eq!(restored.tone_mapper, crate::core::tonemap::ToneMapper::Camera);
        assert_eq!(restored.gt, crate::core::tonemap::GtParams::default());
        assert!(restored.is_unedited());
    }

    #[test]
    fn test_mask_makes_edited() {
        let mut params = EditParams::default();
        assert!(params.is_unedited());
        params.masks[0] = MaskParams::new_linear(0.1, 0.1, 0.9, 0.9);
        params.mask_count = 1;
        assert!(!params.is_unedited());
    }

    // ── before/after reference ───────────────────────────────────────────────

    fn fully_edited() -> EditParams {
        let mut p = EditParams::default();
        p.exposure = 1.25;
        p.contrast = 3.5;
        p.highlights = -0.6;
        p.shadows = 0.4;
        p.whites = 1.1;
        p.blacks = 0.05;
        p.black_offsets = [1.0, 2.0, 3.0, 4.0];
        p.black_phase_x = 1;
        p.black_phase_y = 1;
        p.vibrance = 0.3;
        p.saturation = -0.2;
        p.temperature = 0.5;
        p.tint = -0.25;
        p.luma_noise = 0.4;
        p.color_noise = 0.7;
        p.sharpening = 0.8;
        p.sharpen_masking = 0.6;
        p.rotation = 12.0;
        p.crop = [0.1, 0.2, 0.5, 0.6];
        p.profile_curve = 0.9;
        p.tone_mapper = crate::core::tonemap::ToneMapper::Gt;
        p.gt = crate::core::tonemap::GtParams { contrast: 1.7, ..Default::default() };
        p.masks[0] = MaskParams::new_radial(0.4, 0.4, 0.2, 0.15);
        p.masks[0].exposure = -1.0;
        p.mask_count = 1;
        p
    }

    /// The peek must not reframe the image — otherwise you are comparing two
    /// different photographs instead of two renderings of one.
    #[test]
    fn before_reference_preserves_geometry_and_sensor_calibration() {
        let p = fully_edited();
        let b = p.before_reference();
        assert_eq!(b.crop, p.crop);
        assert_eq!(b.rotation, p.rotation);
        assert_eq!(b.is_cropping, p.is_cropping);
        assert_eq!(b.black_offsets, p.black_offsets);
        assert_eq!(b.black_phase_x, p.black_phase_x);
        assert_eq!(b.black_phase_y, p.black_phase_y);
    }

    #[test]
    fn before_reference_resets_every_look_adjustment() {
        let d = EditParams::default();
        let b = fully_edited().before_reference();
        assert_eq!(b.exposure, d.exposure);
        assert_eq!(b.contrast, d.contrast);
        assert_eq!(b.highlights, d.highlights);
        assert_eq!(b.shadows, d.shadows);
        assert_eq!(b.whites, d.whites);
        assert_eq!(b.blacks, d.blacks);
        assert_eq!(b.vibrance, d.vibrance);
        assert_eq!(b.saturation, d.saturation);
        assert_eq!(b.temperature, d.temperature);
        assert_eq!(b.tint, d.tint);
        assert_eq!(b.luma_noise, d.luma_noise);
        assert_eq!(b.color_noise, d.color_noise);
        assert_eq!(b.sharpening, d.sharpening);
        assert_eq!(b.profile_curve, d.profile_curve);
        assert_eq!(b.tone_mapper, d.tone_mapper);
        assert_eq!(b.mask_count, 0, "masks must not apply to the before view");
    }

    // ── copy categories ──────────────────────────────────────────────────────

    /// The guard that keeps this design from rotting: applying EVERY category
    /// must reproduce the source exactly. If someone adds a field to
    /// `EditParams` and forgets to assign it a category in `apply_categories`,
    /// this fails — and the field would otherwise be silently uncopyable
    /// forever.
    #[test]
    fn apply_categories_covers_every_field() {
        let src = fully_edited();
        let mut dst = EditParams::default();
        apply_categories(&mut dst, &src, CopyCategories::all());
        // is_cropping is the one deliberate exclusion (transient UI state).
        dst.is_cropping = src.is_cropping;
        assert_eq!(
            dst, src,
            "a field is missing from apply_categories' category mapping"
        );
    }

    #[test]
    fn apply_categories_with_nothing_selected_changes_nothing() {
        let src = fully_edited();
        let mut dst = EditParams::default();
        apply_categories(&mut dst, &src, CopyCategories::none());
        assert_eq!(dst, EditParams::default());
    }

    /// Each category must move its OWN fields and nothing else — the property
    /// that makes selective paste safe.
    #[test]
    fn each_category_is_isolated() {
        let src = fully_edited();
        let base = EditParams::default();

        let only = |set: fn(&mut CopyCategories)| {
            let mut c = CopyCategories::none();
            set(&mut c);
            let mut dst = base;
            apply_categories(&mut dst, &src, c);
            dst
        };

        let tone = only(|c| c.tone = true);
        assert_eq!(tone.exposure, src.exposure);
        assert_eq!(tone.contrast, src.contrast);
        // ...and nothing from a neighbouring category leaked in.
        assert_eq!(tone.saturation, base.saturation);
        assert_eq!(tone.temperature, base.temperature);
        assert_eq!(tone.crop, base.crop);

        let wb = only(|c| c.white_balance = true);
        assert_eq!(wb.temperature, src.temperature);
        assert_eq!(wb.tint, src.tint);
        assert_eq!(wb.exposure, base.exposure);

        let geo = only(|c| c.geometry = true);
        assert_eq!(geo.crop, src.crop);
        assert_eq!(geo.rotation, src.rotation);
        assert_eq!(geo.exposure, base.exposure);

        let profile = only(|c| c.profile = true);
        assert_eq!(profile.tone_mapper, src.tone_mapper);
        assert_eq!(profile.gt, src.gt);
        assert_eq!(profile.profile_curve, src.profile_curve);
        assert_eq!(profile.exposure, base.exposure);

        let black = only(|c| c.black_levels = true);
        assert_eq!(black.black_offsets, src.black_offsets);
        assert_eq!(black.black_phase_x, src.black_phase_x);
        assert_eq!(black.exposure, base.exposure);
    }

    /// `masks` and `mask_count` must travel together, or the shader evaluates
    /// slots the user never placed.
    #[test]
    fn masks_category_copies_array_and_count_together() {
        let src = fully_edited();
        let mut dst = EditParams::default();
        let mut c = CopyCategories::none();
        c.masks = true;
        apply_categories(&mut dst, &src, c);
        assert_eq!(dst.mask_count, src.mask_count);
        assert_eq!(dst.masks[0], src.masks[0]);
    }

    /// Pasting must never drop the target into crop-overlay mode.
    #[test]
    fn is_cropping_is_never_copied() {
        let mut src = fully_edited();
        src.is_cropping = 1;
        let mut dst = EditParams::default();
        apply_categories(&mut dst, &src, CopyCategories::all());
        assert_eq!(dst.is_cropping, 0);
    }

    #[test]
    fn copy_categories_default_is_a_look_not_a_frame() {
        let d = CopyCategories::default();
        assert!(d.tone && d.color && d.white_balance && d.profile);
        assert!(!d.geometry, "a crop is composed for one specific frame");
        assert!(!d.black_levels, "sensor calibration is camera-specific");
        assert!(!d.is_empty());
        assert!(CopyCategories::none().is_empty());
        assert!(!CopyCategories::all().is_empty());
    }

    #[test]
    fn copy_categories_round_trips_through_serde() {
        let mut c = CopyCategories::default();
        c.masks = true;
        c.geometry = true;
        let json = serde_json::to_string(&c).unwrap();
        let back: CopyCategories = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
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
