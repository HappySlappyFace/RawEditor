use crate::core::types::{EditParams, MaskParams, MAX_MASKS};

/// `view_rect` covering the entire displayed image — the pre-windowing
/// behaviour, and what the histogram pass always uses so its result stays
/// whole-image regardless of where the display pass is looking.
pub const FULL_VIEW_RECT: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// GPU-friendly local adjustment mask: four vec4s (64 bytes), so the WGSL
/// uniform array stride is a multiple of 16 and Pod has no padding holes.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuMask {
    /// x: type (0 linear / 1 radial), y: enabled, z: invert, w: feather
    pub info: [f32; 4],
    /// linear: (ax, ay, bx, by); radial: (cx, cy, rx, ry)
    pub geom: [f32; 4],
    /// exposure, contrast, saturation, warmth
    pub adj0: [f32; 4],
    /// highlights, shadows, tint, rotation (radians)
    pub adj1: [f32; 4],
}

impl From<&MaskParams> for GpuMask {
    fn from(m: &MaskParams) -> Self {
        Self {
            info: [m.mask_type as f32, m.enabled as f32, m.invert as f32, m.feather],
            geom: [m.ax, m.ay, m.bx, m.by],
            adj0: [m.exposure, m.contrast, m.saturation, m.warmth],
            adj1: [m.highlights, m.shadows, m.tint, m.rotation.to_radians()],
        }
    }
}

/// GPU-friendly representation of edit parameters.
/// Must match the WGSL struct layout with strict 16-byte alignment.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuEditParams {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    padding1: f32,
    padding2: f32,
    pub wb_multipliers: [f32; 4],
    /// Camera RGB → working space, one row per field (the shader rebuilds it
    /// with `transpose(mat3x3(...))`, so these are ROWS, not columns).
    ///
    /// Polymorphic by design — the destination space depends on `has_dcp`:
    ///   • `has_dcp == 1` → camera → **ProPhoto** (the DCP ForwardMatrix with
    ///     XYZ D50 → ProPhoto already folded in; see `raw::dcp`)
    ///   • `has_dcp == 0` → camera → **linear sRGB** (`calculate_cam_to_srgb`)
    /// Both paths therefore cost exactly one matrix multiply per pixel.
    pub forward_matrix_0: [f32; 3],
    _padding3: f32,
    pub forward_matrix_1: [f32; 3],
    _padding4: f32,
    pub forward_matrix_2: [f32; 3],
    _padding5: f32,
    /// Vestigial (Phase 25). Zoom and pan are applied by the viewport shader's
    /// projection matrix, not here; these have been written as 1.0/0.0/0.0
    /// since the viewport took over. Kept only so every later field's byte
    /// offset stays put — see `view_rect` below.
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub cfa_pattern: u32,
    /// Sub-region of the displayed image this render covers, as
    /// `(u0, v0, du, dv)` in VIEW UV — 0..1 across the post-crop image.
    /// `(0, 0, 1, 1)` means "the whole thing", which is what every pre-windowing
    /// caller wants.
    ///
    /// Occupies what used to be four dead `_pad_cfa_*` floats. That slot was
    /// already 16-byte aligned and 16 bytes wide, so adopting it changes
    /// neither the struct size nor any other field's offset — the safest place
    /// to add a `vec4` given the alignment constraint in CLAUDE.md. The size
    /// and offset are both asserted in the tests below.
    pub view_rect: [f32; 4],
    pub black_levels: [u32; 4],
    pub white_level: u32,
    _pad_end_1: f32,
    _pad_end_2: f32,
    _pad_end_3: f32,
    black_offsets: [f32; 4],
    black_phase_x: u32,
    black_phase_y: u32,
    luma_noise: f32,
    color_noise: f32,
    sharpening: f32,
    sharpen_masking: f32,
    rotation: f32,
    _pad_phase_1: f32,
    pub crop: [f32; 4],
    pub is_cropping: u32,
    pub has_dcp: u32,
    pub dcp_has_curve: u32,
    /// EXIF orientation (1/3/6/8) — per-image, set from ImageResources.
    pub orientation: u32,
    /// Number of active local adjustment masks (0..=8)
    pub mask_count: u32,
    _pad_masks_1: u32,
    _pad_masks_2: u32,
    _pad_masks_3: u32,
    /// Local adjustment masks; only the first `mask_count` are evaluated.
    pub masks: [GpuMask; MAX_MASKS],
}

impl From<&EditParams> for GpuEditParams {
    fn from(params: &EditParams) -> Self {
        Self {
            exposure: params.exposure,
            contrast: params.contrast,
            highlights: params.highlights,
            shadows: params.shadows,
            whites: params.whites,
            blacks: params.blacks,
            vibrance: params.vibrance,
            saturation: params.saturation,
            temperature: params.temperature,
            tint: params.tint,
            padding1: 0.0,
            padding2: 0.0,
            wb_multipliers: [1.0, 1.0, 1.0, 1.0],
            forward_matrix_0: [1.0, 0.0, 0.0],
            _padding3: 0.0,
            forward_matrix_1: [0.0, 1.0, 0.0],
            _padding4: 0.0,
            forward_matrix_2: [0.0, 0.0, 1.0],
            _padding5: 0.0,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            cfa_pattern: 0,
            // Whole image by default; the windowed render path overwrites just
            // these 16 bytes per pass (see render_functions).
            view_rect: FULL_VIEW_RECT,
            black_levels: [0, 0, 0, 0],
            white_level: 65535,
            _pad_end_1: 0.0,
            _pad_end_2: 0.0,
            _pad_end_3: 0.0,
            black_offsets: params.black_offsets,
            black_phase_x: params.black_phase_x,
            black_phase_y: params.black_phase_y,
            luma_noise: params.luma_noise,
            color_noise: params.color_noise,
            sharpening: params.sharpening,
            sharpen_masking: params.sharpen_masking,
            rotation: params.rotation,
            _pad_phase_1: 0.0,
            crop: params.crop,
            is_cropping: params.is_cropping,
            has_dcp: 0,
            dcp_has_curve: 0,
            orientation: 1,
            mask_count: params.mask_count.min(MAX_MASKS as u32),
            _pad_masks_1: 0,
            _pad_masks_2: 0,
            _pad_masks_3: 0,
            masks: std::array::from_fn(|i| GpuMask::from(&params.masks[i])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The WGSL EditParams structs in BOTH shaders alias this exact layout in
    /// one shared uniform buffer — a size drift here corrupts GPU memory.
    #[test]
    fn gpu_struct_sizes_match_wgsl_layout() {
        assert_eq!(std::mem::size_of::<GpuMask>(), 64);
        // 256 (base) + 16 (mask_count + pad) + 8 × 64 (masks)
        assert_eq!(std::mem::size_of::<GpuEditParams>(), 784);
    }

    /// `view_rect` replaced four dead padding floats specifically so that
    /// nothing else moved. If this offset shifts, the partial `write_buffer`
    /// in the render path silently overwrites the wrong fields.
    #[test]
    fn view_rect_occupies_the_old_padding_slot() {
        let off = std::mem::offset_of!(GpuEditParams, view_rect);
        // Immediately after cfa_pattern, which completes its own 16-byte block.
        assert_eq!(off, std::mem::offset_of!(GpuEditParams, cfa_pattern) + 4);
        // A vec4 must sit on a 16-byte boundary or the GPU reads garbage.
        assert_eq!(off % 16, 0);
        // And it must not have pushed black_levels off its own boundary.
        assert_eq!(std::mem::offset_of!(GpuEditParams, black_levels), off + 16);
    }

    #[test]
    fn default_view_rect_covers_the_whole_image() {
        let g = GpuEditParams::from(&EditParams::default());
        assert_eq!(g.view_rect, FULL_VIEW_RECT);
    }

    #[test]
    fn mask_mapping_preserves_fields() {
        let mut m = MaskParams::new_radial(0.5, 0.4, 0.3, 0.2);
        m.exposure = -1.5;
        m.invert = 1;
        m.tint = 0.4;
        m.rotation = 90.0;
        let g = GpuMask::from(&m);
        assert_eq!(g.info, [1.0, 1.0, 1.0, 0.5]);
        assert_eq!(g.geom, [0.5, 0.4, 0.3, 0.2]);
        assert_eq!(g.adj0[0], -1.5);
        assert_eq!(g.adj1[2], 0.4);
        // Rotation is converted to radians in the mapping, matching the
        // shader's convention (see mask_weight's aspect-corrected rotation).
        assert!((g.adj1[3] - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }
}
