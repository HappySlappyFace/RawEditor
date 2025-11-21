
/// Compute median black level for each CFA phase from optical black margins
fn compute_cfa_black_levels(
    data: &[u16], 
    width: usize, 
    height: usize, 
    crops: &[usize], 
    cfa: rawloader::CFA
) -> [f32; 4] {
    // crops: [top, right, bottom, left]
    let top_margin = crops[0];
    let right_margin = crops[1];
    let bottom_margin = crops[2];
    let left_margin = crops[3];
    
    // We will collect pixels for each of the 4 CFA phases:
    // Phase 0: (even row, even col) -> (0,0)
    // Phase 1: (even row, odd col)  -> (0,1)
    // Phase 2: (odd row, even col)  -> (1,0)
    // Phase 3: (odd row, odd col)   -> (1,1)
    let mut phase_pixels: [Vec<u16>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    
    // Helper to add pixel to correct phase bucket
    let mut add_pixel = |x: usize, y: usize, val: u16| {
        let phase_idx = ((y & 1) << 1) | (x & 1);
        phase_pixels[phase_idx].push(val);
    };
    
    // 1. Top Margin
    for y in 0..top_margin {
        for x in 0..width {
            if y < height && x < width {
                add_pixel(x, y, data[y * width + x]);
            }
        }
    }
    
    // 2. Bottom Margin
    for y in (height - bottom_margin)..height {
        for x in 0..width {
            if y < height && x < width {
                add_pixel(x, y, data[y * width + x]);
            }
        }
    }
    
    // 3. Left Margin (excluding top/bottom corners to avoid double counting)
    for y in top_margin..(height - bottom_margin) {
        for x in 0..left_margin {
            if y < height && x < width {
                add_pixel(x, y, data[y * width + x]);
            }
        }
    }
    
    // 4. Right Margin (excluding top/bottom corners)
    for y in top_margin..(height - bottom_margin) {
        for x in (width - right_margin)..width {
            if y < height && x < width {
                add_pixel(x, y, data[y * width + x]);
            }
        }
    }
    
    // Compute median for each phase
    let mut medians = [0.0; 4];
    for i in 0..4 {
        let pixels = &mut phase_pixels[i];
        if pixels.is_empty() {
            println!("⚠️  No optical black pixels found for phase {}", i);
            continue;
        }
        
        // Sort to find median
        pixels.sort_unstable();
        let mid = pixels.len() / 2;
        medians[i] = pixels[mid] as f32;
        
        // Calculate stats for debugging
        let min = pixels[0];
        let max = pixels[pixels.len() - 1];
        let p1 = pixels[pixels.len() / 100]; // 1st percentile
        
        println!("Phase {}: Median={:.1}, Min={}, Max={}, P1={} (N={})", 
            i, medians[i], min, max, p1, pixels.len());
    }
    
    // Map phases to CFA colors based on pattern
    // We need to return [R, G1, G2, B] order for the shader
    // rawloader CFA pattern: 0=Red, 1=Green, 2=Blue
    // But we need to know the layout.
    // Let's assume standard Bayer phases for now and map them later if needed.
    // Actually, the shader expects [R, G1, G2, B] values.
    // We need to know which phase corresponds to which color.
    
    // For RGGB (Pattern 0):
    // (0,0)=R, (0,1)=G, (1,0)=G, (1,1)=B
    // So Phase 0->R, Phase 1->G1, Phase 2->G2, Phase 3->B
    
    // For GRBG (Pattern 1):
    // (0,0)=G, (0,1)=R, (1,0)=B, (1,1)=G
    // So Phase 0->G1, Phase 1->R, Phase 2->B, Phase 3->G2
    
    // For GBRG (Pattern 2):
    // (0,0)=G, (0,1)=B, (1,0)=R, (1,1)=G
    // So Phase 0->G1, Phase 1->B, Phase 2->R, Phase 3->G2
    
    // For BGGR (Pattern 3):
    // (0,0)=B, (0,1)=G, (1,0)=G, (1,1)=R
    // So Phase 0->B, Phase 1->G1, Phase 2->G2, Phase 3->R
    
    // However, our shader logic ALREADY handles the mapping from (x,y) to color index.
    // The shader expects `black_levels` to be [R, G1, G2, B].
    // So we need to map our measured phases to these color slots.
    
    let pattern_name = cfa.name.as_str();
    let mut ordered_blacks = [0.0; 4];
    
    if pattern_name == "RGGB" {
        ordered_blacks[0] = medians[0]; // R  (0,0)
        ordered_blacks[1] = medians[1]; // G1 (0,1)
        ordered_blacks[2] = medians[2]; // G2 (1,0)
        ordered_blacks[3] = medians[3]; // B  (1,1)
    } else if pattern_name == "GRBG" {
        ordered_blacks[0] = medians[1]; // R  (0,1)
        ordered_blacks[1] = medians[0]; // G1 (0,0)
        ordered_blacks[2] = medians[3]; // G2 (1,1)
        ordered_blacks[3] = medians[2]; // B  (1,0)
    } else if pattern_name == "GBRG" {
        ordered_blacks[0] = medians[2]; // R  (1,0)
        ordered_blacks[1] = medians[0]; // G1 (0,0)
        ordered_blacks[2] = medians[3]; // G2 (1,1)
        ordered_blacks[3] = medians[1]; // B  (0,1)
    } else if pattern_name == "BGGR" {
        ordered_blacks[0] = medians[3]; // R  (1,1)
        ordered_blacks[1] = medians[1]; // G1 (0,1)
        ordered_blacks[2] = medians[2]; // G2 (1,0)
        ordered_blacks[3] = medians[0]; // B  (0,0)
    } else {
        println!("⚠️  Unknown CFA pattern '{}', assuming RGGB mapping", pattern_name);
        ordered_blacks[0] = medians[0];
        ordered_blacks[1] = medians[1];
        ordered_blacks[2] = medians[2];
        ordered_blacks[3] = medians[3];
    }
    
    println!("📊 Measured Black Levels (Median): R={:.1}, G1={:.1}, G2={:.1}, B={:.1}", 
        ordered_blacks[0], ordered_blacks[1], ordered_blacks[2], ordered_blacks[3]);
        
    ordered_blacks
}
