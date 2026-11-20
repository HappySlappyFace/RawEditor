//! Highlights and Shadows: the CPU mirror of the colour shader's
//! `highlights_shadows_ev` (constants guarded by
//! `shader_tone_region_constants_match_rust`), where the properties the
//! shader relies on are unit-tested.
//!
//! Both sliders are a gain in stops, chosen by the level of a pixel's
//! *surroundings* (the Clarity base, `gpu::local_contrast`) in stops relative
//! to sensor white, and applied as a multiply. One gain per region keeps the
//! detail inside it; a multiply in scene-linear keeps hue, saturation and
//! black; and a gain that changes by less than a stop per stop keeps a
//! brighter surround brighter.

/// Highlights weigh in from here (stops re white; grey is near −2.5)…
pub const HS_HL_LO: f32 = -2.0;
/// …to full weight here, past clipping.
pub const HS_HL_HI: f32 = 0.5;
/// Stops a full-weight surround is pulled down at Highlights −1.
pub const HS_HL_RECOVER: f32 = 1.3;
/// Stops it is lifted at Highlights +1.
pub const HS_HL_LIFT: f32 = 0.6;
/// Shadows are at full weight below this…
pub const HS_SH_LO: f32 = -6.5;
/// …and gone by here, just above grey.
pub const HS_SH_HI: f32 = -2.0;
/// Stops a full-weight surround moves at Shadows ±1.
pub const HS_SH_LIFT: f32 = 1.6;

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The gain in stops for a surround at scene-linear `level`.
pub fn gain_ev(highlights: f32, shadows: f32, level: f32) -> f32 {
    let ev = level.max(1e-6).log2();
    let mut gain = 0.0;
    if highlights != 0.0 {
        let amount = if highlights < 0.0 { HS_HL_RECOVER } else { HS_HL_LIFT };
        gain += highlights * amount * smoothstep(HS_HL_LO, HS_HL_HI, ev);
    }
    if shadows != 0.0 {
        gain += shadows * HS_SH_LIFT * (1.0 - smoothstep(HS_SH_LO, HS_SH_HI, ev));
    }
    gain
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLIDERS: [f32; 5] = [-1.0, -0.5, 0.0, 0.5, 1.0];

    fn levels() -> impl Iterator<Item = f32> {
        // −12 … +3 stops re white, in 1/32-stop steps.
        (0..=480).map(|i| (-12.0 + i as f32 / 32.0).exp2())
    }

    #[test]
    fn zero_is_an_exact_identity() {
        for l in levels() {
            assert_eq!(gain_ev(0.0, 0.0, l), 0.0);
        }
    }

    /// A brighter surround must never come out darker than a dimmer one, or
    /// regions swap tone (a lifted shadow overtaking the midtones beside it).
    #[test]
    fn tonal_order_survives_every_setting() {
        for h in SLIDERS {
            for s in SLIDERS {
                let mut prev = f32::NEG_INFINITY;
                for l in levels() {
                    let out = l.log2() + gain_ev(h, s, l);
                    assert!(out > prev, "h {h} s {s}: order reversed at {} stops", l.log2());
                    prev = out;
                }
            }
        }
    }

    /// Each slider keeps to its own tones: Shadows leaves white alone,
    /// Highlights leaves the shadows alone, and grey moves little under
    /// either.
    #[test]
    fn each_slider_keeps_to_its_tones() {
        let grey = 0.18;
        let white = 1.0;
        let deep = (-7.0f32).exp2();
        assert_eq!(gain_ev(0.0, 1.0, white), 0.0);
        assert_eq!(gain_ev(-1.0, 0.0, deep), 0.0);
        assert!((gain_ev(0.0, 1.0, deep) - HS_SH_LIFT).abs() < 1e-6);
        assert!((gain_ev(-1.0, 0.0, 2.0) + HS_HL_RECOVER).abs() < 1e-6);
        for (h, s) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            assert!(gain_ev(h, s, grey).abs() < 0.15, "h {h} s {s} moves grey by {}", gain_ev(h, s, grey));
        }
    }

    /// Recovery brings a stop-past-white highlight back under white, where
    /// the tone curve still resolves gradation.
    #[test]
    fn full_recovery_lands_a_blown_highlight_below_white() {
        let out = 2.0 * gain_ev(-1.0, 0.0, 2.0).exp2();
        assert!((0.7..0.9).contains(&out), "2.0 → {out}");
    }
}
