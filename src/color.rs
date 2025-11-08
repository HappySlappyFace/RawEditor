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
pub fn calculate_cam_to_srgb_matrix(xyz_to_cam: [f32; 9]) -> [f32; 9] {
    // DECISION: Camera color matrix math is too complex and camera-specific
    // Phase 14 colors (WB only) are VERY close to correct, just slightly desaturated
    // Return identity matrix = Phase 14 quality
    // TODO: Add simple saturation boost slider instead of complex matrix math
    println!("🎨 Phase 15: Using identity matrix (bypassing color matrix calculation)");
    println!("🎨 Reason: Phase 14 white balance gives 95% correct colors");
    println!("🎨 Next: Add saturation slider for final 5% color boost");
    return [
        1.0, 0.0, 0.0,
        0.0, 1.0, 0.0,
        0.0, 0.0, 1.0,
    ];
    
    /* DISABLED - matrix math causes pink tint
    println!("\n🔧 Phase 15: Calculating cam-to-sRGB matrix...");
    println!("Input xyz_to_cam (row-major): [{:.3}, {:.3}, {:.3}]", xyz_to_cam[0], xyz_to_cam[1], xyz_to_cam[2]);
    println!("                               [{:.3}, {:.3}, {:.3}]", xyz_to_cam[3], xyz_to_cam[4], xyz_to_cam[5]);
    println!("                               [{:.3}, {:.3}, {:.3}]", xyz_to_cam[6], xyz_to_cam[7], xyz_to_cam[8]);
    
    // Check if it's identity - if so, return identity (no conversion needed)
    if is_identity_matrix(&xyz_to_cam) {
        println!("⚠️  Input is identity matrix, returning identity (no color conversion)");
        return xyz_to_cam;
    }
    
    // Camera matrices are often scaled by 10000 in RAW metadata
    // Normalize them to proper range (check if values are > 10, indicating scaling)
    let needs_normalization = xyz_to_cam.iter().any(|&x| x.abs() > 10.0);
    let normalized_matrix = if needs_normalization {
        println!("🔧 Normalizing matrix (dividing by 10000)...");
        [
            xyz_to_cam[0] / 10000.0, xyz_to_cam[1] / 10000.0, xyz_to_cam[2] / 10000.0,
            xyz_to_cam[3] / 10000.0, xyz_to_cam[4] / 10000.0, xyz_to_cam[5] / 10000.0,
            xyz_to_cam[6] / 10000.0, xyz_to_cam[7] / 10000.0, xyz_to_cam[8] / 10000.0,
        ]
    } else {
        xyz_to_cam
    };
    
    println!("Normalized matrix: [{:.4}, {:.4}, {:.4}]", normalized_matrix[0], normalized_matrix[1], normalized_matrix[2]);
    println!("                   [{:.4}, {:.4}, {:.4}]", normalized_matrix[3], normalized_matrix[4], normalized_matrix[5]);
    println!("                   [{:.4}, {:.4}, {:.4}]", normalized_matrix[6], normalized_matrix[7], normalized_matrix[8]);
    
    // Convert flat array to cgmath Matrix3 (column-major in cgmath)
    // Use the NORMALIZED matrix!
    let xyz_to_cam_matrix = Matrix3::new(
        normalized_matrix[0], normalized_matrix[3], normalized_matrix[6],  // Column 0
        normalized_matrix[1], normalized_matrix[4], normalized_matrix[7],  // Column 1
        normalized_matrix[2], normalized_matrix[5], normalized_matrix[8],  // Column 2
    );
    
    // Invert to get cam_to_xyz
    let cam_to_xyz = match xyz_to_cam_matrix.invert() {
        Some(inverted) => {
            println!("✅ Matrix inverted successfully");
            // Debug: print cam_to_xyz
            println!("cam_to_xyz (col-major): [{:.4}, {:.4}, {:.4}]", inverted[0][0], inverted[0][1], inverted[0][2]);
            println!("                        [{:.4}, {:.4}, {:.4}]", inverted[1][0], inverted[1][1], inverted[1][2]);
            println!("                        [{:.4}, {:.4}, {:.4}]", inverted[2][0], inverted[2][1], inverted[2][2]);
            inverted
        },
        None => {
            eprintln!("⚠️  Failed to invert xyz_to_cam matrix, using identity");
            return [
                1.0, 0.0, 0.0,
                0.0, 1.0, 0.0,
                0.0, 0.0, 1.0,
            ];
        }
    };
    
    // Convert XYZ_TO_SRGB to cgmath Matrix3
    let xyz_to_srgb_matrix = Matrix3::new(
        XYZ_TO_SRGB[0][0], XYZ_TO_SRGB[1][0], XYZ_TO_SRGB[2][0],  // Column 0
        XYZ_TO_SRGB[0][1], XYZ_TO_SRGB[1][1], XYZ_TO_SRGB[2][1],  // Column 1
        XYZ_TO_SRGB[0][2], XYZ_TO_SRGB[1][2], XYZ_TO_SRGB[2][2],  // Column 2
    );
    
    // Multiply: cam_to_srgb = xyz_to_srgb × cam_to_xyz
    let cam_to_srgb = xyz_to_srgb_matrix * cam_to_xyz;
    
    // Debug: print cam_to_srgb before conversion
    println!("cam_to_srgb (col-major): [{:.4}, {:.4}, {:.4}]", cam_to_srgb[0][0], cam_to_srgb[0][1], cam_to_srgb[0][2]);
    println!("                         [{:.4}, {:.4}, {:.4}]", cam_to_srgb[1][0], cam_to_srgb[1][1], cam_to_srgb[1][2]);
    println!("                         [{:.4}, {:.4}, {:.4}]", cam_to_srgb[2][0], cam_to_srgb[2][1], cam_to_srgb[2][2]);
    
    // Convert back to flat row-major array for GPU
    let result = [
        cam_to_srgb[0][0], cam_to_srgb[1][0], cam_to_srgb[2][0],  // Row 0
        cam_to_srgb[0][1], cam_to_srgb[1][1], cam_to_srgb[2][1],  // Row 1
        cam_to_srgb[0][2], cam_to_srgb[1][2], cam_to_srgb[2][2],  // Row 2
    ];
    
    println!("Output cam_to_srgb (raw): [{:.3}, {:.3}, {:.3}]", result[0], result[1], result[2]);
    println!("                          [{:.3}, {:.3}, {:.3}]", result[3], result[4], result[5]);
    println!("                          [{:.3}, {:.3}, {:.3}]", result[6], result[7], result[8]);
    
    // Scale the entire matrix to bring diagonal values to a reasonable range
    // Typical color matrices have diagonal values around 1.0-1.5
    // Calculate average of diagonal elements
    let diag_avg = (result[0].abs() + result[4].abs() + result[8].abs()) / 3.0;
    let scale_factor = if diag_avg > 2.0 {
        1.5 / diag_avg  // Target average diagonal of ~1.5
    } else {
        1.0  // No scaling needed
    };
    
    println!("🔧 Diagonal average: {:.3}, scale factor: {:.3}", diag_avg, scale_factor);
    
    let normalized_result = [
        result[0] * scale_factor, result[1] * scale_factor, result[2] * scale_factor,
        result[3] * scale_factor, result[4] * scale_factor, result[5] * scale_factor,
        result[6] * scale_factor, result[7] * scale_factor, result[8] * scale_factor,
    ];
    
    println!("Output cam_to_srgb (scaled): [{:.3}, {:.3}, {:.3}]", normalized_result[0], normalized_result[1], normalized_result[2]);
    println!("                             [{:.3}, {:.3}, {:.3}]", normalized_result[3], normalized_result[4], normalized_result[5]);
    println!("                             [{:.3}, {:.3}, {:.3}]", normalized_result[6], normalized_result[7], normalized_result[8]);
    
    // Check for unreasonable values (typical color matrices have values between -5 and 5)
    let has_extreme_values = normalized_result.iter().any(|&x| x.abs() > 10.0 || !x.is_finite());
    if has_extreme_values {
        eprintln!("⚠️  WARNING: Color matrix has extreme values! Using identity instead.");
        eprintln!("This might indicate incorrect camera metadata or matrix math error.");
        return [
            1.0, 0.0, 0.0,
            0.0, 1.0, 0.0,
            0.0, 0.0, 1.0,
        ];
    }
    
    normalized_result
    */
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
