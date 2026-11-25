// ── Black-level histogram loop regression ────────────────────────────────────
// The `break` inside the inner loop previously only exited the x-loop.
// This simulates the fixed loop (labeled break) and verifies counts are correct.

fn count_pixels_with_labeled_break(data: &[u16], width: usize, height: usize) -> usize {
    let mut count = 0usize;
    'outer: for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if idx >= data.len() {
                break 'outer;
            }
            count += 1;
        }
    }
    count
}

#[test]
fn test_labeled_break_counts_all_valid_pixels() {
    // Exact fit: 3×3 grid with exactly 9 elements
    let data: Vec<u16> = vec![0; 9];
    assert_eq!(count_pixels_with_labeled_break(&data, 3, 3), 9);
}

#[test]
fn test_labeled_break_stops_at_truncated_data() {
    // Declare 4×4 but only provide 10 elements — should stop at 10, not 16
    let data: Vec<u16> = vec![0; 10];
    let count = count_pixels_with_labeled_break(&data, 4, 4);
    assert_eq!(count, 10, "Should stop exactly when data runs out");
}

// ── JPEG size overflow regression ────────────────────────────────────────────
// Previously: (width * height * 3) as usize could overflow on 32-bit u32.
// Fix: use saturating_mul chain on usize.

fn expected_rgb_len_safe(width: u32, height: u32) -> usize {
    (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3)
}

#[test]
fn test_jpeg_size_normal() {
    assert_eq!(expected_rgb_len_safe(100, 100), 30_000);
}

#[test]
fn test_jpeg_size_saturates_instead_of_overflowing() {
    // u32::MAX * u32::MAX would overflow as u32 but saturates to usize::MAX
    let result = expected_rgb_len_safe(u32::MAX, u32::MAX);
    assert_eq!(result, usize::MAX, "Must saturate, not wrap");
}

#[test]
fn test_jpeg_size_zero_dimensions() {
    assert_eq!(expected_rgb_len_safe(0, 1000), 0);
    assert_eq!(expected_rgb_len_safe(1000, 0), 0);
}

// ── Crop value clamping regression ───────────────────────────────────────────
// After a drag, crop values must stay in [0,1] and width/height must be ≥ 0.

fn clamp_crop(l: f32, t: f32, r: f32, b: f32) -> [f32; 4] {
    let l = l.clamp(0.0, 1.0);
    let t = t.clamp(0.0, 1.0);
    let r = r.clamp(0.0, 1.0);
    let b = b.clamp(0.0, 1.0);
    [l, t, (r - l).max(0.0), (b - t).max(0.0)]
}

#[test]
fn test_crop_clamp_normal() {
    let c = clamp_crop(0.1, 0.1, 0.9, 0.9);
    // Use approximate comparison due to f32 rounding (0.9 - 0.1 = 0.79999995)
    assert!((c[0] - 0.1).abs() < 1e-5);
    assert!((c[1] - 0.1).abs() < 1e-5);
    assert!((c[2] - 0.8).abs() < 1e-5);
    assert!((c[3] - 0.8).abs() < 1e-5);
}

#[test]
fn test_crop_clamp_negative_origin() {
    let c = clamp_crop(-0.5, -0.5, 0.5, 0.5);
    assert_eq!(c[0], 0.0); // l clamped
    assert_eq!(c[1], 0.0); // t clamped
    assert!(c[2] >= 0.0);  // width non-negative
    assert!(c[3] >= 0.0);  // height non-negative
}

#[test]
fn test_crop_clamp_inverted_does_not_produce_negative_size() {
    // r < l (inverted) must yield width 0, not negative
    let c = clamp_crop(0.8, 0.2, 0.2, 0.8);
    assert!(c[2] >= 0.0, "width must be >= 0");
    assert!(c[3] >= 0.0, "height must be >= 0");
}

#[test]
fn test_crop_clamp_full_image() {
    let c = clamp_crop(0.0, 0.0, 1.0, 1.0);
    assert_eq!(c, [0.0, 0.0, 1.0, 1.0]);
}

// ── Export zero-size guard ───────────────────────────────────────────────────

fn compute_export_dims(image_w: u32, image_h: u32, crop_w: f32, crop_h: f32) -> (u32, u32) {
    let cw = crop_w.clamp(0.001, 1.0);
    let ch = crop_h.clamp(0.001, 1.0);
    let tw = ((image_w as f32 * cw) as u32).max(1);
    let th = ((image_h as f32 * ch) as u32).max(1);
    (tw, th)
}

#[test]
fn test_export_dims_normal() {
    let (w, h) = compute_export_dims(8256, 5504, 1.0, 1.0);
    assert_eq!(w, 8256);
    assert_eq!(h, 5504);
}

#[test]
fn test_export_dims_zero_crop_gives_at_least_one_pixel() {
    let (w, h) = compute_export_dims(8256, 5504, 0.0, 0.0);
    assert!(w >= 1, "width must be at least 1, got {}", w);
    assert!(h >= 1, "height must be at least 1, got {}", h);
}

#[test]
fn test_export_dims_tiny_crop() {
    let (w, h) = compute_export_dims(100, 100, 0.001, 0.001);
    assert!(w >= 1);
    assert!(h >= 1);
}

// ── Export buffer alignment guard ────────────────────────────────────────────

fn validate_export_buffer(bytes: &[u8]) -> Result<(), String> {
    if !bytes.len().is_multiple_of(4) {
        return Err(format!(
            "Export buffer length {} is not a multiple of 4",
            bytes.len()
        ));
    }
    Ok(())
}

#[test]
fn test_export_buffer_valid_alignment() {
    assert!(validate_export_buffer(&[0u8; 8]).is_ok());
    assert!(validate_export_buffer(&[0u8; 0]).is_ok());
}

#[test]
fn test_export_buffer_misaligned_returns_error() {
    assert!(validate_export_buffer(&[0u8; 9]).is_err());
    assert!(validate_export_buffer(&[0u8; 7]).is_err());
    assert!(validate_export_buffer(&[0u8; 1]).is_err());
}

// ── System time safety ───────────────────────────────────────────────────────

#[test]
fn test_system_time_unwrap_or_default_is_safe() {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    // Simulate a pre-epoch time by computing duration_since on a future time
    let future = SystemTime::now() + Duration::from_secs(1_000_000);
    // duration_since(future) would fail; unwrap_or_default should give 0
    let secs = UNIX_EPOCH
        .duration_since(future)
        .unwrap_or_default()
        .as_secs() as i64;
    assert_eq!(secs, 0, "Pre-epoch time must not panic and must return 0");
}
