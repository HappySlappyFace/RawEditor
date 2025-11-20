/// Color space conversion utilities
///
/// This module handles conversion between different color spaces:
/// - Camera RGB (sensor-native color space)
/// - XYZ (device-independent color space)
/// - sRGB (standard display color space)

use cgmath::{Matrix3, SquareMatrix};

/// Standard XYZ to sRGB conversion matrix (D65 white point)
/// This is the industry-standard matrix for converting from CIE XYZ to sRGB
/// Source: IEC 61966-2-1:1999 (sRGB standard)
const XYZ_TO_SRGB: [[f32; 3]; 3] = [
    [ 3.2406, -1.5372, -0.4986],
    [-0.9689,  1.8758,  0.0415],
    [ 0.0557, -0.2040,  1.0570],
];

/// Calculate the camera-to-sRGB color conversion matrix
///
/// This function converts a camera's XYZ-to-camera matrix into a camera-to-sRGB matrix
/// by inverting it and multiplying with the standard XYZ-to-sRGB matrix.
///
/// # Arguments
/// * `xyz_to_cam` - The camera's XYZ to camera RGB matrix (from RAW metadata)
///
/// # Returns
/// * Camera-to-sRGB conversion matrix as a flat [f32; 9] array (row-major)
///
/// # Algorithm
/// 1. Load xyz_to_cam into a 3x3 matrix
/// 2. Invert to get cam_to_xyz: cam_to_xyz = inverse(xyz_to_cam)
/// 3. Multiply: cam_to_srgb = XYZ_TO_SRGB × cam_to_xyz
/// 4. Return as flat array for GPU upload
/// Calculate the camera-to-sRGB color conversion matrix
///
/// This function converts a camera's XYZ-to-camera matrix into a camera-to-sRGB matrix
/// by inverting it and multiplying with the standard XYZ-to-sRGB matrix.
/// It also performs row normalization to prevent color casts (pink tint).
///
/// # Arguments
/// * `raw_matrix` - The camera's XYZ to camera RGB matrix (from RAW metadata)
///
/// # Returns
/// * Camera-to-sRGB conversion matrix as a flat [f32; 9] array (row-major)
pub fn calculate_cam_to_srgb(raw_matrix: [f32; 9]) -> [f32; 9] {
    crate::debug_log!(crate::debug::DEBUG_APP, "🎨 Calculating Cam-to-sRGB matrix...");

    // Step 1: Load raw_matrix into Matrix3 (XYZ_to_Cam)
    // raw_matrix is usually row-major from the loader, but cgmath expects column-major arguments for new()
    // However, if we treat the input array as row-major, we need to transpose it or load it carefully.
    // Let's assume the input `raw_matrix` is row-major: [r0c0, r0c1, r0c2, r1c0, ...]
    // Matrix3::new takes c0r0, c0r1, c0r2, c1r0...
    
    // Let's load it as is and see. Usually these 3x3 matrices are provided as flat lists.
    // If raw_matrix is [a, b, c, d, e, f, g, h, i]
    // We want a matrix:
    // | a b c |
    // | d e f |
    // | g h i |
    //
    // Matrix3::new(c0r0, c0r1, c0r2, c1r0, c1r1, c1r2, c2r0, c2r1, c2r2)
    // c0r0 = a, c0r1 = d, c0r2 = g
    // c1r0 = b, c1r1 = e, c1r2 = h
    // c2r0 = c, c2r1 = f, c2r2 = i
    
    let xyz_to_cam = Matrix3::new(
        raw_matrix[0], raw_matrix[3], raw_matrix[6], // Column 0
        raw_matrix[1], raw_matrix[4], raw_matrix[7], // Column 1
        raw_matrix[2], raw_matrix[5], raw_matrix[8], // Column 2
    );

    // Step 2: Invert it to get Cam_to_XYZ
    let cam_to_xyz = match xyz_to_cam.invert() {
        Some(m) => m,
        None => {
            crate::debug_log!(crate::debug::DEBUG_APP, "⚠️ Failed to invert XYZ-to-Cam matrix, using identity");
            return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        }
    };

    // Define XYZ to sRGB (D65) matrix
    // Column-major order for cgmath
    #[rustfmt::skip]
    const XYZ_TO_SRGB_MAT: Matrix3<f32> = Matrix3::new(
         3.2406, -0.9689,  0.0557, // Column 0
        -1.5372,  1.8758, -0.2040, // Column 1
        -0.4986,  0.0415,  1.0570, // Column 2
    );

    // Step 3: Multiply XYZ_TO_SRGB * Cam_to_XYZ
    let unnormalized_matrix = XYZ_TO_SRGB_MAT * cam_to_xyz;

    // Step 4 (The Fix): Normalize the rows
    // We need to access rows. In cgmath Matrix3, columns are accessible.
    // M = | c0.x c1.x c2.x |
    //     | c0.y c1.y c2.y |
    //     | c0.z c1.z c2.z |
    
    // Row 0: (c0.x, c1.x, c2.x)
    let r0_sum = unnormalized_matrix.x.x + unnormalized_matrix.y.x + unnormalized_matrix.z.x;
    // Row 1: (c0.y, c1.y, c2.y)
    let r1_sum = unnormalized_matrix.x.y + unnormalized_matrix.y.y + unnormalized_matrix.z.y;
    // Row 2: (c0.z, c1.z, c2.z)
    let r2_sum = unnormalized_matrix.x.z + unnormalized_matrix.y.z + unnormalized_matrix.z.z;

    // Avoid division by zero
    let r0_scale = if r0_sum.abs() > 1e-6 { 1.0 / r0_sum } else { 1.0 };
    let r1_scale = if r1_sum.abs() > 1e-6 { 1.0 / r1_sum } else { 1.0 };
    let r2_scale = if r2_sum.abs() > 1e-6 { 1.0 / r2_sum } else { 1.0 };

    // Construct the final normalized matrix
    // We return a flat array [r0c0, r0c1, r0c2, r1c0...]
    let result = [
        unnormalized_matrix.x.x * r0_scale, unnormalized_matrix.y.x * r0_scale, unnormalized_matrix.z.x * r0_scale,
        unnormalized_matrix.x.y * r1_scale, unnormalized_matrix.y.y * r1_scale, unnormalized_matrix.z.y * r1_scale,
        unnormalized_matrix.x.z * r2_scale, unnormalized_matrix.y.z * r2_scale, unnormalized_matrix.z.z * r2_scale,
    ];

    crate::debug_log!(crate::debug::DEBUG_APP, "✅ Calculated Cam-to-sRGB matrix (normalized)");
    
    result
}

/// Check if a color matrix is the identity matrix (no conversion)
pub fn is_identity_matrix(matrix: &[f32; 9]) -> bool {
    const EPSILON: f32 = 0.001;
    
    (matrix[0] - 1.0).abs() < EPSILON && matrix[1].abs() < EPSILON && matrix[2].abs() < EPSILON &&
    matrix[3].abs() < EPSILON && (matrix[4] - 1.0).abs() < EPSILON && matrix[5].abs() < EPSILON &&
    matrix[6].abs() < EPSILON && matrix[7].abs() < EPSILON && (matrix[8] - 1.0).abs() < EPSILON
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

    #[test]
    fn test_cam_to_srgb_calculation() {
        // Example xyz_to_cam matrix (simplified)
        let xyz_to_cam = [
            1.0, 0.0, 0.0,
            0.0, 1.0, 0.0,
            0.0, 0.0, 1.0,
        ];
        
        let result = calculate_cam_to_srgb_matrix(xyz_to_cam);
        
        // Result should not be all zeros
        assert!(result.iter().any(|&x| x != 0.0));
    }
}
