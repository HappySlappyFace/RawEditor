/// Color space conversion utilities
///
/// This module handles conversion between different color spaces:
/// - Camera RGB (sensor-native color space)
/// - XYZ (device-independent color space)
/// - sRGB (standard display color space)
use cgmath::{Matrix3, SquareMatrix};

/// XYZ D50 → ProPhoto RGB (ROMM), **row-major**.
///
/// ProPhoto is itself a D50 space, so this is a pure change of primaries with
/// no chromatic adaptation.
///
/// Row-major to match the convention every `[f32; 9]` matrix in this codebase
/// uses — the shader rebuilds them with `transpose(mat3x3(row0, row1, row2))`.
/// The same numbers appear column-major inside WGSL literals, because
/// `mat3x3<f32>(a, b, c)` takes columns; do not copy them between the two
/// without transposing.
pub const XYZ_D50_TO_PROPHOTO: [f32; 9] = [
    1.3459433, -0.2556075, -0.0511118,
    -0.5445989, 1.5081673, 0.0205351,
    0.0000000, 0.0000000, 1.2118128,
];

/// Row-major 3×3 matrix product, `a × b`.
///
/// Accumulates in f64 and narrows once at the end: the result is folded into a
/// matrix the GPU then applies per pixel, so it is worth being at least as
/// accurate as the two sequential f32 multiplies it replaces.
///
/// Deliberately not `cgmath::Matrix3`, which is column-major — mixing the two
/// conventions is the one mistake a fold like this is likely to make.
pub fn mat3_mul(a: &[f32; 9], b: &[f32; 9]) -> [f32; 9] {
    let mut out = [0.0f32; 9];
    for row in 0..3 {
        for col in 0..3 {
            let mut acc = 0.0f64;
            for k in 0..3 {
                acc += a[row * 3 + k] as f64 * b[k * 3 + col] as f64;
            }
            out[row * 3 + col] = acc as f32;
        }
    }
    out
}

/// Apply a row-major 3×3 to a colour triple. Test/utility helper.
pub fn mat3_apply(m: &[f32; 9], v: [f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
        m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
        m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
    ]
}

/// Calculate the camera-to-sRGB colour conversion matrix.
///
/// The WGSL shader pre-multiplies each camera pixel by the WB multipliers before
/// applying this matrix. To keep the two stages independent, we embed the inverse
/// of those WB gains as a diagonal matrix at the right side of the chain:
///
///   M_final = XYZ_sRGB × Bradford × Cam_to_XYZ × diag(1/wb)
///
/// When the shader executes `M_final × (raw × WB)`, the `diag(1/wb) × diag(WB)`
/// cancels to identity, leaving the mathematically correct `Cam_to_sRGB × raw`.
///
/// # Arguments
/// * `raw_matrix`     - The camera's XYZ-to-Camera matrix from RAW metadata (row-major [f32;9])
/// * `wb_multipliers` - The as-shot WB gains `[R, G, B, G2]` normalised so G = 1.0
/// * `matrix_is_d65`  - True when `raw_matrix` is D65-referenced. Applying the
///   Bradford D50→D65 adaptation to a matrix that already produces D65-referenced
///   XYZ double-adapts and shifts every colour blue/cool, so it must be skipped.
///
/// # Returns
/// * Camera-to-sRGB matrix as a flat `[f32; 9]` array (row-major)
pub fn calculate_cam_to_srgb(raw_matrix: [f32; 9], wb_multipliers: [f32; 4], matrix_is_d65: bool) -> [f32; 9] {
    crate::debug_log!(
        crate::debug::DEBUG_APP,
        "🎨 Phase 48: Calculating Cam-to-sRGB with Bradford Adaptation..."
    );

    // Step 1: Load xyz_to_cam matrix (Camera's XYZ -> Cam conversion in D50)
    // Input is row-major [r0c0, r0c1, r0c2, r1c0, ...], cgmath needs column-major
    let xyz_to_cam = Matrix3::new(
        raw_matrix[0],
        raw_matrix[3],
        raw_matrix[6], // Column 0
        raw_matrix[1],
        raw_matrix[4],
        raw_matrix[7], // Column 1
        raw_matrix[2],
        raw_matrix[5],
        raw_matrix[8], // Column 2
    );

    // Step 2: Invert to get Cam -> XYZ_D50
    let cam_to_xyz_d50 = match xyz_to_cam.invert() {
        Some(m) => m,
        None => {
            crate::debug_log!(
                crate::debug::DEBUG_APP,
                "⚠️ Failed to invert XYZ-to-Cam matrix, using identity"
            );
            return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        }
    };

    // Step 2b: Phase 126 — inverse-WB diagonal.
    // The shader does `color * wb_multipliers` BEFORE the matrix multiply.
    // We pre-bake diag(1/wb) into the matrix so that:
    //   M × (raw × WB) = (XYZ_sRGB × Bradford × Cam_to_XYZ × diag(1/WB)) × (raw × WB)
    //                  = XYZ_sRGB × Bradford × Cam_to_XYZ × raw   ← correct
    // cgmath Matrix3::new is column-major: (c0r0, c0r1, c0r2,  c1r0, c1r1, c1r2,  c2r0, c2r1, c2r2)
    let r_inv = 1.0 / wb_multipliers[0].max(1e-6);
    let g_inv = 1.0 / wb_multipliers[1].max(1e-6);
    let b_inv = 1.0 / wb_multipliers[2].max(1e-6);
    let un_wb = Matrix3::new(
        r_inv, 0.0,   0.0,   // Column 0
        0.0,   g_inv, 0.0,   // Column 1
        0.0,   0.0,   b_inv, // Column 2
    );

    // Step 3: Bradford Chromatic Adaptation (D50 -> D65)
    // This is the KEY to preventing green/pink tints!
    // Standard Bradford matrix from ICC profile specifications
    #[rustfmt::skip]
    const BRADFORD_D50_TO_D65: Matrix3<f32> = Matrix3::new(
         0.9555766, -0.0282895,  0.0122982, // Column 0
        -0.0230393,  1.0099416, -0.0204830, // Column 1
         0.0631636,  0.0210077,  1.3299098, // Column 2
    );

    // Step 4: XYZ (D65) to sRGB matrix
    // Standard sRGB transformation matrix
    #[rustfmt::skip]
    const XYZ_TO_SRGB: Matrix3<f32> = Matrix3::new(
         3.2404542, -0.969_266,  0.0556434, // Column 0
        -1.5371385,  1.8760108, -0.2040259, // Column 1
        -0.4985314,  0.0415560,  1.0572252, // Column 2
    );

    // Step 5: Chain transformations: Cam -> XYZ -> (adapt if needed) -> sRGB
    // Bradford only when the matrix is D50-referenced; D65 matrices are already
    // in the display white point.
    let adaptation = if matrix_is_d65 {
        Matrix3::from_value(1.0) // identity — already D65
    } else {
        BRADFORD_D50_TO_D65
    };
    let mut final_matrix = XYZ_TO_SRGB * adaptation * cam_to_xyz_d50 * un_wb;

    // Step 6: CRITICAL - Normalize rows to prevent pink tint!
    // Each row sum should equal 1.0 so that neutral (1,1,1) -> (1,1,1)
    // This prevents highlights from clipping incorrectly

    // Row 0: (c0.x, c1.x, c2.x)
    let r0_sum = final_matrix.x.x + final_matrix.y.x + final_matrix.z.x;
    // Row 1: (c0.y, c1.y, c2.y)
    let r1_sum = final_matrix.x.y + final_matrix.y.y + final_matrix.z.y;
    // Row 2: (c0.z, c1.z, c2.z)
    let r2_sum = final_matrix.x.z + final_matrix.y.z + final_matrix.z.z;

    // Normalize each row (avoid division by zero)
    if r0_sum.abs() > 1e-6 {
        final_matrix.x.x /= r0_sum;
        final_matrix.y.x /= r0_sum;
        final_matrix.z.x /= r0_sum;
    }
    if r1_sum.abs() > 1e-6 {
        final_matrix.x.y /= r1_sum;
        final_matrix.y.y /= r1_sum;
        final_matrix.z.y /= r1_sum;
    }
    if r2_sum.abs() > 1e-6 {
        final_matrix.x.z /= r2_sum;
        final_matrix.y.z /= r2_sum;
        final_matrix.z.z /= r2_sum;
    }

    // Convert back to flat row-major array [r0c0, r0c1, r0c2, r1c0...]
    let result = [
        final_matrix.x.x,
        final_matrix.y.x,
        final_matrix.z.x, // Row 0
        final_matrix.x.y,
        final_matrix.y.y,
        final_matrix.z.y, // Row 1
        final_matrix.x.z,
        final_matrix.y.z,
        final_matrix.z.z, // Row 2
    ];

    crate::debug_log!(
        crate::debug::DEBUG_APP,
        "✅ Bradford-adapted Cam-to-sRGB matrix calculated"
    );
    crate::debug_log!(
        crate::debug::DEBUG_APP,
        "   Row sums: {:.4}, {:.4}, {:.4}",
        r0_sum,
        r1_sum,
        r2_sum
    );

    result
}

// ═══════════════════════════════════════════════════════════════════════════
// Correlated Color Temperature (CCT) ↔ chromaticity — Robertson's method.
// Port of Adobe's dng_temperature.cpp so our Kelvin/tint numbers line up with
// what Lightroom/ACR display for the same raw file.
// ═══════════════════════════════════════════════════════════════════════════

/// Robertson (1968) isotherm table: (mired, CIE-1960 u, v, isotherm slope).
/// Identical to dng_sdk's kTempTable.
#[rustfmt::skip]
const ROBERTSON_TABLE: [(f32, f32, f32, f32); 31] = [
    (   0.0, 0.18006, 0.26352,  -0.24341),
    (  10.0, 0.18066, 0.26589,  -0.25479),
    (  20.0, 0.18133, 0.26846,  -0.26876),
    (  30.0, 0.18208, 0.27119,  -0.28539),
    (  40.0, 0.18293, 0.27407,  -0.30470),
    (  50.0, 0.18388, 0.27709,  -0.32675),
    (  60.0, 0.18494, 0.28021,  -0.35156),
    (  70.0, 0.18611, 0.28342,  -0.37915),
    (  80.0, 0.18740, 0.28668,  -0.40955),
    (  90.0, 0.18880, 0.28997,  -0.44278),
    ( 100.0, 0.19032, 0.29326,  -0.47888),
    ( 125.0, 0.19462, 0.30141,  -0.58204),
    ( 150.0, 0.19962, 0.30921,  -0.70471),
    ( 175.0, 0.20525, 0.31647,  -0.84901),
    ( 200.0, 0.21142, 0.32312,  -1.0182 ),
    ( 225.0, 0.21807, 0.32909,  -1.2168 ),
    ( 250.0, 0.22511, 0.33439,  -1.4512 ),
    ( 275.0, 0.23247, 0.33904,  -1.7298 ),
    ( 300.0, 0.24010, 0.34308,  -2.0637 ),
    ( 325.0, 0.24792, 0.34655,  -2.4681 ),
    ( 350.0, 0.25591, 0.34951,  -2.9641 ),
    ( 375.0, 0.26400, 0.35200,  -3.5814 ),
    ( 400.0, 0.27218, 0.35407,  -4.3633 ),
    ( 425.0, 0.28039, 0.35577,  -5.3762 ),
    ( 450.0, 0.28863, 0.35714,  -6.7262 ),
    ( 475.0, 0.29685, 0.35823,  -8.5955 ),
    ( 500.0, 0.30505, 0.35907, -11.324  ),
    ( 525.0, 0.31320, 0.35968, -15.628  ),
    ( 550.0, 0.32129, 0.36011, -23.325  ),
    ( 575.0, 0.32931, 0.36038, -40.770  ),
    ( 600.0, 0.33724, 0.36051, -116.45  ),
];

/// Adobe's tint scale: tint units per unit uv distance along the isotherm.
/// Negative so that positive tint = magenta shift, matching the ACR slider.
const TINT_SCALE: f32 = -3000.0;

/// CIE 1931 xy → CIE 1960 uv.
fn xy_to_uv(x: f32, y: f32) -> (f32, f32) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / d, 6.0 * y / d)
}

/// CIE 1960 uv → CIE 1931 xy.
fn uv_to_xy(u: f32, v: f32) -> (f32, f32) {
    let d = 2.0 + u - 4.0 * v;
    (1.5 * u / d, v / d)
}

/// Chromaticity → (correlated color temperature in Kelvin, Adobe-scale tint).
/// Port of dng_temperature::Set_xy_coord.
pub fn xy_to_kelvin_tint(x: f32, y: f32) -> (f32, f32) {
    let (u, v) = xy_to_uv(x, y);

    let mut last_dt = 0.0f32;
    let mut last_dv = 0.0f32;
    let mut last_du = 0.0f32;
    let mut kelvin = 6500.0f32;
    let mut tint = 0.0f32;

    for index in 1..31 {
        // Normalized direction of this row's isotherm.
        let mut du = 1.0f32;
        let mut dv = ROBERTSON_TABLE[index].3;
        let len = (1.0 + dv * dv).sqrt();
        du /= len;
        dv /= len;

        // Signed perpendicular distance of the sample from this isotherm.
        let uu = u - ROBERTSON_TABLE[index].1;
        let vv = v - ROBERTSON_TABLE[index].2;
        let mut dt = -uu * dv + vv * du;

        if dt <= 0.0 || index == 30 {
            dt = -dt.min(0.0);

            // Interpolation factor between this row and the previous one.
            let f = if index == 1 { 0.0 } else { dt / (last_dt + dt) };

            // Interpolate the mired value, convert to Kelvin.
            let mired = ROBERTSON_TABLE[index].0 * (1.0 - f) + ROBERTSON_TABLE[index - 1].0 * f;
            kelvin = if mired > 0.0 { 1.0e6 / mired } else { 100_000.0 };

            // Project the offset from the locus onto the isotherm for tint.
            let uu2 = u - (ROBERTSON_TABLE[index].1 * (1.0 - f) + ROBERTSON_TABLE[index - 1].1 * f);
            let vv2 = v - (ROBERTSON_TABLE[index].2 * (1.0 - f) + ROBERTSON_TABLE[index - 1].2 * f);
            let mut du2 = du * (1.0 - f) + last_du * f;
            let mut dv2 = dv * (1.0 - f) + last_dv * f;
            let len2 = (du2 * du2 + dv2 * dv2).sqrt();
            du2 /= len2;
            dv2 /= len2;

            tint = (uu2 * du2 + vv2 * dv2) * TINT_SCALE;
            break;
        }

        last_dt = dt;
        last_du = du;
        last_dv = dv;
    }

    (kelvin.clamp(1400.0, 100_000.0), tint)
}

/// (Kelvin, Adobe-scale tint) → chromaticity.
/// Port of dng_temperature::Get_xy_coord.
pub fn kelvin_tint_to_xy(kelvin: f32, tint: f32) -> (f32, f32) {
    let r = 1.0e6 / kelvin.clamp(1400.0, 100_000.0);

    for index in 0..30 {
        if r < ROBERTSON_TABLE[index + 1].0 || index == 29 {
            let f = (ROBERTSON_TABLE[index + 1].0 - r)
                / (ROBERTSON_TABLE[index + 1].0 - ROBERTSON_TABLE[index].0);
            let f = f.clamp(0.0, 1.0);

            let mut u = ROBERTSON_TABLE[index].1 * f + ROBERTSON_TABLE[index + 1].1 * (1.0 - f);
            let mut v = ROBERTSON_TABLE[index].2 * f + ROBERTSON_TABLE[index + 1].2 * (1.0 - f);

            // Interpolated isotherm direction for the tint offset.
            let mut du1 = 1.0f32;
            let mut dv1 = ROBERTSON_TABLE[index].3;
            let l1 = (1.0 + dv1 * dv1).sqrt();
            du1 /= l1;
            dv1 /= l1;
            let mut du2 = 1.0f32;
            let mut dv2 = ROBERTSON_TABLE[index + 1].3;
            let l2 = (1.0 + dv2 * dv2).sqrt();
            du2 /= l2;
            dv2 /= l2;

            let mut du = du1 * f + du2 * (1.0 - f);
            let mut dv = dv1 * f + dv2 * (1.0 - f);
            let len = (du * du + dv * dv).sqrt();
            du /= len;
            dv /= len;

            let offset = tint / TINT_SCALE;
            u += du * offset;
            v += dv * offset;

            return uv_to_xy(u, v);
        }
    }
    // Unreachable, but keep a sane fallback (D50-ish).
    (0.3457, 0.3585)
}

/// Which XYZ→camera matrix source to use for neutral↔chromaticity conversion.
pub enum CameraMatrices<'a> {
    /// Dual-illuminant DCP: interpolate ColorMatrix1/2 by 1/T (iterative).
    Dcp(&'a crate::raw::dcp::DcpProfile),
    /// Single matrix from the raw file metadata (XYZ→camera, row-major).
    Single([f32; 9]),
}

impl CameraMatrices<'_> {
    /// XYZ→camera matrix appropriate for the given temperature.
    fn matrix_at(&self, kelvin: f32) -> [f32; 9] {
        match self {
            CameraMatrices::Dcp(p) => crate::raw::dcp::interpolate_color_matrix(p, kelvin),
            CameraMatrices::Single(m) => *m,
        }
    }
}

fn invert_flat(m: [f32; 9]) -> Option<Matrix3<f32>> {
    // Row-major flat → cgmath column-major
    Matrix3::new(
        m[0], m[3], m[6],
        m[1], m[4], m[7],
        m[2], m[5], m[8],
    )
    .invert()
}

/// Camera-neutral (as-shot) → (Kelvin, tint).
///
/// `wb_multipliers` are the G-normalized as-shot gains; the camera neutral is
/// their reciprocal. Port of dng_color_spec::NeutralToXY — iterates because
/// the matrix choice depends on the temperature being solved for.
pub fn wb_to_kelvin_tint(wb_multipliers: [f32; 4], matrices: &CameraMatrices) -> (f32, f32) {
    let neutral = cgmath::Vector3::new(
        1.0 / wb_multipliers[0].max(1e-6),
        1.0 / wb_multipliers[1].max(1e-6),
        1.0 / wb_multipliers[2].max(1e-6),
    );

    // Start from D50 and iterate to convergence (single-matrix converges in 1).
    let (mut x, mut y) = (0.3457f32, 0.3585f32);
    for _ in 0..30 {
        let (kelvin, _) = xy_to_kelvin_tint(x, y);
        let Some(cam_to_xyz) = invert_flat(matrices.matrix_at(kelvin)) else {
            return (5000.0, 0.0);
        };
        let xyz = cam_to_xyz * neutral;
        let sum = xyz.x + xyz.y + xyz.z;
        if sum.abs() < 1e-9 {
            return (5000.0, 0.0);
        }
        let (nx, ny) = (xyz.x / sum, xyz.y / sum);
        let dx = nx - x;
        let dy = ny - y;
        x = nx;
        y = ny;
        if dx.abs() < 1e-7 && dy.abs() < 1e-7 {
            break;
        }
    }

    xy_to_kelvin_tint(x, y)
}

/// (Kelvin, tint) → G-normalized WB multipliers for the camera.
pub fn kelvin_tint_to_wb(kelvin: f32, tint: f32, matrices: &CameraMatrices) -> [f32; 4] {
    let (x, y) = kelvin_tint_to_xy(kelvin, tint);
    // xy → XYZ with Y = 1
    let xyz = cgmath::Vector3::new(x / y.max(1e-6), 1.0, (1.0 - x - y) / y.max(1e-6));

    let m = matrices.matrix_at(kelvin);
    // XYZ→camera (row-major flat), applied directly
    let neutral = cgmath::Vector3::new(
        m[0] * xyz.x + m[1] * xyz.y + m[2] * xyz.z,
        m[3] * xyz.x + m[4] * xyz.y + m[5] * xyz.z,
        m[6] * xyz.x + m[7] * xyz.y + m[8] * xyz.z,
    );

    let g = neutral.y.max(1e-6);
    [
        (g / neutral.x.max(1e-6)).clamp(0.05, 20.0),
        1.0,
        (g / neutral.z.max(1e-6)).clamp(0.05, 20.0),
        1.0,
    ]
}

/// Resolve the target WB for the current slider positions.
///
/// Returns `(kelvin, tint, wb_override)`:
/// - `kelvin`/`tint` — absolute values (anchored at as-shot) for display and
///   for DCP dual-illuminant interpolation.
/// - `wb_override` — recomputed camera multipliers, or None when both sliders
///   sit at 0 (use the exact as-shot multipliers; avoids CCT round-trip drift).
pub fn solve_wb(
    params: &crate::core::types::EditParams,
    as_shot: (f32, f32),
    dcp: Option<&crate::raw::dcp::DcpProfile>,
    fallback_matrix: [f32; 9],
) -> (f32, f32, Option<[f32; 4]>) {
    let kelvin = params.kelvin_from_anchor(as_shot.0);
    let tint = params.tint_from_anchor(as_shot.1);

    if params.temperature == 0.0 && params.tint == 0.0 {
        return (kelvin, tint, None);
    }

    let matrices = match dcp {
        Some(p) => CameraMatrices::Dcp(p),
        None => CameraMatrices::Single(fallback_matrix),
    };
    let wb = kelvin_tint_to_wb(kelvin, tint, &matrices);
    (kelvin, tint, Some(wb))
}

/// Map a display-space UV to sensor-space UV for a normalised EXIF orientation
/// (1/3/6/8). Must stay in lockstep with the identical transform at the top of
/// the color shader's fs_main (shaders.rs).
pub fn display_to_sensor_uv(u: f32, v: f32, orientation: u32) -> (f32, f32) {
    match orientation {
        6 => (v, 1.0 - u),
        8 => (1.0 - v, u),
        3 => (1.0 - u, 1.0 - v),
        _ => (u, v),
    }
}

/// As-shot (Kelvin, tint) for a loaded raw: solve the camera's WB multipliers
/// through the best available XYZ→camera matrix (DCP dual-illuminant when
/// present, otherwise the raw file's single matrix).
pub fn as_shot_kelvin_tint(raw: &crate::raw::loader::RawDataResult) -> (f32, f32) {
    let matrices = match &raw.dcp_profile {
        Some(p) => CameraMatrices::Dcp(p.as_ref()),
        None => CameraMatrices::Single(raw.color_matrix),
    };
    wb_to_kelvin_tint(raw.wb_multipliers, &matrices)
}

/// Check if a color matrix is the identity matrix (no conversion)
pub fn is_identity_matrix(matrix: &[f32; 9]) -> bool {
    const EPSILON: f32 = 0.001;

    (matrix[0] - 1.0).abs() < EPSILON
        && matrix[1].abs() < EPSILON
        && matrix[2].abs() < EPSILON
        && matrix[3].abs() < EPSILON
        && (matrix[4] - 1.0).abs() < EPSILON
        && matrix[5].abs() < EPSILON
        && matrix[6].abs() < EPSILON
        && matrix[7].abs() < EPSILON
        && (matrix[8] - 1.0).abs() < EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_matrix_detection() {
        let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        assert!(is_identity_matrix(&identity));

        let non_identity = [1.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        assert!(!is_identity_matrix(&non_identity));
    }

    // ── Orientation UV mapping ────────────────────────────────────────────

    /// 90° CW (6) and 270° CW (8) must be exact inverses, and 180° (3) must be
    /// an involution — guards the transform shared with the WGSL shader.
    #[test]
    fn test_orientation_uv_mapping() {
        let probes = [(0.0f32, 0.0f32), (1.0, 0.0), (0.25, 0.75), (0.5, 0.5)];
        for &(u, v) in &probes {
            let (a, b) = display_to_sensor_uv(u, v, 6);
            let (u2, v2) = display_to_sensor_uv(a, b, 8);
            assert!((u2 - u).abs() < 1e-6 && (v2 - v).abs() < 1e-6, "6∘8 ≠ id");

            let (c, d) = display_to_sensor_uv(u, v, 3);
            let (u3, v3) = display_to_sensor_uv(c, d, 3);
            assert!((u3 - u).abs() < 1e-6 && (v3 - v).abs() < 1e-6, "3∘3 ≠ id");

            assert_eq!(display_to_sensor_uv(u, v, 1), (u, v));
        }
        // Concrete corner: display top-left of a 90°-CW-rotated image is the
        // sensor's bottom-left (sensor u=0, v=1).
        assert_eq!(display_to_sensor_uv(0.0, 0.0, 6), (0.0, 1.0));
    }

    /// The eyedropper's slider-offset inversion must round-trip through
    /// EditParams::kelvin_from_anchor at several anchors.
    #[test]
    fn test_kelvin_anchor_inversion() {
        use crate::core::types::EditParams;
        for &anchor in &[3200.0f32, 5000.0, 6500.0, 8000.0] {
            for &target in &[4000.0f32, 5500.0, 7500.0] {
                let anchor_mired = 1.0e6 / anchor;
                let temperature = ((anchor_mired - 1.0e6 / target) / 120.0).clamp(-1.0, 1.0);
                let params = EditParams { temperature, ..Default::default() };
                let solved = params.kelvin_from_anchor(anchor);
                // Only exact when the offset fits within the slider range.
                if temperature.abs() < 1.0 {
                    assert!(
                        (solved - target).abs() / target < 0.005,
                        "anchor {anchor} target {target} → solved {solved}"
                    );
                }
            }
        }
    }

    // ── CCT (Robertson) machinery ─────────────────────────────────────────

    /// D65 chromaticity must solve to ~6500 K with near-zero tint.
    #[test]
    fn test_d65_cct() {
        let (kelvin, tint) = xy_to_kelvin_tint(0.31271, 0.32902);
        assert!(
            (6350.0..6650.0).contains(&kelvin),
            "D65 CCT out of range: {kelvin}"
        );
        assert!(tint.abs() < 15.0, "D65 tint too large: {tint}");
    }

    /// Standard Illuminant A (2856 K blackbody) sits ON the locus: tint ≈ 0.
    #[test]
    fn test_illuminant_a_cct() {
        let (kelvin, tint) = xy_to_kelvin_tint(0.44757, 0.40745);
        assert!(
            (2800.0..2950.0).contains(&kelvin),
            "Illuminant A CCT out of range: {kelvin}"
        );
        assert!(tint.abs() < 3.0, "Illuminant A tint too large: {tint}");
    }

    /// kelvin/tint → xy → kelvin/tint must round-trip across the usable range.
    #[test]
    fn test_kelvin_tint_roundtrip() {
        for &k in &[2500.0f32, 2850.0, 4000.0, 5000.0, 6500.0, 8000.0, 12000.0] {
            for &t in &[-30.0f32, 0.0, 30.0] {
                let (x, y) = kelvin_tint_to_xy(k, t);
                let (k2, t2) = xy_to_kelvin_tint(x, y);
                assert!(
                    (k2 - k).abs() / k < 0.02,
                    "kelvin roundtrip {k}K/{t} → {k2}K"
                );
                assert!((t2 - t).abs() < 2.0, "tint roundtrip {k}K/{t} → {t2}");
            }
        }
    }

    /// WB multipliers → (K, tint) → WB multipliers must round-trip through a
    /// realistic camera matrix (Nikon-ish XYZ→cam values).
    #[test]
    fn test_wb_roundtrip_through_matrix() {
        // Approximate D3300 ColorMatrix (XYZ→cam, row-major, D65-ish)
        let m = [
            0.7013, -0.1408, -0.0922,
            -0.4224, 1.1994, 0.2523,
            -0.0938, 0.2018, 0.5789,
        ];
        let matrices = CameraMatrices::Single(m);
        let wb = [2.1f32, 1.0, 1.4, 1.0]; // plausible daylight multipliers
        let (kelvin, tint) = wb_to_kelvin_tint(wb, &matrices);
        assert!(
            (3000.0..9000.0).contains(&kelvin),
            "implausible CCT for daylight WB: {kelvin}"
        );
        let wb2 = kelvin_tint_to_wb(kelvin, tint, &matrices);
        assert!(
            (wb2[0] - wb[0]).abs() / wb[0] < 0.02,
            "R multiplier roundtrip: {} → {}",
            wb[0],
            wb2[0]
        );
        assert!(
            (wb2[2] - wb[2]).abs() / wb[2] < 0.02,
            "B multiplier roundtrip: {} → {}",
            wb[2],
            wb2[2]
        );
    }

    /// Warmer slider (higher Kelvin) must raise the red multiplier relative to
    /// blue — i.e. the image gets warmer, matching the Lightroom convention.
    #[test]
    fn test_temperature_direction() {
        let m = [
            0.7013, -0.1408, -0.0922,
            -0.4224, 1.1994, 0.2523,
            -0.0938, 0.2018, 0.5789,
        ];
        let matrices = CameraMatrices::Single(m);
        let wb_warm = kelvin_tint_to_wb(7000.0, 0.0, &matrices);
        let wb_cool = kelvin_tint_to_wb(4000.0, 0.0, &matrices);
        assert!(
            wb_warm[0] / wb_warm[2] > wb_cool[0] / wb_cool[2],
            "higher Kelvin must boost R relative to B: warm {:?} vs cool {:?}",
            wb_warm,
            wb_cool
        );
    }

    #[test]
    fn test_cam_to_srgb_calculation() {
        // Example xyz_to_cam matrix (simplified)
        let xyz_to_cam = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

        let result = calculate_cam_to_srgb(xyz_to_cam, [1.0, 1.0, 1.0, 1.0], true);

        // Result should not be all zeros
        assert!(result.iter().any(|&x| x != 0.0));
    }


    // ── Matrix helpers ───────────────────────────────────────────────────────

    const IDENTITY: [f32; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

    #[test]
    fn mat3_mul_identity_is_a_no_op() {
        let m = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        assert_eq!(mat3_mul(&IDENTITY, &m), m);
        assert_eq!(mat3_mul(&m, &IDENTITY), m);
    }

    #[test]
    fn mat3_mul_matches_a_hand_computed_product() {
        // [1 2 0]   [0 1 0]   [2 1 0]
        // [0 1 0] x [1 0 0] = [1 0 0]
        // [0 0 1]   [0 0 1]   [0 0 1]
        let a = [1.0, 2.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let b = [0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        assert_eq!(
            mat3_mul(&a, &b),
            [2.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn mat3_mul_is_not_commutative() {
        // Guards against an accidentally symmetric test fixture hiding an
        // argument-order bug in the fold.
        let a = [1.0, 2.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let b = [0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        assert_ne!(mat3_mul(&a, &b), mat3_mul(&b, &a));
    }

    /// The transposition canary: XYZ D50 → ProPhoto must map the D50 white
    /// point to neutral (1, 1, 1). A transposed constant fails here loudly,
    /// whereas a render comparison would only show it as a colour shift.
    #[test]
    fn xyz_d50_to_prophoto_maps_white_point_to_neutral() {
        let d50_white = [0.96422, 1.0, 0.82521];
        let rgb = mat3_apply(&XYZ_D50_TO_PROPHOTO, d50_white);
        for (i, c) in rgb.iter().enumerate() {
            assert!(
                (c - 1.0).abs() < 2e-3,
                "channel {i} = {c}, expected ~1.0 — constant may be transposed"
            );
        }
    }

    #[test]
    fn xyz_d50_to_prophoto_keeps_blue_out_of_the_luminance_row() {
        // ProPhoto's Y row is (0.2880, 0.7119, 0.0001) in the INVERSE
        // direction; here the giveaway for a transpose is the zero pair in the
        // last ROW, which a column-major copy would put in the last column.
        assert_eq!(XYZ_D50_TO_PROPHOTO[6], 0.0);
        assert_eq!(XYZ_D50_TO_PROPHOTO[7], 0.0);
        assert_ne!(XYZ_D50_TO_PROPHOTO[2], 0.0);
    }
}
