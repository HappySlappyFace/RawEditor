pub type HistogramData = [[u32; 256]; 3];

pub fn calculate(rgba_bytes: &[u8]) -> HistogramData {
    let mut histogram = [[0u32; 256]; 3];
    // Use chunks(4) and only process complete 4-byte RGBA pixels
    for chunk in rgba_bytes.chunks(4) {
        if chunk.len() == 4 {
            histogram[0][chunk[0] as usize] += 1;
            histogram[1][chunk[1] as usize] += 1;
            histogram[2][chunk[2] as usize] += 1;
        }
    }
    histogram
}

/// The count a drawn histogram scales to: the tallest bin except the two
/// ends. Clipped shadows or highlights pile into bin 0 or 255, and scaling to
/// that spike flattens everything else to nothing (a dark edit then shows an
/// empty box). The end bins are drawn clamped to full height instead. Falls
/// back to the end bins only when nothing else is counted.
pub fn display_peak(h: &HistogramData) -> u32 {
    let inner = h.iter().flat_map(|c| c[1..255].iter()).copied().max().unwrap_or(0);
    if inner > 0 {
        inner
    } else {
        h.iter().flat_map(|c| [c[0], c[255]]).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clipped_spike_does_not_set_the_scale() {
        let mut h = [[0u32; 256]; 3];
        h[2][0] = 1_000_000;
        h[0][40] = 300;
        h[1][200] = 500;
        assert_eq!(display_peak(&h), 500);
        // Everything clipped: the ends are all there is to show.
        let mut all_black = [[0u32; 256]; 3];
        all_black[1][0] = 70;
        assert_eq!(display_peak(&all_black), 70);
        assert_eq!(display_peak(&[[0; 256]; 3]), 0);
    }

    #[test]
    fn test_empty_buffer_does_not_panic() {
        let data: &[u8] = &[];
        let h = calculate(data);
        assert!(h[0].iter().all(|&v| v == 0));
        assert!(h[1].iter().all(|&v| v == 0));
        assert!(h[2].iter().all(|&v| v == 0));
    }

    #[test]
    fn test_misaligned_buffer_does_not_panic() {
        // 9 bytes — not divisible by 4; the trailing byte must be silently ignored
        let data: &[u8] = &[255, 0, 0, 255, 0, 255, 0, 255, 99];
        let h = calculate(data);
        // Only two complete RGBA pixels should have been counted
        assert_eq!(h[0][255], 1); // R=255 from pixel 1
        assert_eq!(h[0][0],   1); // R=0   from pixel 2
        // The trailing byte (99) must NOT appear anywhere
        assert_eq!(h[0][99], 0);
        assert_eq!(h[1][99], 0);
        assert_eq!(h[2][99], 0);
    }

    #[test]
    fn test_single_pixel_counted_correctly() {
        // One RGBA pixel: R=10, G=20, B=30, A=255
        let data: &[u8] = &[10, 20, 30, 255];
        let h = calculate(data);
        assert_eq!(h[0][10], 1);
        assert_eq!(h[1][20], 1);
        assert_eq!(h[2][30], 1);
        // No spurious counts anywhere else
        let total_r: u32 = h[0].iter().sum();
        assert_eq!(total_r, 1);
    }

    #[test]
    fn test_all_channels_accumulated() {
        // Three identical white pixels
        let pixel = [255u8, 255, 255, 255];
        let data: Vec<u8> = pixel.iter().cloned().cycle().take(12).collect();
        let h = calculate(&data);
        assert_eq!(h[0][255], 3);
        assert_eq!(h[1][255], 3);
        assert_eq!(h[2][255], 3);
    }

    #[test]
    fn test_alpha_channel_ignored() {
        // A=0 (fully transparent) — alpha must not affect the RGB bins
        let data: &[u8] = &[100, 150, 200, 0];
        let h = calculate(data);
        assert_eq!(h[0][100], 1);
        assert_eq!(h[1][150], 1);
        assert_eq!(h[2][200], 1);
    }
}
