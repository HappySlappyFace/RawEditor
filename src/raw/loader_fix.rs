    Ok(RawDataResult {
        data,
        width,
        height,
        wb_multipliers: wb_normalized,
        color_matrix: xyz_to_cam_matrix,  // Return xyz_to_cam, will convert in main.rs
        cfa_pattern,
        black_levels,
        white_level,
        crops: if raw_image.crops.len() == 4 {
            [raw_image.crops[0], raw_image.crops[1], raw_image.crops[2], raw_image.crops[3]]
        } else {
            [0, 0, 0, 0]
        },
        cfa_name: raw_image.cfa.name.clone(),
        measured_black_levels,
    })
}
