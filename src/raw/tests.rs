use super::processor::normalize_pixel;

#[test]
fn test_normalization() {
    // Test 1: 12-bit sensor data (0-4095)
    // Black=0, White=4095, Input=2048 (approx mid-grey)
    let black = 0;
    let white = 4095;
    let input = 2048;

    let result = normalize_pixel(input, black, white);

    // 2048 / 4095 = 0.500122...
    assert!(
        (result - 0.5).abs() < 0.001,
        "Expected approx 0.5, got {}",
        result
    );
}

#[test]
fn test_black_level_subtraction() {
    // Test 2: Black level subtraction
    // Input matches black level -> should be 0.0
    let black = 100;
    let white = 4095;
    let input = 100;

    let result = normalize_pixel(input, black, white);

    assert_eq!(result, 0.0, "Expected 0.0 for input == black level");
}

#[test]
fn test_clipping() {
    let black = 100;
    let white = 4095;

    // Below black
    assert_eq!(normalize_pixel(50, black, white), 0.0);

    // Above white
    assert_eq!(normalize_pixel(5000, black, white), 1.0);
}
