// Single source of truth for "where the displayed image is on screen" in the
// develop viewport. Previously the display shader, the crop/mask canvas
// overlay, and the zoom/pan/crop input math each computed this independently
// and disagreed at any zoom != 1.0 (and, for the raw-dims consumers, on any
// portrait image at all) — this module is the fix: everyone consumes the
// same `image_rect`.

use iced::Rectangle;

/// Letterbox-fit size of an image inside a viewport, at zoom = 1.0.
pub fn fitted_size(image_w: u32, image_h: u32, vp_w: f32, vp_h: f32) -> (f32, f32) {
    let img_aspect = image_w as f32 / image_h.max(1) as f32;
    let vp_aspect = vp_w / vp_h.max(1.0);
    if img_aspect > vp_aspect {
        (vp_w, vp_w / img_aspect)
    } else {
        (vp_h * img_aspect, vp_h)
    }
}

/// The on-screen rectangle the displayed image occupies inside a viewport
/// (widget-local logical pixels).
///
/// Model: letterbox-fit, grown by `zoom` about the viewport center, shifted
/// by `offset` in FITTED-SIZE FRACTIONS — shift_px = (offset.x * fitted_w,
/// offset.y * fitted_h), independent of zoom. This is the one pan
/// convention used everywhere: `Δoffset = Δcursor_px / fitted_px` gives 1:1
/// cursor tracking at any zoom with no zoom term in the pan math at all.
pub fn image_rect(
    image_w: u32,
    image_h: u32,
    vp_w: f32,
    vp_h: f32,
    zoom: f32,
    offset: cgmath::Vector2<f32>,
) -> Rectangle {
    let (fw, fh) = fitted_size(image_w, image_h, vp_w, vp_h);
    let (zw, zh) = (fw * zoom, fh * zoom);
    Rectangle {
        x: vp_w / 2.0 - zw / 2.0 + offset.x * fw,
        y: vp_h / 2.0 - zh / 2.0 + offset.y * fh,
        width: zw,
        height: zh,
    }
}

/// How much larger than the strictly-visible area the develop render covers.
///
/// Panning does not re-render (see `handle_pan`) — the viewport shader just
/// re-projects the existing texture — so a render covering exactly the visible
/// pixels would expose unrendered edges the moment the user drags. This margin
/// buys a band of free panning before a refresh is needed. Raising it costs
/// render area quadratically; lowering it makes pan refreshes more frequent.
pub const RENDER_OVERSCAN: f32 = 1.25;

/// The sub-region of the displayed image that needs rendering, as
/// `(u0, v0, du, dv)` in view UV — 0..1 across the fitted image rect.
///
/// Returns `(0,0,1,1)` whenever the whole image is on screen (any zoom ≤ fit),
/// which keeps the un-zoomed path byte-identical to the pre-windowing
/// behaviour. Zoomed in, it returns the visible slice grown by
/// `RENDER_OVERSCAN` and clamped back inside the image.
pub fn visible_view_rect(
    image_w: u32,
    image_h: u32,
    vp_w: f32,
    vp_h: f32,
    zoom: f32,
    offset: cgmath::Vector2<f32>,
    overscan: f32,
) -> [f32; 4] {
    let ib = image_rect(image_w, image_h, vp_w, vp_h, zoom, offset);
    if ib.width <= 0.0 || ib.height <= 0.0 {
        return [0.0, 0.0, 1.0, 1.0];
    }

    // Visible slice of the image rect, in view UV.
    let u0 = ((0.0 - ib.x) / ib.width).clamp(0.0, 1.0);
    let u1 = ((vp_w - ib.x) / ib.width).clamp(0.0, 1.0);
    let v0 = ((0.0 - ib.y) / ib.height).clamp(0.0, 1.0);
    let v1 = ((vp_h - ib.y) / ib.height).clamp(0.0, 1.0);

    // Degenerate (image entirely off-screen): render everything rather than
    // nothing, so there is always something sensible to show.
    if u1 <= u0 || v1 <= v0 {
        return [0.0, 0.0, 1.0, 1.0];
    }

    let grow = |a: f32, b: f32| -> (f32, f32) {
        let half = (b - a) * 0.5 * overscan;
        let mid = (a + b) * 0.5;
        ((mid - half).max(0.0), (mid + half).min(1.0))
    };
    let (gu0, gu1) = grow(u0, u1);
    let (gv0, gv1) = grow(v0, v1);

    [gu0, gv0, gu1 - gu0, gv1 - gv0]
}

/// True when `inner` lies entirely within `outer` (both `(u0, v0, du, dv)`).
///
/// Used after a pan to decide whether the already-rendered window still covers
/// what the user is looking at, or whether a refresh is due.
pub fn view_rect_contains(outer: [f32; 4], inner: [f32; 4]) -> bool {
    const EPS: f32 = 1e-4;
    inner[0] >= outer[0] - EPS
        && inner[1] >= outer[1] - EPS
        && inner[0] + inner[2] <= outer[0] + outer[2] + EPS
        && inner[1] + inner[3] <= outer[1] + outer[3] + EPS
}

/// UV→isotropic scale factor for mask geometry: the horizontal display pixels
/// spanned by one unit of full-image `u`, divided by the vertical pixels
/// spanned by one unit of `v`.
///
/// Mask geometry lives in full-image UV, but what's *displayed* is the crop
/// sub-rect stretched across the whole render target (the vertex shader remaps
/// tex coords through `params.crop`, and the target is always the full-image
/// aspect). So a crop whose aspect differs from the frame's rescales the two
/// axes unequally, and a rotated ellipse renders sheared — and at a visibly
/// different angle from the canvas overlay — unless that term is included.
/// At `crop = [0, 0, 1, 1]` this reduces to the plain display aspect.
pub fn mask_uv_aspect(image_w: u32, image_h: u32, crop: [f32; 4]) -> f32 {
    let display_aspect = image_w as f32 / image_h.max(1) as f32;
    display_aspect * (crop[3].max(1e-6) / crop[2].max(1e-6))
}

/// Normalize an angle in degrees into [-180, 180).
///
/// Rotation is cyclic: `atan2` deltas jump by ~360° across the ±180° branch
/// cut, and a raw clamp would make a continuous drag stick at the ends.
pub fn wrap_degrees(deg: f32) -> f32 {
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_offset() -> cgmath::Vector2<f32> {
        cgmath::Vector2::new(0.0, 0.0)
    }

    // ── visible_view_rect ────────────────────────────────────────────────────

    #[test]
    fn visible_view_rect_at_fit_covers_the_whole_image() {
        // The no-regression guarantee: un-zoomed rendering must be unchanged.
        let r = visible_view_rect(3000, 2000, 1600.0, 1000.0, 1.0, no_offset(), 1.25);
        assert_eq!(r, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn visible_view_rect_below_fit_still_covers_the_whole_image() {
        let r = visible_view_rect(3000, 2000, 1600.0, 1000.0, 0.5, no_offset(), 1.25);
        assert_eq!(r, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn visible_view_rect_zoomed_and_centred_is_centred_and_overscanned() {
        // 3000x2000 in a 1600x1000 viewport is height-limited, so the fitted
        // size is 1500x1000 — NOT 1600 wide. At zoom 4 the image spans
        // 6000x4000, so the visible fraction is 1600/6000 by 1000/4000.
        let visible_du = 1600.0 / (1500.0 * 4.0);
        let visible_dv = 1000.0 / (1000.0 * 4.0);
        let r = visible_view_rect(3000, 2000, 1600.0, 1000.0, 4.0, no_offset(), 1.25);
        assert!((r[2] - visible_du * 1.25).abs() < 1e-3, "du was {}", r[2]);
        assert!((r[3] - visible_dv * 1.25).abs() < 1e-3, "dv was {}", r[3]);
        // Centred pan, so the window is centred too.
        assert!((r[0] + r[2] / 2.0 - 0.5).abs() < 1e-3);
        assert!((r[1] + r[3] / 2.0 - 0.5).abs() < 1e-3);
    }

    #[test]
    fn visible_view_rect_clamps_at_the_image_edge() {
        // Panned hard to one corner: overscan must not push the window
        // outside [0,1], or the shader samples off the image.
        let r = visible_view_rect(
            3000,
            2000,
            1600.0,
            1000.0,
            4.0,
            cgmath::Vector2::new(2.0, 2.0),
            1.25,
        );
        assert!(r[0] >= 0.0 && r[1] >= 0.0, "origin {:?}", r);
        assert!(r[0] + r[2] <= 1.0 + 1e-4, "u overflow {:?}", r);
        assert!(r[1] + r[3] <= 1.0 + 1e-4, "v overflow {:?}", r);
    }

    #[test]
    fn visible_view_rect_shrinks_as_zoom_grows() {
        let a = visible_view_rect(3000, 2000, 1600.0, 1000.0, 2.0, no_offset(), 1.25);
        let b = visible_view_rect(3000, 2000, 1600.0, 1000.0, 8.0, no_offset(), 1.25);
        // This is the whole point: 4x the zoom renders ~1/4 the width.
        assert!(b[2] < a[2] / 3.0, "{} vs {}", b[2], a[2]);
    }

    // ── view_rect_contains ───────────────────────────────────────────────────

    #[test]
    fn view_rect_contains_accepts_a_small_pan_inside_the_margin() {
        let rendered = visible_view_rect(3000, 2000, 1600.0, 1000.0, 4.0, no_offset(), 1.25);
        // A pan small enough to stay inside the overscan band.
        let after = visible_view_rect(
            3000,
            2000,
            1600.0,
            1000.0,
            4.0,
            cgmath::Vector2::new(0.01, 0.0),
            1.0,
        );
        assert!(view_rect_contains(rendered, after));
    }

    #[test]
    fn view_rect_contains_rejects_a_pan_past_the_margin() {
        let rendered = visible_view_rect(3000, 2000, 1600.0, 1000.0, 4.0, no_offset(), 1.25);
        let after = visible_view_rect(
            3000,
            2000,
            1600.0,
            1000.0,
            4.0,
            cgmath::Vector2::new(0.5, 0.0),
            1.0,
        );
        assert!(!view_rect_contains(rendered, after));
    }

    #[test]
    fn view_rect_contains_is_reflexive() {
        let r = [0.2, 0.3, 0.4, 0.5];
        assert!(view_rect_contains(r, r));
    }

    #[test]
    fn mask_uv_aspect_uncropped_is_display_aspect() {
        let a = mask_uv_aspect(3000, 2000, [0.0, 0.0, 1.0, 1.0]);
        assert!((a - 1.5).abs() < 1e-4);
    }

    #[test]
    fn mask_uv_aspect_accounts_for_non_matching_crop() {
        // 16:9 crop out of a 3:2 frame: full width, 0.844 of the height.
        let a = mask_uv_aspect(3000, 2000, [0.0, 0.078, 1.0, 0.84375]);
        // 1.5 * 0.84375 / 1.0
        assert!((a - 1.265_625).abs() < 1e-4);
    }

    #[test]
    fn mask_uv_aspect_narrow_crop_stretches_u() {
        // Half-width crop of a 3:2 frame: the 1500px-wide region is stretched
        // back over the full-width target, doubling the pixels per unit u
        // while v is untouched.
        let a = mask_uv_aspect(3000, 2000, [0.0, 0.0, 0.5, 1.0]);
        assert!((a - 3.0).abs() < 1e-4);
    }

    #[test]
    fn wrap_degrees_passes_through_small_angles() {
        assert!((wrap_degrees(0.0) - 0.0).abs() < 1e-4);
        assert!((wrap_degrees(45.0) - 45.0).abs() < 1e-4);
        assert!((wrap_degrees(-90.0) + 90.0).abs() < 1e-4);
    }

    #[test]
    fn wrap_degrees_folds_branch_cut_jumps() {
        // The atan2 failure case: ~0.6 deg of motion reported as -359.4.
        assert!((wrap_degrees(-359.4) - 0.6).abs() < 1e-3);
        assert!((wrap_degrees(359.4) + 0.6).abs() < 1e-3);
    }

    #[test]
    fn wrap_degrees_is_cyclic_past_half_turn() {
        assert!((wrap_degrees(190.0) + 170.0).abs() < 1e-3);
        assert!((wrap_degrees(-190.0) - 170.0).abs() < 1e-3);
        assert!((wrap_degrees(540.0) + 180.0).abs() < 1e-3);
    }

    #[test]
    fn fitted_size_landscape_image_in_landscape_viewport() {
        let (w, h) = fitted_size(1600, 1000, 800.0, 800.0);
        // image is wider than viewport aspect -> width-limited
        assert!((w - 800.0).abs() < 0.01);
        assert!((h - 500.0).abs() < 0.01);
    }

    #[test]
    fn fitted_size_portrait_image_in_landscape_viewport() {
        let (w, h) = fitted_size(1000, 1600, 800.0, 800.0);
        // image is taller than viewport aspect -> height-limited
        assert!((h - 800.0).abs() < 0.01);
        assert!((w - 500.0).abs() < 0.01);
    }

    #[test]
    fn image_rect_zoom_one_offset_zero_is_centered_fitted_rect() {
        let r = image_rect(1000, 1600, 800.0, 800.0, 1.0, cgmath::Vector2::new(0.0, 0.0));
        assert!((r.width - 500.0).abs() < 0.01);
        assert!((r.height - 800.0).abs() < 0.01);
        // centered: x = vp_w/2 - w/2
        assert!((r.x - 150.0).abs() < 0.01);
        assert!((r.y - 0.0).abs() < 0.01);
    }

    #[test]
    fn image_rect_zoom_grows_the_rect_about_center() {
        let base = image_rect(1000, 1600, 800.0, 800.0, 1.0, cgmath::Vector2::new(0.0, 0.0));
        let zoomed = image_rect(1000, 1600, 800.0, 800.0, 2.0, cgmath::Vector2::new(0.0, 0.0));
        assert!((zoomed.width - base.width * 2.0).abs() < 0.01);
        assert!((zoomed.height - base.height * 2.0).abs() < 0.01);
        // center stays fixed
        let base_cx = base.x + base.width / 2.0;
        let zoomed_cx = zoomed.x + zoomed.width / 2.0;
        assert!((base_cx - zoomed_cx).abs() < 0.01);
    }

    #[test]
    fn image_rect_offset_shifts_by_fitted_fraction_independent_of_zoom() {
        let (fw, _) = fitted_size(1600, 1000, 800.0, 800.0);
        let r1 = image_rect(1600, 1000, 800.0, 800.0, 1.0, cgmath::Vector2::new(0.5, 0.0));
        let r0 = image_rect(1600, 1000, 800.0, 800.0, 1.0, cgmath::Vector2::new(0.0, 0.0));
        assert!((r1.x - r0.x - 0.5 * fw).abs() < 0.01);

        // Shift in fitted-fractions is the same physical distance regardless of zoom.
        let r1z = image_rect(1600, 1000, 800.0, 800.0, 3.0, cgmath::Vector2::new(0.5, 0.0));
        let r0z = image_rect(1600, 1000, 800.0, 800.0, 3.0, cgmath::Vector2::new(0.0, 0.0));
        assert!((r1z.x - r0z.x - 0.5 * fw).abs() < 0.01);
    }
}
