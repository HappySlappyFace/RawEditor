/// Histogram data for R, G, B, and Luma channels
#[derive(Debug, Clone, PartialEq)]
pub struct HistogramData {
    pub r: [u32; 256],
    pub g: [u32; 256],
    pub b: [u32; 256],
    pub l: [u32; 256],
}

impl Default for HistogramData {
    fn default() -> Self {
        Self {
            r: [0; 256],
            g: [0; 256],
            b: [0; 256],
            l: [0; 256],
        }
    }
}

/// Calculate histogram from RGBA bytes (CPU-bound)
pub fn calculate(rgba_bytes: &[u8]) -> HistogramData {
    let mut data = HistogramData::default();

    // Iterate through pixels in chunks of 4 [R, G, B, A]
    for chunk in rgba_bytes.chunks_exact(4) {
        let r = chunk[0];
        let g = chunk[1];
        let b = chunk[2];
        // chunk[3] is alpha, ignored

        // Increment RGB bins
        data.r[r as usize] += 1;
        data.g[g as usize] += 1;
        data.b[b as usize] += 1;

        // Calculate Luma (Rec. 601 coefficients usually, or Rec. 709)
        // User specified: 0.299 * r + 0.587 * g + 0.114 * b
        let y = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as usize;

        // Clamp to 255 just in case float math goes slightly over
        let y = y.min(255);

        data.l[y] += 1;
    }

    data
}
