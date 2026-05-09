use std::fs::File;
use std::path::{Path, PathBuf};
use tiff::decoder::{Decoder, ifd::Value};
use tiff::tags::Tag;
use std::collections::HashMap;

/// Parsed representation of a DCP (DNG Camera Profile) file
#[derive(Debug, Clone)]
pub struct DcpProfile {
    pub illuminant1: u32,
    pub illuminant2: u32,
    pub color_matrix_1: [f32; 9],
    pub color_matrix_2: [f32; 9],
    pub forward_matrix_1: Option<[f32; 9]>,
    pub forward_matrix_2: Option<[f32; 9]>,
    pub hue_sat_dims: (u32, u32, u32), // H, S, V divisions
    pub hue_sat_data_1: Option<Vec<[f32; 3]>>, // (hue_shift, sat_scale, val_scale)
    pub hue_sat_data_2: Option<Vec<[f32; 3]>>,
    pub tone_curve: Option<Vec<(f32, f32)>>, // ProfileToneCurve control points
}

/// Blended profile data ready for GPU upload
#[derive(Debug, Clone)]
pub struct InterpolatedProfile {
    pub forward_matrix: [f32; 9],   // Blended ForwardMatrix (CamRGB→XYZ)
    pub hue_sat_lut: Vec<[f32; 3]>, // Blended 3D HSV LUT
    pub tone_curve: Vec<f32>,       // 1024-sample spline-interpolated curve
    pub hue_sat_dims: (u32, u32, u32),
}

// TIFF tags for DCP
const TAG_CALIBRATION_ILLUMINANT_1: u16 = 50778;
const TAG_CALIBRATION_ILLUMINANT_2: u16 = 50779;
const TAG_COLOR_MATRIX_1: u16 = 50721;
const TAG_COLOR_MATRIX_2: u16 = 50722;
const TAG_FORWARD_MATRIX_1: u16 = 50964;
const TAG_FORWARD_MATRIX_2: u16 = 50965;
const TAG_PROFILE_HUE_SAT_MAP_DIMS: u16 = 50937;
const TAG_PROFILE_HUE_SAT_MAP_DATA_1: u16 = 50938;
const TAG_PROFILE_HUE_SAT_MAP_DATA_2: u16 = 50939;
const TAG_PROFILE_TONE_CURVE: u16 = 50940;

/// Parses a DCP file
pub fn parse_dcp(path: &Path) -> Result<DcpProfile, String> {
    let file = File::open(path).map_err(|e| format!("Failed to open DCP: {}", e))?;
    let mut decoder = Decoder::new(file).map_err(|e| format!("Failed to create TIFF decoder: {}", e))?;
    

    // TIFF decoder provides high level access, but for generic tags we might need to find them directly
    // Let's iterate over the IFD entries
    // Unfortunately, the `tiff` crate might not expose an easy way to get an arbitrary tag by ID directly
    // Let's try to get them using `get_tag` if it supports custom tags, or just get all entries if possible
    // Wait, Decoder has `get_tag_u32`, `get_tag_f32_vec`, etc., but for custom tags we need `find_tag`.
    // Actually, `Decoder` doesn't have `find_tag`. It has `get_tag` which takes a `Tag` enum.
    // If the tag is unknown, `Tag::Unknown(id)` can be used.
    
    let get_floats = |decoder: &mut Decoder<File>, id: u16| -> Option<Vec<f32>> {
        match decoder.get_tag(Tag::Unknown(id)) {
            Ok(Value::Float(f)) => Some(vec![f]),
            Ok(Value::Rational(n, d)) => Some(vec![n as f32 / d as f32]),
            Ok(Value::SRational(n, d)) => Some(vec![n as f32 / d as f32]),
            Ok(Value::List(vec)) => {
                let mut res = Vec::new();
                for item in vec {
                    match item {
                        Value::Float(f) => res.push(f),
                        Value::Rational(n, d) => res.push(n as f32 / d as f32),
                        Value::SRational(n, d) => res.push(n as f32 / d as f32),
                        Value::Unsigned(v) => res.push(v as f32),
                        Value::Short(v) => res.push(v as f32),
                        _ => {}
                    }
                }
                if res.is_empty() { None } else { Some(res) }
            },
            _ => None,
        }
    };

    let get_u32 = |decoder: &mut Decoder<File>, id: u16| -> Option<u32> {
        match decoder.get_tag(Tag::Unknown(id)) {
            Ok(Value::Short(v)) => Some(v as u32),
            Ok(Value::Unsigned(v)) => Some(v),
            Ok(Value::List(vec)) => {
                match vec.first() {
                    Some(Value::Short(v)) => Some(*v as u32),
                    Some(Value::Unsigned(v)) => Some(*v),
                    _ => None
                }
            },
            _ => None,
        }
    };
    
    let get_u32_vec = |decoder: &mut Decoder<File>, id: u16| -> Option<Vec<u32>> {
         match decoder.get_tag(Tag::Unknown(id)) {
            Ok(Value::Short(v)) => Some(vec![v as u32]),
            Ok(Value::Unsigned(v)) => Some(vec![v]),
            Ok(Value::List(vec)) => {
                let mut res = Vec::new();
                for item in vec {
                    match item {
                        Value::Short(v) => res.push(v as u32),
                        Value::Unsigned(v) => res.push(v),
                        _ => {}
                    }
                }
                Some(res)
            },
            _ => None,
        }
    };

    let illuminant1 = get_u32(&mut decoder, TAG_CALIBRATION_ILLUMINANT_1).unwrap_or(17); // Default to StdA
    let illuminant2 = get_u32(&mut decoder, TAG_CALIBRATION_ILLUMINANT_2).unwrap_or(21); // Default to D65

    let cm1_floats = get_floats(&mut decoder, TAG_COLOR_MATRIX_1).unwrap_or_else(|| vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    let cm2_floats = get_floats(&mut decoder, TAG_COLOR_MATRIX_2).unwrap_or_else(|| cm1_floats.clone());

    let mut color_matrix_1 = [0.0; 9];
    let mut color_matrix_2 = [0.0; 9];
    for i in 0..9 {
        if i < cm1_floats.len() { color_matrix_1[i] = cm1_floats[i]; }
        if i < cm2_floats.len() { color_matrix_2[i] = cm2_floats[i]; }
    }

    let fm1_floats = get_floats(&mut decoder, TAG_FORWARD_MATRIX_1);
    let fm2_floats = get_floats(&mut decoder, TAG_FORWARD_MATRIX_2);

    let forward_matrix_1 = fm1_floats.map(|v| {
        let mut m = [0.0; 9];
        for i in 0..9 { if i < v.len() { m[i] = v[i]; } }
        m
    });

    let forward_matrix_2 = fm2_floats.map(|v| {
        let mut m = [0.0; 9];
        for i in 0..9 { if i < v.len() { m[i] = v[i]; } }
        m
    });

    let dims_vec = get_u32_vec(&mut decoder, TAG_PROFILE_HUE_SAT_MAP_DIMS).unwrap_or_else(|| vec![0, 0, 0]);
    let hue_sat_dims = (
        *dims_vec.get(0).unwrap_or(&0),
        *dims_vec.get(1).unwrap_or(&0),
        *dims_vec.get(2).unwrap_or(&0)
    );

    let hue_sat_data_1 = get_floats(&mut decoder, TAG_PROFILE_HUE_SAT_MAP_DATA_1).map(|v| {
        let mut res = Vec::new();
        for chunk in v.chunks_exact(3) {
            res.push([chunk[0], chunk[1], chunk[2]]);
        }
        res
    });

    let hue_sat_data_2 = get_floats(&mut decoder, TAG_PROFILE_HUE_SAT_MAP_DATA_2).map(|v| {
        let mut res = Vec::new();
        for chunk in v.chunks_exact(3) {
            res.push([chunk[0], chunk[1], chunk[2]]);
        }
        res
    });

    let tone_curve = get_floats(&mut decoder, TAG_PROFILE_TONE_CURVE).map(|v| {
        let mut res = Vec::new();
        for chunk in v.chunks_exact(2) {
            res.push((chunk[0], chunk[1]));
        }
        res
    });

    Ok(DcpProfile {
        illuminant1,
        illuminant2,
        color_matrix_1,
        color_matrix_2,
        forward_matrix_1,
        forward_matrix_2,
        hue_sat_dims,
        hue_sat_data_1,
        hue_sat_data_2,
        tone_curve,
    })
}

/// Temperature of an illuminant in Kelvin
pub fn illuminant_to_kelvin(ill: u32) -> f32 {
    match ill {
        17 => 2850.0, // Standard Light A
        21 => 6500.0, // D65
        18 => 4800.0, // Standard Light B
        19 => 6774.0, // Standard Light C
        20 => 5500.0, // D55
        22 => 7500.0, // D75
        23 => 5000.0, // D50
        _ => 5000.0,
    }
}

/// Interpolates the matrices and LUTs based on temperature
pub fn interpolate_at_temperature(profile: &DcpProfile, kelvin: f32) -> InterpolatedProfile {
    let t1 = illuminant_to_kelvin(profile.illuminant1);
    let t2 = illuminant_to_kelvin(profile.illuminant2);

    let inv_t = 1.0 / kelvin.max(1000.0);
    let inv_t1 = 1.0 / t1;
    let inv_t2 = 1.0 / t2;

    let weight = if (inv_t2 - inv_t1).abs() < 1e-6 {
        0.0 // Avoid division by zero if both illuminants are the same
    } else {
        ((inv_t - inv_t1) / (inv_t2 - inv_t1)).clamp(0.0, 1.0)
    };

    // Interpolate ForwardMatrix. Fallback to ColorMatrix if missing.
    let fm1 = profile.forward_matrix_1.unwrap_or(profile.color_matrix_1);
    let fm2 = profile.forward_matrix_2.unwrap_or(profile.color_matrix_2);
    
    let mut forward_matrix = [0.0; 9];
    for i in 0..9 {
        forward_matrix[i] = fm1[i] * (1.0 - weight) + fm2[i] * weight;
    }

    // Interpolate 3D LUT
    let mut hue_sat_lut = Vec::new();
    let num_elements = profile.hue_sat_dims.0 * profile.hue_sat_dims.1 * profile.hue_sat_dims.2;
    if num_elements > 0 {
        if let (Some(lut1), Some(lut2)) = (&profile.hue_sat_data_1, &profile.hue_sat_data_2) {
            for i in 0..num_elements as usize {
                let mut v = [0.0; 3];
                let v1 = lut1.get(i).unwrap_or(&[0.0, 1.0, 1.0]);
                let v2 = lut2.get(i).unwrap_or(&[0.0, 1.0, 1.0]);
                v[0] = v1[0] * (1.0 - weight) + v2[0] * weight;
                v[1] = v1[1] * (1.0 - weight) + v2[1] * weight;
                v[2] = v1[2] * (1.0 - weight) + v2[2] * weight;
                hue_sat_lut.push(v);
            }
        } else if let Some(lut) = profile.hue_sat_data_1.as_ref().or(profile.hue_sat_data_2.as_ref()) {
             hue_sat_lut = lut.clone();
        }
    }

    // Bake tone curve
    let baked_curve = if let Some(pts) = &profile.tone_curve {
        bake_tone_curve(pts)
    } else {
        // Linear fallback
        (0..1024).map(|i| i as f32 / 1023.0).collect()
    };

    InterpolatedProfile {
        forward_matrix,
        hue_sat_lut,
        tone_curve: baked_curve,
        hue_sat_dims: profile.hue_sat_dims,
    }
}

/// Bakes ProfileToneCurve control points into a 1024-sample curve using linear interpolation.
/// A full cubic spline could be implemented here, but linear interpolation of the
/// fine control points in a DCP is often sufficient as they are densely packed.
pub fn bake_tone_curve(points: &[(f32, f32)]) -> Vec<f32> {
    let mut curve = vec![0.0; 1024];
    if points.is_empty() {
        for (i, v) in curve.iter_mut().enumerate() {
            *v = i as f32 / 1023.0;
        }
        return curve;
    }

    for i in 0..1024 {
        let x = i as f32 / 1023.0;
        
        // Find segment
        if x <= points[0].0 {
            curve[i] = points[0].1;
        } else if x >= points.last().unwrap().0 {
            curve[i] = points.last().unwrap().1;
        } else {
            for j in 0..points.len() - 1 {
                let p1 = points[j];
                let p2 = points[j + 1];
                if x >= p1.0 && x <= p2.0 {
                    let t = (x - p1.0) / (p2.0 - p1.0);
                    curve[i] = p1.1 * (1.0 - t) + p2.1 * t;
                    break;
                }
            }
        }
    }
    curve
}

/// Find a profile for a camera model
pub fn find_profile_for_camera(_make: &str, model: &str) -> Option<PathBuf> {
    // Phase 1: Look in ~/.local/share/raw-editor/profiles/
    let mut path = dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    path.push("raw-editor");
    path.push("profiles");
    
    // Normalize model name (remove spaces, lowercase)
    // Try to find a match. For now, simple exact match or basic search
    let safe_model = model.replace(" ", "_").to_lowercase();
    
    if let Ok(entries) = std::fs::read_dir(&path) {
        for entry in entries.flatten() {
            let filename = entry.file_name().to_string_lossy().to_lowercase();
            if filename.ends_with(".dcp") && filename.contains(&safe_model) {
                return Some(entry.path());
            }
            // Also try without spaces
            let filename_no_spaces = filename.replace(" ", "");
            if filename_no_spaces.ends_with(".dcp") && filename_no_spaces.contains(&model.replace(" ", "").to_lowercase()) {
                return Some(entry.path());
            }
        }
    }

    None
}
