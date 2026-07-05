// src/raw/dcp.rs

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
const TAG_PROFILE_HUE_SAT_MAP_DIMS:   u16 = 50981; // WAS WRONG: was 50937
const TAG_PROFILE_HUE_SAT_MAP_DATA_1: u16 = 50982; // WAS WRONG: was 50938
const TAG_PROFILE_HUE_SAT_MAP_DATA_2: u16 = 50983; // WAS WRONG: was 50939
const TAG_PROFILE_TONE_CURVE:         u16 = 50940;
const TAG_BASELINE_EXPOSURE_OFFSET:   u16 = 51109;
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
    count: u32,
    data:  Vec<u8>,
}

fn read_ifd(path: &Path) -> Result<HashMap<u16, RawTag>, String> {
    let mut f = std::fs::File::open(path)
        .map_err(|e| format!("Cannot open DCP: {e}"))?;

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
            let saved = f.stream_position().map_err(|e| e.to_string())?;
            f.seek(SeekFrom::Start(offset as u64))
                .map_err(|e| format!("Seek data tag={tag}: {e}"))?;
            let mut buf = vec![0u8; total as usize];
            f.read_exact(&mut buf)
                .map_err(|e| format!("Read data tag={tag}: {e}"))?;
            f.seek(SeekFrom::Start(saved)).map_err(|e| e.to_string())?;
            buf
        };

        map.insert(tag, RawTag { typ, count, data });
    }

    Ok(map)
}

// ── Helpers for reading typed tag data ──────────────────────────────────────

fn tag_floats(tag: &RawTag, le: bool) -> Vec<f32> {
    match tag.typ {
        11 => { // FLOAT
            tag.data.chunks(4).map(|c| {
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
}

#[derive(Debug, Clone)]
pub struct InterpolatedProfile {
    pub forward_matrix: [f32; 9],
    pub hue_sat_lut: Vec<[f32; 3]>,
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
        .unwrap_or(21);

    let hue_sat_dims = map.get(&TAG_PROFILE_HUE_SAT_MAP_DIMS)
        .map(|t| { let v = tag_u32s(t, le); (v[0], v[1], v[2]) })
        .unwrap_or((0, 0, 0));

    let parse_hsm = |tag: &RawTag| -> Vec<[f32; 3]> {
        tag_floats(tag, le).chunks(3)
            .filter(|c| c.len() == 3)
            .map(|c| [c[0], c[1], c[2]])
            .collect()
    };
    let hue_sat_data_1 = map.get(&TAG_PROFILE_HUE_SAT_MAP_DATA_1).map(parse_hsm);
    let hue_sat_data_2 = map.get(&TAG_PROFILE_HUE_SAT_MAP_DATA_2).map(parse_hsm);

    let baseline_exposure_offset = map.get(&TAG_BASELINE_EXPOSURE_OFFSET)
        .and_then(|t| tag_floats(t, le).into_iter().next())
        .unwrap_or(0.0);

    let tone_curve = map.get(&TAG_PROFILE_TONE_CURVE).map(|t| {
        tag_floats(t, le).chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| (c[0], c[1]))
            .collect()
    });

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
        _  => 5000.0,
    }
}

pub fn interpolate_at_temperature(
    profile: &DcpProfile,
    kelvin: f32,
    curve_strength: f32,
) -> InterpolatedProfile {
    let t1 = illuminant_to_kelvin(profile.illuminant1);
    let t2 = illuminant_to_kelvin(profile.illuminant2);

    let inv_t  = 1.0 / kelvin.max(1000.0);
    let inv_t1 = 1.0 / t1;
    let inv_t2 = 1.0 / t2;

    let weight = if (inv_t2 - inv_t1).abs() < 1e-6 {
        0.0
    } else {
        ((inv_t - inv_t1) / (inv_t2 - inv_t1)).clamp(0.0, 1.0)
    };

    let fm1 = profile.forward_matrix_1.unwrap_or(profile.color_matrix_1);
    let fm2 = profile.forward_matrix_2.unwrap_or(profile.color_matrix_2);

    // Fold BaselineExposureOffset into the forward matrix: scaling camera→XYZ
    // uniformly is an exposure shift applied before the HueSatMap and tone
    // curve — where Adobe applies it. This is what makes the default render
    // match the in-camera JPEG brightness (-0.35 EV for this profile family).
    let baseline_gain = 2.0f32.powf(profile.baseline_exposure_offset);
    let mut forward_matrix = [0.0f32; 9];
    for i in 0..9 {
        forward_matrix[i] = (fm1[i] * (1.0 - weight) + fm2[i] * weight) * baseline_gain;
    }

    let num_elements = (profile.hue_sat_dims.0 * profile.hue_sat_dims.1 * profile.hue_sat_dims.2) as usize;
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

    let has_tone_curve = profile.tone_curve.is_some();
    let baked_curve = profile.tone_curve.as_deref()
        .map(|pts| bake_tone_curve(pts, curve_strength))
        .unwrap_or_else(|| (0..1024).map(|i| i as f32 / 1023.0).collect());

    InterpolatedProfile {
        forward_matrix,
        hue_sat_lut,
        tone_curve: baked_curve,
        hue_sat_dims: profile.hue_sat_dims,
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

/// Bake the DCP ProfileToneCurve into a 1024-entry linear-output LUT.
///
/// `strength` blends the profile curve against a flat rendering, computed in
/// display space where the curve is defined: 1.0 = full profile curve,
/// 0.0 = plain sRGB gamma (no added contrast).
pub fn bake_tone_curve(points: &[(f32, f32)], strength: f32) -> Vec<f32> {
    if points.is_empty() {
        return (0..1024).map(|i| i as f32 / 1023.0).collect();
    }
    let strength = strength.clamp(0.0, 1.0);
    // Adobe ProfileToneCurve points map LINEAR input → GAMMA-ENCODED (display)
    // output — the curve embeds the ~2.2 display gamma plus the profile's
    // contrast s-curve (e.g. the ACR default maps 0.18 → ≈0.48 ≈ 0.18^(1/2.2)).
    // Our shader keeps working in linear space after the curve and applies the
    // sRGB transfer function at the very end, so bake the LUT with the curve's
    // output linearised.  At default settings the final encode then reproduces
    // the profile's intended rendering exactly; applying the raw curve values
    // instead double-gammas the image (~+1.3 EV apparent overexposure).
    (0..1024usize).map(|i| {
        let x = i as f32 / 1023.0;
        let y = if x <= points[0].0 {
            points[0].1
        } else if x >= points.last().unwrap().0 {
            points.last().unwrap().1
        } else {
            points.windows(2)
                .find(|w| x >= w[0].0 && x <= w[1].0)
                .map(|w| {
                    let t = (x - w[0].0) / (w[1].0 - w[0].0);
                    w[0].1 * (1.0 - t) + w[1].1 * t
                })
                .unwrap_or(x)
        };
        // Blend toward "no curve" (= plain sRGB gamma) in display space.
        let y_blended = strength * y + (1.0 - strength) * linear_to_srgb(x);
        srgb_to_linear(y_blended)
    }).collect()
}

pub fn find_profile_for_camera(make: &str, model: &str) -> Option<PathBuf> {
    let base = dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("raw-editor")
        .join("profiles");

    let model_lower = model.to_lowercase();
    let model_nospace = model_lower.replace(' ', "");

    let mut matches = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().to_lowercase();
            let fname_nospace = fname.replace(' ', "");
            if fname.ends_with(".dcp")
                && (fname.contains(&model_lower) || fname_nospace.contains(&model_nospace))
            {
                matches.push(entry.path());
            }
        }
    }

    matches.iter()
        .find(|p| p.file_name().unwrap_or_default().to_string_lossy().to_lowercase().contains("adobe standard"))
        .or_else(|| matches.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse every DCP installed in the user profiles dir (skips when none).
    /// Guards against regressions like rejecting the 0x4352 "CR" magic that
    /// all genuine Adobe profiles use.
    #[test]
    fn parse_installed_profiles() {
        let base = dirs::data_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("raw-editor")
            .join("profiles");

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
        let base = dirs::data_dir().unwrap().join("raw-editor").join("profiles");
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
