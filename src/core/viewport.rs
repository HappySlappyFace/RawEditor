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

#[cfg(test)]
mod tests {
    use super::*;

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
