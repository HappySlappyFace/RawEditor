pub type HistogramData = [[u32; 256]; 3];

pub fn calculate(rgba_bytes: &[u8]) -> HistogramData {
    let mut histogram = [[0u32; 256]; 3];
    for chunk in rgba_bytes.chunks_exact(4) {
        histogram[0][chunk[0] as usize] += 1;
        histogram[1][chunk[1] as usize] += 1;
        histogram[2][chunk[2] as usize] += 1;
    }
    histogram
}
