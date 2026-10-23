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

    // ========== Presence ==========
    /// Texture (-1.0 to +1.0): medium-fine detail — skin, bark, foliage —
    /// boosted or smoothed without touching pixel-level noise or larger
    /// structure. See `gpu::local_contrast`. serde default: off, so edits
    /// saved before this existed load unchanged.
    #[serde(default)]
    pub texture: f32,

    /// Clarity (-1.0 to +1.0): mid-scale local contrast, weighted to the
    /// midtones, edge-aware so strong edges do not halo.
    #[serde(default)]
    pub clarity: f32,

    /// Dehaze (-1.0 to +1.0): removes (or adds) atmospheric haze by
    /// inverting the haze model with a dark-channel transmission estimate.
    /// See `core::haze`.
    #[serde(default)]
    pub dehaze: f32,

    // ========== White Balance ==========
    /// Temperature adjustment (-1.0 to +1.0)
    /// - Negative = cooler (bluer)
    /// - Positive = warmer (yellower)
    /// - 0.0 = as-shot
    ///
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

    /// Luma noise reduction detail retention (0.0 to 1.0). Higher keeps
    /// more fine texture: it narrows how far a non-local-means patch match may
    /// differ from the centre before it stops contributing.
    /// serde default keeps edits saved before this field existed loadable.
    #[serde(default = "default_luma_detail")]
    pub luma_detail: f32,

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
    /// Straighten angle in degrees (-45.0 to +45.0). Positive turns the
    /// picture counter-clockwise: the colour pass samples at `R(θ)·out`
    /// (`core::viewport::straighten_from_line` relies on this).
    /// - Phase 52: Straighten horizons
    pub rotation: f32,

    /// Quarter turns clockwise on top of the EXIF orientation (0–3): for
    /// portraits the camera never tagged (auto-rotate off — a D3400 file
    /// can carry Orientation = 1 for a vertical frame) and for turning any
    /// frame by hand. Composed into the one orientation the colour pass
    /// already applies (`raw::loader::compose_orientation`), so the crop,
    /// mask shapes and brush strokes all live in the TURNED display space
    /// — `turn_quarter` carries them along when it changes.
    /// serde default keeps edits saved before this field existed loadable.
    #[serde(default)]
    pub quarter_turns: u32,

    // Phase 66: Crop
    /// Crop rectangle [x, y, width, height]
    /// - Normalized coordinates (0.0 to 1.0)
    /// - Defines the visible sub-rectangle of the image
    pub crop: [f32; 4],

    // Phase 135: Non-destructive crop visibility
    /// Flag to indicate if the user is currently cropping
    /// If 1, the shader will show the full image with the crop dimmed outside.
    ///
    /// UI state riding along for the shader, NOT an edit — so never persisted.
    /// It used to be: a crop-handle release flushed `1` to SQLite, and the
    /// image then reopened (and exported) as the crop-mode visualisation —
    /// the whole sensor, outside dimmed, squeezed into the cropped aspect.
    /// `skip` also heals rows already written with `1`.
    #[serde(skip)]
    pub is_cropping: u32,

    /// Mask overlay (0 = off, i + 1 = tint mask i): tool state riding to the
    /// shader exactly like `is_cropping` — never persisted, never copied,
    /// cleared for export. See `without_tool_state`.
    #[serde(skip)]
    pub mask_overlay: u32,

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

    /// Point curves (RGB master + R/G/B), applied in display space after tone
    /// mapping. Identity by default; serde default keeps old edits loadable.
    #[serde(default)]
    pub curves: crate::core::curves::ToneCurves,

    // ========== Lens ==========
    /// Profile and manual lens corrections (distortion, vignetting, lateral
    /// CA). serde default keeps old edits loadable — and uncorrected: the
    /// profile is opt-in, so no saved image changes on upgrade.
    #[serde(default)]
    pub lens: crate::core::lens::LensParams,

    /// Purple/green fringe removal (axial CA). serde default: off.
    #[serde(default)]
    pub defringe: crate::core::lens::DefringeParams,

    // ========== Local Adjustment Masks ==========
    /// Fixed-size mask slots; only the first `mask_count` are meaningful.
    /// Fixed array (not Vec) keeps EditParams `Copy` for the undo stack and
    /// edit clipboard. serde default keeps old saved edits loadable.
    #[serde(default)]
    pub masks: [MaskParams; MAX_MASKS],

    /// Number of active masks (0..=MAX_MASKS)
    #[serde(default)]
    pub mask_count: u32,

    /// Brush strokes for all masks (each stroke names its mask).
    #[serde(default)]
    pub brush: crate::core::brush::BrushData,

    /// Camera colour profile (DCP) this image uses (`core::camera_profiles`).
    /// Applied when the image's GPU resources are built, so changing it
    /// rebuilds them; `Auto` follows the per-camera default.
    #[serde(default)]
    pub camera_profile: crate::core::camera_profiles::ProfileChoice,
}

/// Maximum number of local adjustment masks per image.
/// Mirrored in the WGSL shaders' `masks: array<Mask, 8>` — keep in sync.
pub const MAX_MASKS: usize = 8;

/// `MaskParams::mask_type` values. Mirrored in the colour shader.
pub const MASK_LINEAR: u32 = 0;
pub const MASK_RADIAL: u32 = 1;
pub const MASK_BRUSH: u32 = 2;
pub const MASK_RANGE: u32 = 3;

/// Epsilon in a colour-range mask's log-chromaticity, `ln((r+ε)/(g+ε))`:
/// keeps near-black pixels (whose chromaticity is noise) from scattering.
/// MIRRORED in the colour shader.
pub const MASK_CHROMA_EPS: f32 = 0.004;

/// Colour-range target for a camera-native (black-subtracted, unit-range)
/// colour — the same quantity the shader computes per pixel.
pub fn mask_chroma(rgb: [f32; 3]) -> [f32; 2] {
    let c = rgb.map(|v| v.max(0.0) + MASK_CHROMA_EPS);
    [(c[0] / c[1]).ln(), (c[2] / c[1]).ln()]
}

fn default_profile_curve() -> f32 {
    0.33
}

fn default_luma_detail() -> f32 {
    0.5
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

    // Fields below arrived with brush/range masking; serde defaults keep
    // every mask saved before them loadable, with the refinements off.

    /// Local hue rotation (-1.0 to +1.0 = ±30°), about the grey axis so
    /// luminance is untouched.
    #[serde(default)]
    pub hue: f32,
    /// 1 = restrict the mask to a luminance band.
    #[serde(default)]
    pub luma_range: u32,
    /// Band edges in display (gamma-encoded) luminance, 0..1, and the width
    /// of the soft fade at each edge.
    #[serde(default)]
    pub luma_lo: f32,
    #[serde(default)]
    pub luma_hi: f32,
    #[serde(default)]
    pub luma_smooth: f32,
    /// 1 = restrict the mask to colours near `color_target`.
    #[serde(default)]
    pub color_range: u32,
    /// Picked colour as camera-native log-chromaticity (ln r/g, ln b/g):
    /// independent of exposure and of the white-balance sliders, so a pick
    /// stays valid while either is edited afterwards.
    #[serde(default)]
    pub color_target: [f32; 2],
    /// 0..1 — how far from the picked colour still counts.
    #[serde(default)]
    pub color_tolerance: f32,
}

impl MaskParams {
    /// Turn the luminance refinement on with a band that does nothing yet,
    /// so enabling it never changes the image by itself.
    pub fn enable_luma_range(&mut self) {
        if self.luma_hi <= self.luma_lo {
            self.luma_lo = 0.0;
            self.luma_hi = 1.0;
            self.luma_smooth = 0.1;
        }
        self.luma_range = 1;
    }
}

impl MaskParams {
    /// New enabled linear gradient: weight 1 at (ax, ay) falling to 0 at (bx, by).
    pub fn new_linear(ax: f32, ay: f32, bx: f32, by: f32) -> Self {
        Self {
            mask_type: MASK_LINEAR,
            enabled: 1,
            ax,
            ay,
            bx,
            by,
            ..Self::default()
        }
    }

    /// New enabled brush mask: no shape of its own, only what is painted.
    pub fn new_brush() -> Self {
        Self { mask_type: MASK_BRUSH, enabled: 1, ..Self::default() }
    }

    /// New enabled range mask: the whole frame, narrowed by its luminance
    /// and colour refinements (luminance starts on, at the full range).
    pub fn new_range() -> Self {
        let mut m = Self { mask_type: MASK_RANGE, enabled: 1, ..Self::default() };
        m.enable_luma_range();
        m
    }

    /// New enabled radial ellipse centered at (cx, cy) with radii (rx, ry).
    pub fn new_radial(cx: f32, cy: f32, rx: f32, ry: f32) -> Self {
        Self {
            mask_type: MASK_RADIAL,
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
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            temperature: 0.0,           // Phase 140: As-shot (maps to ~5000K for DCP)
            tint: 0.0,                  // Phase 18: Manual white balance (as-shot)
            luma_noise: 0.0,            // Phase 133: No luma noise reduction by default
            luma_detail: 0.5,           // Mid detail retention (only acts with luma_noise > 0)
            color_noise: 0.3,           // Phase 133: Default color noise reduction to eliminate speckles
            sharpening: 0.0,            // Phase 50: No sharpening by default
            sharpen_masking: 0.0,       // Phase 51: No masking by default
            rotation: 0.0,              // Phase 52: No rotation by default
            quarter_turns: 0,           // as the camera recorded it
            crop: [0.0, 0.0, 1.0, 1.0], // Phase 66: Full image by default
            is_cropping: 0,             // Phase 135: Not cropping by default
            mask_overlay: 0,
            profile_curve: 0.33,        // 1/3 DCP tone curve (matches in-camera JPEG contrast)
            tone_mapper: crate::core::tonemap::ToneMapper::Camera, // calibrated path
            gt: crate::core::tonemap::GtParams::default(),         // inert unless Gt selected
            curves: crate::core::curves::ToneCurves::default(),    // identity
            lens: crate::core::lens::LensParams::default(),        // no correction
            defringe: crate::core::lens::DefringeParams::default(), // off
            masks: [MaskParams::default(); MAX_MASKS], // No local masks by default
            mask_count: 0,
            brush: crate::core::brush::BrushData::default(),
            camera_profile: crate::core::camera_profiles::ProfileChoice::AUTO,
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

    /// These params with the UI tool flags (`is_cropping`, `mask_overlay`)
    /// cleared — the edit itself. Use for every "is this the same edit?"
    /// comparison and before exporting.
    pub fn without_tool_state(mut self) -> Self {
        self.is_cropping = 0;
        self.mask_overlay = 0;
        self
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

    /// Turn the frame a quarter turn, carrying its display-space geometry
    /// along: the crop, mask shapes and brush strokes are all in the oriented
    /// frame, so without this they would stay put on screen over different
    /// content.
    ///
    /// `aspect` is the full frame's display width / height BEFORE the turn
    /// (`core::viewport::mask_uv_aspect`'s convention). A UV point turns
    /// rigidly — clockwise, the top-left corner goes to the top-right — and
    /// a radial mask's radii trade axes, since what spanned u now spans v;
    /// its angle needs nothing, because both are measured in the isotropic
    /// space the turn rotates rigidly. The straighten angle is a rotation
    /// about the same centre, so it commutes with the turn.
    pub fn turn_quarter(&mut self, clockwise: bool, aspect: f32) {
        let turn = |u: f32, v: f32| if clockwise { (1.0 - v, u) } else { (v, 1.0 - u) };
        self.quarter_turns = (self.quarter_turns + if clockwise { 1 } else { 3 }) % 4;

        let [x, y, w, h] = self.crop;
        self.crop = if clockwise {
            [1.0 - (y + h), x, h, w]
        } else {
            [y, 1.0 - (x + w), h, w]
        };

        for m in self.masks.iter_mut().take(self.mask_count as usize) {
            match m.mask_type {
                MASK_LINEAR => {
                    (m.ax, m.ay) = turn(m.ax, m.ay);
                    (m.bx, m.by) = turn(m.bx, m.by);
                }
                MASK_RADIAL => {
                    (m.ax, m.ay) = turn(m.ax, m.ay);
                    (m.bx, m.by) = (m.by, m.bx);
                }
                // Brush and range masks have no shape of their own.
                _ => {}
            }
        }
        self.brush.turn_quarter(clockwise, aspect);
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
    /// Noise reduction is kept too. Its stage caches one output, and the
    /// default (Color 0.3) is itself a setting, so a reset NR meant a
    /// different denoise on every press AND every release of the peek —
    /// ~0.5 s each way at full resolution on an iGPU. NR is judged by
    /// dragging its own sliders at 1:1 anyway.
    ///
    /// Change the preserved list here (and nowhere else) if you would rather
    /// see the truly untouched frame.
    pub fn before_reference(&self) -> Self {
        Self {
            luma_noise: self.luma_noise,
            luma_detail: self.luma_detail,
            color_noise: self.color_noise,
            crop: self.crop,
            rotation: self.rotation,
            quarter_turns: self.quarter_turns,
            // Lens corrections reframe the image (distortion, constrain zoom),
            // so they are geometry for this purpose.
            lens: self.lens,
            is_cropping: self.is_cropping,
            black_offsets: self.black_offsets,
            black_phase_x: self.black_phase_x,
            black_phase_y: self.black_phase_y,
            // Not a look you compare against: switching it rebuilds the GPU
            // resources, which a momentary peek must not do.
            camera_profile: self.camera_profile,
            ..Self::default()
        }
    }
}

/// Which groups of adjustments a copy/paste operation carries.
///
/// Stored ALONGSIDE the copied `EditParams` rather than being applied at copy
/// time — see [`EditClipboard`] for why that distinction matters.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
// Per-field defaults: a preferences file saved before a category existed
// (e.g. `lens`) must still parse — otherwise the whole settings file fails
// and every preference resets.
#[serde(default)]
pub struct CopyCategories {
    pub tone: bool,
    pub color: bool,
    pub white_balance: bool,
    /// Texture, clarity, dehaze.
    pub presence: bool,
    pub noise: bool,
    pub detail: bool,
    pub geometry: bool,
    pub profile: bool,
    pub masks: bool,
    /// Lens corrections and defringe.
    pub lens: bool,
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
            presence: true,
            noise: true,
            detail: true,
            geometry: false,
            profile: true,
            masks: false,
            lens: true,
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
            presence: true,
            noise: true,
            detail: true,
            geometry: true,
            profile: true,
            masks: true,
            lens: true,
            black_levels: true,
        }
    }

    pub fn none() -> Self {
        Self {
            tone: false,
            color: false,
            white_balance: false,
            presence: false,
            noise: false,
            detail: false,
            geometry: false,
            profile: false,
            masks: false,
            lens: false,
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
        dst.curves = src.curves;
    }
    if cats.color {
        dst.vibrance = src.vibrance;
        dst.saturation = src.saturation;
    }
    if cats.white_balance {
        dst.temperature = src.temperature;
        dst.tint = src.tint;
    }
    if cats.presence {
        dst.texture = src.texture;
        dst.clarity = src.clarity;
        dst.dehaze = src.dehaze;
    }
    if cats.noise {
        dst.luma_noise = src.luma_noise;
        dst.luma_detail = src.luma_detail;
        dst.color_noise = src.color_noise;
    }
    if cats.detail {
        dst.sharpening = src.sharpening;
        dst.sharpen_masking = src.sharpen_masking;
    }
    if cats.geometry {
        dst.rotation = src.rotation;
        dst.quarter_turns = src.quarter_turns;
        dst.crop = src.crop;
    }
    if cats.profile {
        dst.profile_curve = src.profile_curve;
        dst.tone_mapper = src.tone_mapper;
        dst.gt = src.gt;
        dst.camera_profile = src.camera_profile;
    }
    if cats.masks {
        // Together, always: `masks` without `mask_count` (or vice versa)
        // leaves the render evaluating slots the user never placed.
        dst.masks = src.masks;
        dst.mask_count = src.mask_count;
        // Strokes index into `masks`; they travel with them.
        dst.brush = src.brush;
    }
    if cats.lens {
        dst.lens = src.lens;
        dst.defringe = src.defringe;
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

    /// Edits saved before Texture/Clarity/Dehaze existed must load with all
    /// three off — not as an image-wide change to anyone's catalogue.
    #[test]
    fn test_legacy_json_without_presence_defaults_to_off() {
        let json = EditParams::default().to_json().unwrap();
        let legacy = json
            .replace(",\"texture\":0.0", "")
            .replace(",\"clarity\":0.0", "")
            .replace(",\"dehaze\":0.0", "");
        assert!(!legacy.contains("clarity"), "legacy JSON still has the field");
        let restored = EditParams::from_json(&legacy).unwrap();
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
        p.texture = 0.4;
        p.clarity = -0.3;
        p.dehaze = 0.5;
        p.temperature = 0.5;
        p.tint = -0.25;
        p.luma_noise = 0.4;
        p.luma_detail = 0.8;
        p.color_noise = 0.7;
        p.sharpening = 0.8;
        p.sharpen_masking = 0.6;
        p.rotation = 12.0;
        p.quarter_turns = 3;
        p.crop = [0.1, 0.2, 0.5, 0.6];
        p.profile_curve = 0.9;
        p.tone_mapper = crate::core::tonemap::ToneMapper::Gt;
        p.gt = crate::core::tonemap::GtParams { contrast: 1.7, ..Default::default() };
        p.curves.master.insert(0.5, 0.6);
        p.lens.profile = true;
        p.lens.distortion = 0.3;
        p.defringe.purple_amount = 0.5;
        p.masks[0] = MaskParams::new_radial(0.4, 0.4, 0.2, 0.15);
        p.masks[0].exposure = -1.0;
        p.masks[0].enable_luma_range();
        p.mask_count = 1;
        p.brush.begin(0, false, 0.05, 0.5, 1.0);
        p.brush.extend([0.2, 0.2], 1.5);
        p.camera_profile = crate::core::camera_profiles::ProfileChoice::named("Test.dcp").unwrap();
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
        assert_eq!(b.quarter_turns, p.quarter_turns, "a turned frame stays turned");
        assert_eq!(b.lens, p.lens, "lens corrections reframe the image");
        assert_eq!(b.is_cropping, p.is_cropping);
        assert_eq!(b.black_offsets, p.black_offsets);
        assert_eq!(b.black_phase_x, p.black_phase_x);
        assert_eq!(b.black_phase_y, p.black_phase_y);
    }

    /// NR is kept: the denoise stage caches one output, so a reset NR re-ran
    /// it on every press and release of the peek.
    #[test]
    fn before_reference_keeps_noise_reduction() {
        let mut p = fully_edited();
        p.luma_noise = 0.4;
        p.luma_detail = 0.7;
        p.color_noise = 0.6;
        let b = p.before_reference();
        assert_eq!(
            (b.luma_noise, b.luma_detail, b.color_noise),
            (p.luma_noise, p.luma_detail, p.color_noise)
        );
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
        assert_eq!(b.texture, d.texture);
        assert_eq!(b.clarity, d.clarity);
        assert_eq!(b.dehaze, d.dehaze);
        assert_eq!(b.temperature, d.temperature);
        assert_eq!(b.tint, d.tint);
        assert_eq!(b.sharpening, d.sharpening);
        assert_eq!(b.profile_curve, d.profile_curve);
        assert_eq!(b.tone_mapper, d.tone_mapper);
        assert_eq!(b.mask_count, 0, "masks must not apply to the before view");
        assert_eq!(b.curves, d.curves);
        assert_eq!(b.defringe, d.defringe);
    }

    // ── quarter turns ────────────────────────────────────────────────────────

    fn close(a: &EditParams, b: &EditParams) -> bool {
        let near = |x: f32, y: f32| (x - y).abs() < 1e-5;
        let crop = a.crop.iter().zip(&b.crop).all(|(x, y)| near(*x, *y));
        let masks = a.masks.iter().zip(&b.masks).all(|(m, n)| {
            near(m.ax, n.ax) && near(m.ay, n.ay) && near(m.bx, n.bx) && near(m.by, n.by) && near(m.rotation, n.rotation)
        });
        let points = a.brush.points.iter().zip(&b.brush.points).all(|(p, q)| near(p[0], q[0]) && near(p[1], q[1]));
        let radii = a.brush.strokes.iter().zip(&b.brush.strokes).all(|(s, t)| near(s.radius, t.radius));
        a.quarter_turns == b.quarter_turns && crop && masks && points && radii
    }

    /// Turning back, or all the way round, must give back exactly the frame
    /// the user had — crop, masks and strokes included.
    #[test]
    fn quarter_turns_round_trip() {
        let mut p = fully_edited();
        p.quarter_turns = 0;
        p.masks[1] = MaskParams::new_linear(0.1, 0.2, 0.7, 0.9);
        p.mask_count = 2;
        let aspect = 1.5;

        let mut back = p;
        back.turn_quarter(true, aspect);
        back.turn_quarter(false, 1.0 / aspect);
        assert!(close(&back, &p), "cw then ccw");

        let mut round = p;
        for i in 0..4 {
            let a = if i % 2 == 0 { aspect } else { 1.0 / aspect };
            round.turn_quarter(true, a);
        }
        assert!(close(&round, &p), "four turns");
    }

    /// The geometry lands on the same content: the left half of the frame,
    /// turned clockwise, is the top half; a point near the top-left goes
    /// near the top-right; radial radii trade axes.
    #[test]
    fn a_clockwise_turn_carries_the_geometry_with_the_content() {
        let mut p = EditParams { crop: [0.0, 0.0, 0.5, 1.0], ..Default::default() };
        p.masks[0] = MaskParams::new_radial(0.1, 0.2, 0.3, 0.05);
        p.mask_count = 1;
        p.turn_quarter(true, 1.5);
        assert_eq!(p.quarter_turns, 1);
        assert_eq!(p.crop, [0.0, 0.0, 1.0, 0.5]);
        let m = p.masks[0];
        assert!((m.ax - 0.8).abs() < 1e-6 && (m.ay - 0.1).abs() < 1e-6, "centre {:?}", (m.ax, m.ay));
        assert_eq!((m.bx, m.by), (0.05, 0.3));
    }

    /// A brush radius is a fraction of the image height, and the turn makes
    /// the old width the new height: the dab must keep its size in pixels.
    #[test]
    fn a_turn_keeps_brush_dabs_the_same_size_on_screen() {
        let mut p = EditParams::default();
        p.brush.begin(0, false, 0.06, 0.5, 1.0);
        p.brush.extend([0.25, 0.5], 1.5);
        let (w, h) = (3000.0f32, 2000.0f32);
        let before_px = p.brush.strokes[0].radius * h;
        p.turn_quarter(true, w / h);
        // Now 2000 wide and 3000 tall.
        let after_px = p.brush.strokes[0].radius * w;
        assert!((before_px - after_px).abs() < 1e-3, "{before_px} px became {after_px} px");
        assert_eq!(p.brush.points[0], [0.5, 0.25]);
    }

    #[test]
    fn edits_saved_before_quarter_turns_load_upright() {
        let json = EditParams::default().to_json().unwrap().replace(",\"quarter_turns\":0", "");
        assert!(!json.contains("quarter_turns"));
        assert_eq!(EditParams::from_json(&json).unwrap().quarter_turns, 0);
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
        // The tool flags are the deliberate exclusions (transient UI state).
        dst.is_cropping = src.is_cropping;
        dst.mask_overlay = src.mask_overlay;
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
        assert_eq!(geo.quarter_turns, src.quarter_turns);
        assert_eq!(geo.exposure, base.exposure);

        let profile = only(|c| c.profile = true);
        assert_eq!(profile.tone_mapper, src.tone_mapper);
        assert_eq!(profile.gt, src.gt);
        assert_eq!(profile.profile_curve, src.profile_curve);
        assert_eq!(profile.exposure, base.exposure);

        let presence = only(|c| c.presence = true);
        assert_eq!(presence.texture, src.texture);
        assert_eq!(presence.clarity, src.clarity);
        assert_eq!(presence.dehaze, src.dehaze);
        assert_eq!(presence.exposure, base.exposure);
        assert_eq!(presence.saturation, base.saturation);

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
        assert!(d.tone && d.color && d.white_balance && d.profile && d.lens);
        assert!(!d.geometry, "a crop is composed for one specific frame");
        assert!(!d.black_levels, "sensor calibration is camera-specific");
        assert!(!d.is_empty());
        assert!(CopyCategories::none().is_empty());
        assert!(!CopyCategories::all().is_empty());
    }

    #[test]
    fn copy_categories_saved_before_a_category_existed_still_load() {
        let json = r#"{"tone":false,"color":true,"white_balance":true,"noise":true,"detail":true,"geometry":false,"profile":true,"masks":false,"black_levels":false}"#;
        let c: CopyCategories = serde_json::from_str(json).expect("legacy prefs must parse");
        assert!(!c.tone, "existing choices survive");
        assert!(c.lens, "the new category takes its default");
        assert!(c.presence, "the new category takes its default");
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
