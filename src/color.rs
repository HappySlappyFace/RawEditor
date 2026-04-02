/// Color space conversion utilities
///
/// This module handles conversion between different color spaces:
/// - Camera RGB (sensor-native color space)
/// - XYZ (device-independent color space)
/// - sRGB (standard display color space)
use cgmath::{Matrix3, SquareMatrix};

/// Calculate the camera-to-sRGB color conversion matrix.
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

    // Step 5: Chain transformations: Cam -> XYZ_D50 -> XYZ_D65 -> sRGB
    let mut final_matrix = XYZ_TO_SRGB * BRADFORD_D50_TO_D65 * cam_to_xyz_d50;

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

    #[test]
    fn test_cam_to_srgb_calculation() {
        // Example xyz_to_cam matrix (simplified)
        let xyz_to_cam = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

        let result = calculate_cam_to_srgb(xyz_to_cam);

        // Result should not be all zeros
        assert!(result.iter().any(|&x| x != 0.0));
    }
}
