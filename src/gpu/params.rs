use crate::core::types::{EditParams, MaskParams, MAX_MASKS};

/// `view_rect` covering the entire displayed image — the pre-windowing
/// behaviour, and what the histogram pass always uses so its result stays
/// whole-image regardless of where the display pass is looking.
pub const FULL_VIEW_RECT: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// GPU-friendly local adjustment mask: seven vec4s (112 bytes), so the WGSL
/// uniform array stride is a multiple of 16 and Pod has no padding holes.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuMask {
    /// x: type (MASK_*), y: enabled, z: invert, w: feather
    pub info: [f32; 4],
    /// linear: (ax, ay, bx, by); radial: (cx, cy, rx, ry)
    pub geom: [f32; 4],
    /// exposure, contrast, saturation, warmth
    pub adj0: [f32; 4],
    /// highlights, shadows, tint, rotation (radians)
    pub adj1: [f32; 4],
    /// hue (radians), has brush strokes, luma range on, colour range on
    pub adj2: [f32; 4],
    /// luma lo, luma hi, luma smooth, colour tolerance
    pub range: [f32; 4],
    /// colour target (ln r/g, ln b/g), unused ×2
    pub color: [f32; 4],
}

/// Largest hue rotation of the local Hue slider at ±1.
pub const MASK_HUE_RANGE_DEG: f32 = 30.0;

impl GpuMask {
    pub fn new(m: &MaskParams, has_brush: bool) -> Self {
        Self {
            info: [m.mask_type as f32, m.enabled as f32, m.invert as f32, m.feather],
            geom: [m.ax, m.ay, m.bx, m.by],
            adj0: [m.exposure, m.contrast, m.saturation, m.warmth],
            adj1: [m.highlights, m.shadows, m.tint, m.rotation.to_radians()],
            adj2: [
                (m.hue.clamp(-1.0, 1.0) * MASK_HUE_RANGE_DEG).to_radians(),
                if has_brush { 1.0 } else { 0.0 },
                m.luma_range as f32,
                m.color_range as f32,
            ],
            range: [m.luma_lo, m.luma_hi, m.luma_smooth.max(1e-3), m.color_tolerance],
            color: [m.color_target[0], m.color_target[1], 0.0, 0.0],
        }
    }
}

/// The mask uniform buffer (colour pass, group 1).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuMasks {
    pub masks: [GpuMask; MAX_MASKS],
}

impl GpuMasks {
    pub fn from_params(p: &EditParams) -> Self {
        Self {
            masks: std::array::from_fn(|i| GpuMask::new(&p.masks[i], p.brush.has_strokes_for(i))),
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
    /// With `src_scale`, maps full-image sensor UV to the INPUT texture's UV:
    /// `src_uv = uv * src_scale + src_offset`. The identity — `(1, 1)`,
    /// `(0, 0)`, exact in floating point — for every render except the
    /// detail loupe (`gpu::loupe`), whose input is a full-resolution crop.
    /// Two former pad slots; no offset moved.
    pub src_offset: [f32; 2],
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
    /// See `src_offset`. Occupies the vestigial Phase-25 `zoom`/`pan_x`
    /// slots (the viewport shader's projection owns zoom and pan); `pan_y`
    /// is still dead and only holds later offsets in place.
    pub src_scale: [f32; 2],
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
    /// Highlight-reconstruction correction `(k_r, k_g, k_b)`, CAMERA-NATIVE
    /// (`core::highlights`; the shader converts it to the current WB). Three
    /// former pad slots — no offset moved. Filled per image by
    /// `set_highlights`.
    highlight_k: [f32; 3],
    black_offsets: [f32; 4],
    black_phase_x: u32,
    black_phase_y: u32,
    luma_noise: f32,
    color_noise: f32,
    sharpening: f32,
    sharpen_masking: f32,
    pub rotation: f32,
    /// Export colour space (`OutputSpace::shader_id`): 0 = sRGB, the only
    /// value the display uses. Set per export by
    /// `render_functions::write_output_space`, never by `update_uniforms`.
    /// Former pad slot — no offset moved.
    pub output_space: u32,
    pub crop: [f32; 4],
    pub is_cropping: u32,
    pub has_dcp: u32,
    pub dcp_has_curve: u32,
    /// EXIF orientation (1/3/6/8) — per-image, set from ImageResources.
    pub orientation: u32,
    /// Number of active local adjustment masks (0..=8)
    pub mask_count: u32,
    /// Selected tone mapping operator (`ToneMapper::shader_id`).
    ///
    /// The shader only distinguishes `Camera` (0) from everything else — it
    /// decides whether the pre-HueSatMap knee runs, and which stage of the
    /// fallback path curves the image. The operator's actual SHAPE never
    /// reaches the GPU as parameters; it is baked into the tone curve LUT on
    /// the CPU (`core::tonemap::bake`), which is why adding five operators
    /// costs one u32 and zero per-pixel work.
    ///
    /// Claims one of three dead pad slots, so nothing moved and the struct is
    /// still 784 bytes — asserted below.
    pub tone_mapper: u32,
    /// 1 when any point curve is non-identity: the shader then samples the
    /// baked curve LUT (binding 6). Another former pad slot — no offsets moved.
    pub curve_active: u32,
    /// Mask overlay: 0 = off, i + 1 = tint mask i red over the render so its
    /// coverage is visible while painting or tuning ranges. Former pad slot.
    pub mask_overlay: u32,
    /// Lens correction (`core::lens::GpuLens`), evaluated in the shader's
    /// Step 0. Packed as vec4s so every field sits on a 16-byte boundary:
    /// dist = (p0, p1, p2, p3)
    pub lens_dist: [f32; 4],
    /// (p4, scale.x, scale.y, active 0/1)
    pub lens_misc: [f32; 4],
    /// red TCA (v, c, b), vignetting radius ratio
    pub lens_tca_r: [f32; 4],
    /// blue TCA (v, c, b), manual vignetting amount
    pub lens_tca_b: [f32; 4],
    /// PA vignetting (k1, k2, k3), 1 / output corner radius
    pub lens_vig: [f32; 4],
    /// Defringe: purple amount, purple hue°, green amount, green hue°
    pub defringe: [f32; 4],
    /// Defringe: active 0/1, edge search radius in texels, unused ×2
    pub defringe_misc: [f32; 4],
    /// Presence: texture, clarity, dehaze (each −1..1), unused. The bands
    /// Texture and Clarity scale come from the local-contrast pre-pass
    /// (`gpu::local_contrast`, bind group 2), which `prepare_stages` keeps
    /// current whenever either is non-zero — so the amounts alone say
    /// whether the shader may sample it.
    pub presence: [f32; 4],
    /// Dehaze airlight in CAMERA-NATIVE units (the shader multiplies by the
    /// current WB, like the pixel), and 1 when a haze map exists.
    pub haze: [f32; 4],
    // The masks themselves live in their own uniform buffer (`GpuMasks`,
    // bind group 1) next to the brush layers they sample.
}

impl GpuEditParams {
    /// Fill the lens block from a resolved `GpuLens`.
    pub fn set_lens(&mut self, g: &crate::core::lens::GpuLens) {
        self.lens_dist = [g.dist[0], g.dist[1], g.dist[2], g.dist[3]];
        self.lens_misc = [g.dist[4], g.scale[0], g.scale[1], if g.active { 1.0 } else { 0.0 }];
        self.lens_tca_r = [g.tca_r[0], g.tca_r[1], g.tca_r[2], g.vig_ratio];
        self.lens_tca_b = [g.tca_b[0], g.tca_b[1], g.tca_b[2], g.manual_vig];
        self.lens_vig = [g.vig[0], g.vig[1], g.vig[2], g.inv_corner];
    }

    /// The highlight-reconstruction correction of `core::highlights`,
    /// already camera-native.
    pub fn set_highlights(&mut self, correction: [f32; 3]) {
        self.highlight_k = correction;
    }

    /// The dehaze airlight, given in the analysis space of `core::haze`
    /// (white-balanced by `analysis_wb`); stored camera-native.
    pub fn set_haze(&mut self, airlight: [f32; 3], analysis_wb: [f32; 3]) {
        self.haze = [
            airlight[0] / analysis_wb[0],
            airlight[1] / analysis_wb[1],
            airlight[2] / analysis_wb[2],
            1.0,
        ];
    }

    /// Defringe's edge search radius, in texels of the working texture.
    /// Fringes are a few SENSOR pixels wide, so a subsampled preview needs a
    /// proportionally smaller radius.
    pub fn set_defringe_radius(&mut self, stride: f32) {
        self.defringe_misc[1] = (3.0 / stride.max(1.0)).round().clamp(1.0, 3.0);
    }
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
            src_offset: [0.0, 0.0],
            wb_multipliers: [1.0, 1.0, 1.0, 1.0],
            forward_matrix_0: [1.0, 0.0, 0.0],
            _padding3: 0.0,
            forward_matrix_1: [0.0, 1.0, 0.0],
            _padding4: 0.0,
            forward_matrix_2: [0.0, 0.0, 1.0],
            _padding5: 0.0,
            src_scale: [1.0, 1.0],
            pan_y: 0.0,
            cfa_pattern: 0,
            // Whole image by default; the windowed render path overwrites just
            // these 16 bytes per pass (see render_functions).
            view_rect: FULL_VIEW_RECT,
            black_levels: [0, 0, 0, 0],
            white_level: 65535,
            highlight_k: [0.0; 3],
            black_offsets: params.black_offsets,
            black_phase_x: params.black_phase_x,
            black_phase_y: params.black_phase_y,
            luma_noise: params.luma_noise,
            color_noise: params.color_noise,
            sharpening: params.sharpening,
            sharpen_masking: params.sharpen_masking,
            rotation: params.rotation,
            output_space: 0,
            crop: params.crop,
            is_cropping: params.is_cropping,
            has_dcp: 0,
            dcp_has_curve: 0,
            orientation: 1,
            mask_count: params.mask_count.min(MAX_MASKS as u32),
            tone_mapper: params.tone_mapper.shader_id(),
            curve_active: u32::from(!params.curves.is_identity()),
            mask_overlay: params.mask_overlay.min(MAX_MASKS as u32),
            lens_dist: [1.0, 0.0, 0.0, 0.0],
            lens_misc: [0.0, 1.0, 1.0, 0.0],
            lens_tca_r: [1.0, 0.0, 0.0, 1.0],
            lens_tca_b: [1.0, 0.0, 0.0, 0.0],
            lens_vig: [0.0, 0.0, 0.0, 1.0],
            defringe: [
                params.defringe.purple_amount.clamp(0.0, 1.0),
                params.defringe.purple_hue,
                params.defringe.green_amount.clamp(0.0, 1.0),
                params.defringe.green_hue,
            ],
            defringe_misc: [if params.defringe.is_active() { 1.0 } else { 0.0 }, 2.0, 0.0, 0.0],
            presence: [
                params.texture.clamp(-1.0, 1.0),
                params.clarity.clamp(-1.0, 1.0),
                params.dehaze.clamp(-1.0, 1.0),
                0.0,
            ],
            // Filled per image by `set_haze`.
            haze: [1.0, 1.0, 1.0, 0.0],
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
        assert_eq!(std::mem::size_of::<GpuMask>(), 112);
        assert_eq!(std::mem::size_of::<GpuMasks>(), 8 * 112);
        // 256 (base) + 16 (mask_count, tone_mapper, curve_active, overlay)
        // + 7 × 16 (lens + defringe) + 2 × 16 (presence + haze)
        assert_eq!(std::mem::size_of::<GpuEditParams>(), 416);
        assert_eq!(std::mem::offset_of!(GpuEditParams, lens_dist), 272);
        assert_eq!(std::mem::offset_of!(GpuEditParams, presence), 384);
        assert_eq!(std::mem::offset_of!(GpuEditParams, haze), 400);
    }

    #[test]
    fn presence_reaches_the_uniform_clamped() {
        let p = EditParams { texture: 0.4, clarity: -2.0, dehaze: 0.7, ..EditParams::default() };
        let g = GpuEditParams::from(&p);
        assert_eq!(g.presence, [0.4, -1.0, 0.7, 0.0]);
        assert_eq!(GpuEditParams::from(&EditParams::default()).presence, [0.0; 4]);
    }

    /// The airlight is measured white-balanced but the shader re-applies the
    /// CURRENT white balance, so it must be stored camera-native.
    #[test]
    fn haze_airlight_is_stored_camera_native() {
        let mut g = GpuEditParams::from(&EditParams::default());
        assert_eq!(g.haze[3], 0.0, "no haze map until one is set");
        g.set_haze([0.8, 0.6, 0.75], [2.0, 1.0, 1.5]);
        assert_eq!(g.haze, [0.4, 0.6, 0.5, 1.0]);
    }

    /// `tone_mapper` claimed a dead pad slot specifically so nothing moved.
    /// If it ever displaces `masks`, every mask reads garbage.
    #[test]
    fn tone_mapper_sits_beside_mask_count() {
        assert_eq!(
            std::mem::offset_of!(GpuEditParams, tone_mapper),
            std::mem::offset_of!(GpuEditParams, mask_count) + 4
        );
        // The lens block is vec4s; it must start on a 16-byte boundary.
        assert_eq!(std::mem::offset_of!(GpuEditParams, lens_dist) % 16, 0);
    }

    #[test]
    fn default_params_select_the_camera_operator() {
        let g = GpuEditParams::from(&EditParams::default());
        assert_eq!(g.tone_mapper, 0, "default must be the calibrated path");
    }

    #[test]
    fn tone_mapper_reaches_the_uniform() {
        let mut p = EditParams::default();
        p.tone_mapper = crate::core::tonemap::ToneMapper::Gt;
        let g = GpuEditParams::from(&p);
        assert_eq!(g.tone_mapper, crate::core::tonemap::ToneMapper::Gt.shader_id());
        assert_ne!(g.tone_mapper, 0);
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

    /// The loupe's source mapping took two pairs of dead slots; WGSL reads
    /// each pair as a `vec2<f32>`, which needs 8-byte alignment, and nothing
    /// around them may move.
    #[test]
    fn source_mapping_occupies_old_pad_slots() {
        assert_eq!(std::mem::offset_of!(GpuEditParams, src_offset), 40);
        assert_eq!(std::mem::offset_of!(GpuEditParams, wb_multipliers), 48);
        assert_eq!(std::mem::offset_of!(GpuEditParams, src_scale), 112);
        assert_eq!(std::mem::offset_of!(GpuEditParams, pan_y), 120);
        assert_eq!(std::mem::offset_of!(GpuEditParams, cfa_pattern), 124);
    }

    /// Every render except the loupe samples its input in full-image UV.
    #[test]
    fn default_source_mapping_is_the_identity() {
        let g = GpuEditParams::from(&EditParams::default());
        assert_eq!(g.src_scale, [1.0, 1.0]);
        assert_eq!(g.src_offset, [0.0, 0.0]);
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
        let g = GpuMask::new(&m, false);
        assert_eq!(g.info, [1.0, 1.0, 1.0, 0.5]);
        assert_eq!(g.geom, [0.5, 0.4, 0.3, 0.2]);
        assert_eq!(g.adj0[0], -1.5);
        assert_eq!(g.adj1[2], 0.4);
        // Rotation is converted to radians in the mapping, matching the
        // shader's convention (see mask_weight's aspect-corrected rotation).
        assert!((g.adj1[3] - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }
}
