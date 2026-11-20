// src/raw/dcp.rs

use crate::core::tonemap::{tone_lut_decode, ToneSettings, TONE_LUT_SIZE};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};


// ── Correct DCP Tag IDs ──────────────────────────────────────────────────────
const TAG_CALIBRATION_ILLUMINANT_1:    u16 = 50778;
const TAG_CALIBRATION_ILLUMINANT_2:    u16 = 50779;
const TAG_COLOR_MATRIX_1:              u16 = 50721;
const TAG_COLOR_MATRIX_2:             u16 = 50722;
const TAG_FORWARD_MATRIX_1:           u16 = 50964;
const TAG_FORWARD_MATRIX_2:           u16 = 50965;
// A profile carries up to two 3D hue/sat/value tables. Camera-matching
// profiles (like the D3300 "Camera Standard" this was calibrated on) ship a
// LookTable and no HueSatMap; open-source ones (RawTherapee's) ship a
// dual-illuminant HueSatMap and no LookTable. The pipeline has one table
// slot: the LookTable when there is one — what it always used — else the
// HueSatMap. (Reading 50981 as "the HueSatMap" was this code's old name for
// the LookTable, and is why profiles without one rendered black.)
const TAG_PROFILE_HUE_SAT_MAP_DIMS:   u16 = 50937;
const TAG_PROFILE_HUE_SAT_MAP_DATA_1: u16 = 50938;
const TAG_PROFILE_HUE_SAT_MAP_DATA_2: u16 = 50939;
const TAG_PROFILE_LOOK_TABLE_DIMS:    u16 = 50981;
const TAG_PROFILE_LOOK_TABLE_DATA:    u16 = 50982;
const TAG_PROFILE_TONE_CURVE:         u16 = 50940;
const TAG_BASELINE_EXPOSURE_OFFSET:   u16 = 51109;
const TAG_UNIQUE_CAMERA_MODEL:        u16 = 50708;
const TAG_PROFILE_NAME:               u16 = 50936;

// TIFF type sizes in bytes
fn tiff_type_size(typ: u16) -> u32 {
    match typ {
        1 | 2 | 6 | 7 => 1,   // BYTE, ASCII, SBYTE, UNDEFINED
        3 | 8          => 2,   // SHORT, SSHORT
        4 | 9 | 11     => 4,   // LONG, SLONG, FLOAT
        5 | 10 | 12    => 8,   // RATIONAL, SRATIONAL, DOUBLE
        _              => 1,
    }
}

struct RawTag {
    typ:   u16,
    data:  Vec<u8>,
}

fn read_ifd(path: &Path) -> Result<HashMap<u16, RawTag>, String> {
    let mut f = std::fs::File::open(path)
        .map_err(|e| format!("Cannot open DCP: {e}"))?;

    let file_len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut hdr = [0u8; 8];
    f.read_exact(&mut hdr).map_err(|e| format!("Header read: {e}"))?;

    let le = match &hdr[0..2] {
        b"II" => true,
        b"MM" => false,
        o => return Err(format!("Not a TIFF file: {:02X}{:02X}", o[0], o[1])),
    };

    let u16_from = |b: [u8; 2]| if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) };
    let u32_from = |b: [u8; 4]| if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) };

    // Adobe DCP files are TIFF-structured but use magic 0x4352 ("CR", Camera Raw
    // profile) instead of TIFF's 42. Accept both.
    let magic = u16_from([hdr[2], hdr[3]]);
    if magic != 42 && magic != 0x4352 {
        return Err(format!("Not a TIFF/DCP file (magic=0x{magic:04X}, expected 0x002A or 0x4352)"));
    }

    let ifd0 = u32_from([hdr[4], hdr[5], hdr[6], hdr[7]]);
    f.seek(SeekFrom::Start(ifd0 as u64))
        .map_err(|e| format!("Seek IFD0: {e}"))?;

    let mut cnt_buf = [0u8; 2];
    f.read_exact(&mut cnt_buf).map_err(|e| format!("IFD count: {e}"))?;
    let entry_count = u16_from(cnt_buf);

    let mut map: HashMap<u16, RawTag> = HashMap::new();

    for _ in 0..entry_count {
        let mut raw = [0u8; 12];
        f.read_exact(&mut raw).map_err(|e| format!("IFD entry: {e}"))?;

        let tag   = u16_from([raw[0], raw[1]]);
        let typ   = u16_from([raw[2], raw[3]]);
        let count = u32_from([raw[4], raw[5], raw[6], raw[7]]);
        let voff  = [raw[8], raw[9], raw[10], raw[11]];

        let total = count.saturating_mul(tiff_type_size(typ));

        let data = if total <= 4 {
            voff[..total as usize].to_vec()
        } else {
            let offset = u32_from(voff);
            // A damaged or hostile profile can claim gigabytes: check the
            // range lies inside the file before allocating for it.
            if offset as u64 + total as u64 > file_len {
                return Err(format!("Tag {tag} data runs past the end of the file"));
            }
            let saved = f.stream_position().map_err(|e| e.to_string())?;
            f.seek(SeekFrom::Start(offset as u64))
                .map_err(|e| format!("Seek data tag={tag}: {e}"))?;
            let mut buf = vec![0u8; total as usize];
            f.read_exact(&mut buf)
                .map_err(|e| format!("Read data tag={tag}: {e}"))?;
            f.seek(SeekFrom::Start(saved)).map_err(|e| e.to_string())?;
            buf
        };

        map.insert(tag, RawTag { typ, data });
    }

    Ok(map)
}

// ── Helpers for reading typed tag data ──────────────────────────────────────

fn tag_floats(tag: &RawTag, le: bool) -> Vec<f32> {
    match tag.typ {
        11 => { // FLOAT
            tag.data.chunks(4).filter(|c| c.len() == 4).map(|c| {
                f32::from_bits(if le {
                    u32::from_le_bytes([c[0],c[1],c[2],c[3]])
                } else {
                    u32::from_be_bytes([c[0],c[1],c[2],c[3]])
                })
            }).collect()
        }
        10 => { // SRATIONAL
            tag.data.chunks(8).filter(|c| c.len()==8).map(|c| {
                let n = if le { i32::from_le_bytes([c[0],c[1],c[2],c[3]]) }
                        else  { i32::from_be_bytes([c[0],c[1],c[2],c[3]]) };
                let d = if le { i32::from_le_bytes([c[4],c[5],c[6],c[7]]) }
                        else  { i32::from_be_bytes([c[4],c[5],c[6],c[7]]) };
                if d == 0 { 0.0 } else { n as f32 / d as f32 }
            }).collect()
        }
        5 => { // RATIONAL (unsigned)
            tag.data.chunks(8).filter(|c| c.len()==8).map(|c| {
                let n = if le { u32::from_le_bytes([c[0],c[1],c[2],c[3]]) }
                        else  { u32::from_be_bytes([c[0],c[1],c[2],c[3]]) };
                let d = if le { u32::from_le_bytes([c[4],c[5],c[6],c[7]]) }
                        else  { u32::from_be_bytes([c[4],c[5],c[6],c[7]]) };
                if d == 0 { 0.0 } else { n as f32 / d as f32 }
            }).collect()
        }
        _ => vec![],
    }
}

fn tag_u32s(tag: &RawTag, le: bool) -> Vec<u32> {
    match tag.typ {
        3 => tag.data.chunks(2).filter(|c| c.len()==2).map(|c|
            if le { u16::from_le_bytes([c[0],c[1]]) as u32 }
            else  { u16::from_be_bytes([c[0],c[1]]) as u32 }).collect(),
        4 => tag.data.chunks(4).filter(|c| c.len()==4).map(|c|
            if le { u32::from_le_bytes([c[0],c[1],c[2],c[3]]) }
            else  { u32::from_be_bytes([c[0],c[1],c[2],c[3]]) }).collect(),
        _ => vec![],
    }
}

/// An ASCII (or UTF-8, as some tools write) tag, NUL-terminated.
fn tag_string(tag: &RawTag) -> Option<String> {
    let bytes = tag.data.split(|b| *b == 0).next()?;
    let s = String::from_utf8_lossy(bytes).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn tag_matrix(tag: &RawTag, le: bool) -> Option<[f32; 9]> {
    let v = tag_floats(tag, le);
    if v.len() >= 9 {
        Some([v[0],v[1],v[2],v[3],v[4],v[5],v[6],v[7],v[8]])
    } else { None }
}

// ── Public types (unchanged interface) ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DcpProfile {
    pub illuminant1: u32,
    pub illuminant2: u32,
    pub color_matrix_1: [f32; 9],
    pub color_matrix_2: [f32; 9],
    pub forward_matrix_1: Option<[f32; 9]>,
    pub forward_matrix_2: Option<[f32; 9]>,
    pub hue_sat_dims: (u32, u32, u32),
    pub hue_sat_data_1: Option<Vec<[f32; 3]>>,
    pub hue_sat_data_2: Option<Vec<[f32; 3]>>,
    pub tone_curve: Option<Vec<(f32, f32)>>,
    /// BaselineExposureOffset (tag 51109), in EV. Camera-matching profiles use
    /// it to cancel Adobe's per-camera baseline so the render matches the
    /// in-camera JPEG brightness (this D3300 profile: -0.35 EV).
    pub baseline_exposure_offset: f32,
    /// UniqueCameraModel: the camera it was made for ("Nikon D3300").
    pub camera_model: Option<String>,
    /// ProfileName: what it calls itself ("Camera Standard").
    pub profile_name: Option<String>,
}

impl DcpProfile {
    /// The pipeline needs a camera -> XYZ D50 ForwardMatrix. Without one the
    /// profile cannot supply the matrix (the ColorMatrix is its inverse
    /// direction), so it is not offered.
    pub fn has_forward_matrix(&self) -> bool {
        self.forward_matrix_1.is_some() || self.forward_matrix_2.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct InterpolatedProfile {
    /// Camera RGB → **ProPhoto** RGB, row-major.
    ///
    /// The DCP's ForwardMatrix is camera → XYZ D50; the XYZ → ProPhoto step is
    /// folded in here rather than done per pixel on the GPU. XYZ is unavoidable
    /// in the derivation (that is how the DCP defines its matrix, and ProPhoto's
    /// primaries are specified in XYZ) but it does not belong in the hot path —
    /// both matrices are constant across a frame, so their product is too.
    ///
    /// Recomputed only when the DCP is re-interpolated, which
    /// `RawEditor::dcp_memo` already gates on `(kelvin, ToneSettings)`.
    pub camera_to_prophoto: [f32; 9],
    pub hue_sat_lut: Vec<[f32; 3]>,
    /// Tone LUT for the CURRENTLY SELECTED operator — which is the profile's
    /// own ProfileToneCurve only while `ToneMapper::Camera` is active. Indexed
    /// by `tonemap::tone_lut_encode`, display-linear output. See
    /// `bake_tone_curve`.
    pub tone_curve: Vec<f32>,
    pub hue_sat_dims: (u32, u32, u32),
    /// True when the DCP embeds a ProfileToneCurve. "Adobe Standard" profiles
    /// don't — they expect the host's default rendering curve, so the shader
    /// keeps its filmic fallback curve when this is false.
    pub has_tone_curve: bool,
}

pub fn parse_dcp(path: &Path) -> Result<DcpProfile, String> {
    // Detect byte order from the header for helpers
    let mut hdr2 = [0u8; 2];
    {
        let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        f.read_exact(&mut hdr2).map_err(|e| e.to_string())?;
    }
    let le = hdr2 == *b"II";

    let map = read_ifd(path)?;

    let identity = [1.,0.,0., 0.,1.,0., 0.,0.,1.];

    let cm1 = map.get(&TAG_COLOR_MATRIX_1)
        .and_then(|t| tag_matrix(t, le))
        .unwrap_or(identity);
    let cm2 = map.get(&TAG_COLOR_MATRIX_2)
        .and_then(|t| tag_matrix(t, le))
        .unwrap_or(cm1);
    let fm1 = map.get(&TAG_FORWARD_MATRIX_1).and_then(|t| tag_matrix(t, le));
    let fm2 = map.get(&TAG_FORWARD_MATRIX_2).and_then(|t| tag_matrix(t, le));

    let illuminant1 = map.get(&TAG_CALIBRATION_ILLUMINANT_1)
        .and_then(|t| tag_u32s(t, le).into_iter().next())
        .unwrap_or(17);
    let illuminant2 = map.get(&TAG_CALIBRATION_ILLUMINANT_2)
        .and_then(|t| tag_u32s(t, le).into_iter().next())
        // No second illuminant means a single-illuminant profile: the blend
        // weight must be 0 at every temperature, not a guess at D65.
        .unwrap_or(illuminant1);

    let dims_of = |tag: u16| {
        map.get(&tag)
            .map(|t| tag_u32s(t, le))
            .filter(|v| v.len() == 3 && v.iter().all(|&d| d > 0))
            .map(|v| (v[0], v[1], v[2]))
    };
    let parse_hsm = |tag: &RawTag| -> Vec<[f32; 3]> {
        tag_floats(tag, le).chunks(3)
            .filter(|c| c.len() == 3)
            .map(|c| [c[0], c[1], c[2]])
            .collect()
    };
    // See the tag constants: the LookTable if present, else the HueSatMap.
    let (hue_sat_dims, hue_sat_data_1, hue_sat_data_2) = match (
        dims_of(TAG_PROFILE_LOOK_TABLE_DIMS).zip(map.get(&TAG_PROFILE_LOOK_TABLE_DATA)),
        dims_of(TAG_PROFILE_HUE_SAT_MAP_DIMS),
    ) {
        (Some((dims, data)), _) => (dims, Some(parse_hsm(data)), None),
        (None, Some(dims)) => (
            dims,
            map.get(&TAG_PROFILE_HUE_SAT_MAP_DATA_1).map(parse_hsm),
            map.get(&TAG_PROFILE_HUE_SAT_MAP_DATA_2).map(parse_hsm),
        ),
        (None, None) => ((0, 0, 0), None, None),
    };

    let baseline_exposure_offset = map.get(&TAG_BASELINE_EXPOSURE_OFFSET)
        .and_then(|t| tag_floats(t, le).into_iter().next())
        .unwrap_or(0.0);

    // An identity curve (RawTherapee's profiles carry (0,0)–(1,1)) says "no
    // tone shaping". Read as a curve it is not neutral here: this pipeline
    // treats the profile curve as linear → display-encoded, so the identity
    // would darken the image by a whole gamma. No curve leaves the default
    // rendering in charge, which is what the profile means.
    let tone_curve = map.get(&TAG_PROFILE_TONE_CURVE).map(|t| {
        tag_floats(t, le).chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| (c[0], c[1]))
            .collect::<Vec<_>>()
    })
    .filter(|points| !points.iter().all(|(x, y)| (x - y).abs() < 1e-4));

    tracing::info!(
        "DCP parsed: illuminants=({},{}) fm1={} fm2={} hsm_dims={:?} hsm1_len={:?} curve_pts={:?}",
        illuminant1, illuminant2,
        fm1.is_some(), fm2.is_some(),
        hue_sat_dims,
        hue_sat_data_1.as_ref().map(|v| v.len()),
        tone_curve.as_ref().map(|v: &Vec<(f32, f32)>| v.len()),
    );

    Ok(DcpProfile {
        illuminant1, illuminant2,
        color_matrix_1: cm1, color_matrix_2: cm2,
        forward_matrix_1: fm1, forward_matrix_2: fm2,
        hue_sat_dims,
        hue_sat_data_1, hue_sat_data_2,
        tone_curve,
        baseline_exposure_offset,
        camera_model: map.get(&TAG_UNIQUE_CAMERA_MODEL).and_then(tag_string),
        profile_name: map.get(&TAG_PROFILE_NAME).and_then(tag_string),
    })
}

// ── Keep these from old dcp.rs ───────────────────────────────────────────────

pub fn illuminant_to_kelvin(ill: u32) -> f32 {
    match ill {
        17 => 2850.0, // Standard Light A
        18 => 4800.0, // Standard Light B
        19 => 6774.0, // Standard Light C
        20 => 5500.0, // D55
        21 => 6500.0, // D65
        22 => 7500.0, // D75
        23 => 5000.0, // D50
        1  => 5500.0, // Daylight
        3  => 2850.0, // Tungsten
        10 => 6500.0, // Cloudy
        11 => 7500.0, // Shade
        24 => 3200.0, // ISO studio tungsten
        _  => 5000.0,
    }
}

/// Dual-illuminant blend weight for `kelvin` (0 = illuminant1, 1 = illuminant2),
/// interpolated in inverse-temperature (mired) space per the DNG spec.
fn illuminant_weight(profile: &DcpProfile, kelvin: f32) -> f32 {
    let t1 = illuminant_to_kelvin(profile.illuminant1);
    let t2 = illuminant_to_kelvin(profile.illuminant2);

    let inv_t  = 1.0 / kelvin.max(1000.0);
    let inv_t1 = 1.0 / t1;
    let inv_t2 = 1.0 / t2;

    if (inv_t2 - inv_t1).abs() < 1e-6 {
        0.0
    } else {
        ((inv_t - inv_t1) / (inv_t2 - inv_t1)).clamp(0.0, 1.0)
    }
}

/// Interpolated XYZ→camera ColorMatrix at the given temperature.
/// Used by the CCT solver (`color::wb_to_kelvin_tint` and friends).
pub fn interpolate_color_matrix(profile: &DcpProfile, kelvin: f32) -> [f32; 9] {
    let weight = illuminant_weight(profile, kelvin);
    std::array::from_fn(|i| {
        profile.color_matrix_1[i] * (1.0 - weight) + profile.color_matrix_2[i] * weight
    })
}

pub fn interpolate_at_temperature(
    profile: &DcpProfile,
    kelvin: f32,
    tone: ToneSettings,
) -> InterpolatedProfile {
    let weight = illuminant_weight(profile, kelvin);

    // A ColorMatrix is XYZ -> camera, the inverse of a ForwardMatrix, so it is
    // no stand-in; `load_profile` rejects profiles with no ForwardMatrix at
    // all. With only one of the two, use it for both.
    let fm1 = profile.forward_matrix_1.or(profile.forward_matrix_2).unwrap_or(profile.color_matrix_1);
    let fm2 = profile.forward_matrix_2.or(profile.forward_matrix_1).unwrap_or(profile.color_matrix_2);

    // Fold BaselineExposureOffset into the forward matrix: scaling camera→XYZ
    // uniformly is an exposure shift applied before the HueSatMap and tone
    // curve — where Adobe applies it. This is what makes the default render
    // match the in-camera JPEG brightness (-0.35 EV for this profile family).
    let baseline_gain = 2.0f32.powf(profile.baseline_exposure_offset);
    let mut forward_matrix = [0.0f32; 9];
    for i in 0..9 {
        forward_matrix[i] = (fm1[i] * (1.0 - weight) + fm2[i] * weight) * baseline_gain;
    }

    // Fold XYZ D50 → ProPhoto in here so the shader multiplies by ONE matrix.
    // Order matters: the camera matrix is applied to the pixel first, so it is
    // the right-hand operand.
    let camera_to_prophoto = crate::color::mat3_mul(&crate::color::XYZ_D50_TO_PROPHOTO, &forward_matrix);

    let num_elements = (profile.hue_sat_dims.0 as usize)
        .saturating_mul(profile.hue_sat_dims.1 as usize)
        .saturating_mul(profile.hue_sat_dims.2 as usize);
    let hue_sat_lut = if num_elements > 0 {
        match (&profile.hue_sat_data_1, &profile.hue_sat_data_2) {
            (Some(lut1), Some(lut2)) => (0..num_elements).map(|i| {
                let v1 = lut1.get(i).unwrap_or(&[0.0, 1.0, 1.0]);
                let v2 = lut2.get(i).unwrap_or(&[0.0, 1.0, 1.0]);
                [
                    v1[0] * (1.0 - weight) + v2[0] * weight,
                    v1[1] * (1.0 - weight) + v2[1] * weight,
                    v1[2] * (1.0 - weight) + v2[2] * weight,
                ]
            }).collect(),
            (Some(lut), None) | (None, Some(lut)) => lut.clone(),
            (None, None) => vec![],
        }
    } else {
        vec![]
    };
    // No table at all (or an unusable one): a single identity entry, so the
    // GPU still gets a valid 3D texture to sample. A zero-sized one is an
    // invalid texture, and the colour pass renders black.
    let (hue_sat_dims, hue_sat_lut) = if hue_sat_lut.is_empty() {
        ((1, 1, 1), vec![[0.0, 1.0, 1.0]])
    } else {
        (profile.hue_sat_dims, hue_sat_lut)
    };

    let has_tone_curve = profile.tone_curve.is_some();
    // `bake_tone_curve` handles the no-points case itself, so a profile with
    // no embedded curve and a non-Camera operator selected still gets that
    // operator's table rather than an identity ramp.
    let baked_curve = bake_tone_curve(
        profile.tone_curve.as_deref().unwrap_or(&[]),
        tone,
    );

    InterpolatedProfile {
        camera_to_prophoto,
        hue_sat_lut,
        tone_curve: baked_curve,
        hue_sat_dims,
        has_tone_curve,
    }
}

/// Inverse sRGB transfer function (IEC 61966-2-1).
fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Forward sRGB transfer function (IEC 61966-2-1).
fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Piecewise-linear evaluation of a ProfileToneCurve's control points,
/// holding the end values outside the sampled range.
fn eval_curve_points(points: &[(f32, f32)], x: f32) -> f32 {
    if x <= points[0].0 {
        return points[0].1;
    }
    let last = points.last().expect("caller checked non-empty");
    if x >= last.0 {
        return last.1;
    }
    points
        .windows(2)
        .find(|w| x >= w[0].0 && x <= w[1].0 && w[1].0 > w[0].0)
        .map(|w| {
            let t = (x - w[0].0) / (w[1].0 - w[0].0);
            w[0].1 * (1.0 - t) + w[1].1 * t
        })
        .unwrap_or(x)
}

/// Bake the selected tone rendering into a display-linear LUT of
/// `TONE_LUT_SIZE` entries.
///
/// Entry `i` holds the curve evaluated at the scene-linear value that LUT
/// coordinate `i / (N-1)` stands for — the table is indexed by
/// `tonemap::tone_lut_encode`, NOT uniformly over [0,1], so that one table can
/// serve operators whose domain runs well past 1.0. See `core::tonemap`.
///
/// `points` is the profile's own ProfileToneCurve and is used only when
/// `tone.mapper` is `Camera`; every other operator replaces this stage
/// outright and needs no profile at all (which is what lets them work on
/// images with no DCP).
pub fn bake_tone_curve(points: &[(f32, f32)], tone: ToneSettings) -> Vec<f32> {
    if tone.mapper.is_baked_operator() {
        return crate::core::tonemap::bake(tone.mapper, tone.gt);
    }

    if points.is_empty() {
        // No embedded curve: the LUT must be a no-op, and `sample_tone_curve`
        // looks up by ENCODED coordinate — so the identity table is `decode`,
        // not a uniform ramp. (The shader's Step 15 filmic block supplies the
        // rendering curve in this case.)
        return (0..TONE_LUT_SIZE)
            .map(|i| tone_lut_decode(i as f32 / (TONE_LUT_SIZE - 1) as f32))
            .collect();
    }

    let strength = tone.profile_curve.clamp(0.0, 1.0);
    // Adobe ProfileToneCurve points map LINEAR input → GAMMA-ENCODED (display)
    // output — the curve embeds the ~2.2 display gamma plus the profile's
    // contrast s-curve (e.g. the ACR default maps 0.18 → ≈0.48 ≈ 0.18^(1/2.2)).
    // Our shader keeps working in linear space after the curve and applies the
    // sRGB transfer function at the very end, so bake the LUT with the curve's
    // output linearised.  At default settings the final encode then reproduces
    // the profile's intended rendering exactly; applying the raw curve values
    // instead double-gammas the image (~+1.3 EV apparent overexposure).
    //
    // This is the one operator whose output needs that conversion — everything
    // in `core::tonemap` is display-LINEAR already.
    (0..TONE_LUT_SIZE)
        .map(|i| {
            let t = i as f32 / (TONE_LUT_SIZE - 1) as f32;
            // The profile curve is only defined over [0,1]. On the Camera path
            // the shader's Reinhard knee runs first and is asymptotic to 1.0,
            // so inputs never actually exceed it; this clamp is defensive and
            // reproduces the old [0,1]-domain table's behaviour exactly.
            let x = tone_lut_decode(t).min(1.0);
            let y = eval_curve_points(points, x);
            // Blend toward "no curve" (= plain sRGB gamma) in display space.
            let y_blended = strength * y + (1.0 - strength) * linear_to_srgb(x);
            srgb_to_linear(y_blended)
        })
        .collect()
}

/// The DCP for a camera, parsed at most once per profile file.
///
/// Every RAW load used to re-read and re-parse the profile — including each
/// preloaded neighbour, all from the same camera. Entries are keyed by path
/// AND modification time, so dropping a corrected profile into the folder
/// still takes effect without a restart. A file that fails to parse is
/// remembered too, rather than re-failing (and re-logging) on every image.
pub fn load_profile_for_camera(make: &str, model: &str) -> Option<std::sync::Arc<DcpProfile>> {
    // The automatic choice; an image's own choice and the per-camera default
    // are applied where its GPU resources are built (`core::camera_profiles`).
    crate::core::camera_profiles::resolve(
        make,
        model,
        crate::core::camera_profiles::ProfileChoice::AUTO,
        &HashMap::new(),
    )
    .map(|(_, profile)| profile)
}

/// The DCP at `path`, parsed at most once per modification (see
/// `load_profile_for_camera`). Blocking.
pub fn load_profile(path: &Path) -> Option<std::sync::Arc<DcpProfile>> {
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::SystemTime;

    type Entry = (Option<SystemTime>, Option<Arc<DcpProfile>>);
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Entry>>> = OnceLock::new();

    let path = path.to_path_buf();
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let cache = CACHE.get_or_init(Default::default);

    if let Ok(cache) = cache.lock() {
        if let Some((stamp, profile)) = cache.get(&path) {
            if *stamp == modified {
                return profile.clone();
            }
        }
    }

    // Parsed outside the lock: two loads racing on a cold cache just parse
    // twice, which beats serialising every RAW decode behind the file read.
    let profile = match parse_dcp(&path) {
        Ok(profile) if !profile.has_forward_matrix() => {
            tracing::warn!("DCP {:?} has no ForwardMatrix; not usable by this pipeline", path);
            None
        }
        Ok(profile) => {
            tracing::info!(
                "DCP {:?} parsed: fm1={} hsm_dims={:?}",
                path,
                profile.forward_matrix_1.is_some(),
                profile.hue_sat_dims,
            );
            Some(Arc::new(profile))
        }
        Err(e) => {
            tracing::error!("DCP {:?} failed to parse: {e}", path);
            None
        }
    };
    if let Ok(mut cache) = cache.lock() {
        cache.insert(path, (modified, profile.clone()));
    }
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal little-endian DCP with these `(tag, type, payload)` entries.
    fn synthetic_dcp(entries: &[(u16, u16, Vec<u8>)]) -> std::path::PathBuf {
        let mut entries = entries.to_vec();
        entries.sort_by_key(|e| e.0);
        let ifd_len = 2 + entries.len() * 12 + 4;
        let mut out = Vec::new();
        out.extend_from_slice(b"IIRC");
        out.extend_from_slice(&8u32.to_le_bytes());
        let mut data = Vec::new();
        let mut ifd = (entries.len() as u16).to_le_bytes().to_vec();
        for (tag, typ, payload) in &entries {
            let count = payload.len() as u32 / tiff_type_size(*typ);
            ifd.extend_from_slice(&tag.to_le_bytes());
            ifd.extend_from_slice(&typ.to_le_bytes());
            ifd.extend_from_slice(&count.to_le_bytes());
            if payload.len() <= 4 {
                let mut v = payload.clone();
                v.resize(4, 0);
                ifd.extend_from_slice(&v);
            } else {
                ifd.extend_from_slice(&((8 + ifd_len + data.len()) as u32).to_le_bytes());
                data.extend_from_slice(payload);
            }
        }
        ifd.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&ifd);
        out.extend_from_slice(&data);
        let path = std::env::temp_dir().join(format!(
            "raw-editor-synthetic-{}-{}.dcp",
            std::process::id(),
            entries.iter().map(|e| e.0 as u64).sum::<u64>()
        ));
        std::fs::write(&path, out).unwrap();
        path
    }

    fn floats(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    fn longs(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Open-source profiles carry a HueSatMap and no LookTable: it must be
    /// read (it used to be ignored — and the empty table rendered black),
    /// and their identity tone curve must not count as a curve.
    #[test]
    fn hue_sat_map_profiles_parse_and_identity_curves_are_dropped() {
        let n = 2 * 2; // hue × sat × 1 value division
        let table = |v: f32| floats(&(0..n).flat_map(|_| [v, 1.0, 1.0]).collect::<Vec<_>>());
        let path = synthetic_dcp(&[
            (TAG_PROFILE_HUE_SAT_MAP_DIMS, 4, longs(&[2, 2, 1])),
            (TAG_PROFILE_HUE_SAT_MAP_DATA_1, 11, table(10.0)),
            (TAG_PROFILE_HUE_SAT_MAP_DATA_2, 11, table(20.0)),
            (TAG_PROFILE_TONE_CURVE, 11, floats(&[0.0, 0.0, 1.0, 1.0])),
        ]);
        let p = parse_dcp(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(p.hue_sat_dims, (2, 2, 1));
        assert_eq!(p.hue_sat_data_1.as_ref().map(Vec::len), Some(n));
        assert!(p.hue_sat_data_2.is_some(), "dual-illuminant table lost");
        assert!(p.tone_curve.is_none(), "an identity curve would darken by a gamma");
    }

    /// Camera-matching profiles: the LookTable wins, as it always has.
    #[test]
    fn a_look_table_takes_the_slot_over_a_hue_sat_map() {
        let path = synthetic_dcp(&[
            (TAG_PROFILE_HUE_SAT_MAP_DIMS, 4, longs(&[1, 1, 1])),
            (TAG_PROFILE_HUE_SAT_MAP_DATA_1, 11, floats(&[5.0, 1.0, 1.0])),
            (TAG_PROFILE_LOOK_TABLE_DIMS, 4, longs(&[1, 1, 1])),
            (TAG_PROFILE_LOOK_TABLE_DATA, 11, floats(&[7.0, 1.0, 1.0])),
        ]);
        let p = parse_dcp(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(p.hue_sat_data_1, Some(vec![[7.0, 1.0, 1.0]]));
    }

    /// A profile with no table at all still gives the GPU a valid texture:
    /// one identity entry, never a zero-sized table.
    #[test]
    fn a_profile_without_a_table_interpolates_to_identity() {
        let path = synthetic_dcp(&[(TAG_COLOR_MATRIX_1, 11, floats(&[1.0; 9]))]);
        let p = parse_dcp(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let i = interpolate_at_temperature(&p, 5000.0, ToneSettings::from_params(&Default::default()));
        assert_eq!(i.hue_sat_dims, (1, 1, 1));
        assert_eq!(i.hue_sat_lut, vec![[0.0, 1.0, 1.0]]);
    }

    /// A damaged profile claiming a huge tag must fail, not allocate it.
    #[test]
    fn a_tag_running_past_the_file_is_an_error() {
        let mut bytes = b"IIRC".to_vec();
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&TAG_COLOR_MATRIX_1.to_le_bytes());
        bytes.extend_from_slice(&11u16.to_le_bytes());
        bytes.extend_from_slice(&0x3000_0000u32.to_le_bytes());
        bytes.extend_from_slice(&26u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let path = std::env::temp_dir().join(format!("raw-editor-huge-{}.dcp", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let r = parse_dcp(&path);
        let _ = std::fs::remove_file(&path);
        assert!(r.is_err());
    }

    #[test]
    fn duplicate_curve_x_gives_no_nan() {
        let pts = [(0.0, 0.0), (0.5, 0.2), (0.5, 0.8), (1.0, 1.0)];
        for i in 0..=100 {
            assert!(eval_curve_points(&pts, i as f32 / 100.0).is_finite());
        }
    }

    /// A single-illuminant profile (ForwardMatrix1 only) must give the same
    /// matrix at every temperature; it used to blend against the ColorMatrix.
    #[test]
    fn single_illuminant_forward_matrix_is_temperature_independent() {
        let fm = [0.8, 0.1, 0.1, 0.2, 0.7, 0.1, 0.0, 0.1, 0.9];
        let path = synthetic_dcp(&[
            (TAG_COLOR_MATRIX_1, 11, floats(&[1.5, -0.3, -0.2, -0.4, 1.3, 0.1, -0.1, 0.2, 0.8])),
            (TAG_FORWARD_MATRIX_1, 11, floats(&fm)),
            (TAG_CALIBRATION_ILLUMINANT_1, 3, vec![17, 0]),
        ]);
        let p = parse_dcp(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(p.has_forward_matrix());
        let tone = ToneSettings::from_params(&Default::default());
        let warm = interpolate_at_temperature(&p, 3000.0, tone).camera_to_prophoto;
        let cool = interpolate_at_temperature(&p, 7000.0, tone).camera_to_prophoto;
        assert_eq!(warm, cool);
        let expect = crate::color::mat3_mul(&crate::color::XYZ_D50_TO_PROPHOTO, &fm);
        for i in 0..9 {
            assert!((warm[i] - expect[i]).abs() < 1e-5, "{i}");
        }
    }

    #[test]
    fn a_profile_without_a_forward_matrix_is_not_usable() {
        let path = synthetic_dcp(&[(TAG_COLOR_MATRIX_1, 11, floats(&[1.5, -0.3, -0.2, -0.4, 1.3, 0.1, -0.1, 0.2, 0.8]))]);
        let p = parse_dcp(&path).unwrap();
        assert!(!p.has_forward_matrix());
        assert!(load_profile(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }

    /// Parse every DCP installed in the user profiles dir (skips when none).
    /// Guards against regressions like rejecting the 0x4352 "CR" magic that
    /// all genuine Adobe profiles use.
    #[test]
    fn parse_installed_profiles() {
        let base = crate::core::paths::profiles_dir();

        let Ok(entries) = std::fs::read_dir(&base) else {
            eprintln!("no profiles dir at {base:?}, skipping");
            return;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dcp")) {
                let profile = parse_dcp(&path)
                    .unwrap_or_else(|e| panic!("failed to parse {path:?}: {e}"));
                // An Adobe camera profile must carry at least one color matrix
                assert_ne!(
                    profile.color_matrix_1,
                    [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                    "{path:?}: ColorMatrix1 missing (identity fallback)"
                );
                // If a HueSatMap is declared, the data length must match the dims
                let (h, s, v) = profile.hue_sat_dims;
                if let Some(lut) = &profile.hue_sat_data_1 {
                    assert_eq!(lut.len(), (h * s * v) as usize, "{path:?}: HSM size mismatch");
                }
                eprintln!(
                    "parsed {path:?}: fm1={} hsm_dims={:?} curve_pts={:?}",
                    profile.forward_matrix_1.is_some(),
                    profile.hue_sat_dims,
                    profile.tone_curve.as_ref().map(|c| c.len())
                );
            }
        }
    }
}
#[cfg(test)]
mod curve_debug {
    use super::*;

    #[test]
    fn dump_installed_curve() {
        let base = crate::core::paths::profiles_dir();
        let Ok(entries) = std::fs::read_dir(&base) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "dcp").unwrap_or(false) {
                let prof = parse_dcp(&p).expect("parse");
                println!("profile: {:?}", p.file_name().unwrap());
                if let Some(pts) = &prof.tone_curve {
                    println!("curve points: {}", pts.len());
                    for probe in [0.02f32, 0.05, 0.10, 0.18, 0.30, 0.50, 0.70, 0.90, 1.00] {
                        // linear interp over points
                        let y = if probe <= pts[0].0 { pts[0].1 }
                        else if probe >= pts.last().unwrap().0 { pts.last().unwrap().1 }
                        else {
                            pts.windows(2).find(|w| probe >= w[0].0 && probe <= w[1].0)
                                .map(|w| { let t = (probe - w[0].0)/(w[1].0-w[0].0); w[0].1*(1.0-t)+w[1].1*t })
                                .unwrap()
                        };
                        println!("  curve({probe:.2}) = {y:.4}");
                    }
                } else {
                    println!("NO embedded tone curve");
                }
                println!("hsm dims: {:?}, hsm1: {:?}, hsm2: {:?}",
                    prof.hue_sat_dims,
                    prof.hue_sat_data_1.as_ref().map(|v| v.len()),
                    prof.hue_sat_data_2.as_ref().map(|v| v.len()));
            }
        }
    }
}
