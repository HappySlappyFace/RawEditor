//! The sliders the phone shows: which edit field each moves, its range and
//! how its value reads. Ranges follow the desktop's sidebar
//! (`views::develop::view_sidebar`), so an edit made on one shows the same on
//! the other. Kotlin builds its controls from [`spec`] and refers to them by
//! index.

use raw_editor_session::core::types::EditParams;

pub struct Control {
    pub section: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub get: fn(&EditParams) -> f32,
    pub set: fn(&mut EditParams, f32),
    /// The value as shown; white balance reads against the photo's as-shot
    /// (kelvin, tint).
    pub show: fn(f32, (f32, f32)) -> String,
    /// What the rail depicts, as the desktop's `rail_gradient`: `Rail::Plain`
    /// for most, a gradient of the effect for a few.
    pub rail: Rail,
    /// A noise-reduction slider: it goes through `Session::noise_changed`,
    /// as on the desktop, so the view keeps its noise reduction while the
    /// finger moves (the stage costs ~10× any other) and takes the new
    /// value once it pauses or lifts.
    pub noise: bool,
}

/// A slider's rail, named for the UI, which holds the colours (the
/// desktop's, `views::develop::rail_gradient`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Rail {
    Plain,
    DarkToLight,
    CoolToWarm,
    GreenToMagenta,
    GreyToColour,
}

impl Rail {
    pub fn name(self) -> &'static str {
        match self {
            Rail::Plain => "plain",
            Rail::DarkToLight => "dark-light",
            Rail::CoolToWarm => "cool-warm",
            Rail::GreenToMagenta => "green-magenta",
            Rail::GreyToColour => "grey-colour",
        }
    }
}

fn plain(v: f32, _: (f32, f32)) -> String {
    format!("{v:+.2}")
}

fn ev(v: f32, _: (f32, f32)) -> String {
    format!("{v:+.2} EV")
}

fn kelvin(v: f32, (k, _): (f32, f32)) -> String {
    let p = EditParams { temperature: v, ..Default::default() };
    format!("{:.0} K", p.kelvin_from_anchor(k))
}

fn tint(v: f32, (_, t): (f32, f32)) -> String {
    let p = EditParams { tint: v, ..Default::default() };
    format!("{:+.0}", p.tint_from_anchor(t))
}

fn degrees(v: f32, _: (f32, f32)) -> String {
    format!("{v:+.1}°")
}

fn whole(v: f32, _: (f32, f32)) -> String {
    format!("{v:+.0}")
}

macro_rules! control {
    ($section:literal, $label:literal, $($field:ident).+, $min:expr, $max:expr, $show:expr) => {
        control!($section, $label, $($field).+, $min, $max, $show, Rail::Plain)
    };
    ($section:literal, $label:literal, $($field:ident).+, $min:expr, $max:expr, $show:expr, $rail:expr) => {
        Control {
            section: $section,
            label: $label,
            min: $min,
            max: $max,
            get: |p| p.$($field).+,
            set: |p, v| p.$($field).+ = v,
            show: $show,
            rail: $rail,
            noise: false,
        }
    };
}

pub const CONTROLS: &[Control] = &[
    control!("Light", "Exposure", exposure, -5.0, 5.0, ev, Rail::DarkToLight),
    control!("Light", "Contrast", contrast, -10.0, 10.0, plain),
    control!("Light", "Highlights", highlights, -1.0, 1.0, plain),
    control!("Light", "Shadows", shadows, -1.0, 1.0, plain),
    control!("Light", "Whites", whites, 0.8, 1.2, plain),
    control!("Light", "Blacks", blacks, 0.0, 0.2, plain),
    control!("Colour", "Temp", temperature, -1.0, 1.0, kelvin, Rail::CoolToWarm),
    control!("Colour", "Tint", tint, -1.0, 1.0, tint, Rail::GreenToMagenta),
    control!("Colour", "Vibrance", vibrance, -1.0, 1.0, plain, Rail::GreyToColour),
    control!("Colour", "Saturation", saturation, -100.0, 100.0, whole, Rail::GreyToColour),
    control!("Presence", "Texture", texture, -1.0, 1.0, plain),
    control!("Presence", "Clarity", clarity, -1.0, 1.0, plain),
    control!("Presence", "Dehaze", dehaze, -1.0, 1.0, plain),
    // Detail and Lens as the desktop's sidebar has them. Lens profile
    // corrections need the lensfun database, which the phone doesn't have
    // yet: the manual corrections and defringe work without it. Purple and
    // Green are the defringe amounts, named as on the desktop.
    Control { noise: true, ..control!("Detail", "Luminance", luma_noise, 0.0, 1.0, plain) },
    Control { noise: true, ..control!("Detail", "Detail", luma_detail, 0.0, 1.0, plain) },
    Control { noise: true, ..control!("Detail", "Colour", color_noise, 0.0, 1.0, plain) },
    control!("Detail", "Sharpening", sharpening, 0.0, 2.0, plain),
    control!("Detail", "Masking", sharpen_masking, 0.0, 1.0, plain),
    control!("Lens", "Distortion", lens.distortion, -1.0, 1.0, plain),
    control!("Lens", "Vignetting", lens.vignetting, -1.0, 1.0, plain),
    control!("Lens", "Red/Cyan", lens.ca_red, -1.0, 1.0, plain),
    control!("Lens", "Blue/Yellow", lens.ca_blue, -1.0, 1.0, plain),
    control!("Lens", "Purple", defringe.purple_amount, 0.0, 1.0, plain),
    control!("Lens", "Green", defringe.green_amount, 0.0, 1.0, plain),
    control!("Crop", "Straighten", rotation, -45.0, 45.0, degrees),
];

/// One line per control, in index order: `section, label, min, max,
/// default, rail`, tab-separated.
pub fn spec() -> String {
    let d = EditParams::default();
    CONTROLS
        .iter()
        .map(|c| format!("{}\t{}\t{}\t{}\t{}\t{}", c.section, c.label, c.min, c.max, (c.get)(&d), c.rail.name()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every control's value in `p`, in index order.
pub fn values(p: &EditParams) -> Vec<f32> {
    CONTROLS.iter().map(|c| (c.get)(p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_sit_inside_their_ranges() {
        let d = EditParams::default();
        for c in CONTROLS {
            let v = (c.get)(&d);
            assert!(c.min <= v && v <= c.max, "{} default {v} outside {}..{}", c.label, c.min, c.max);
        }
    }

    #[test]
    fn each_control_moves_its_own_field() {
        for (i, c) in CONTROLS.iter().enumerate() {
            let mut p = EditParams::default();
            let v = c.min + (c.max - c.min) * 0.75;
            (c.set)(&mut p, v);
            assert_eq!((c.get)(&p), v);
            for (j, other) in CONTROLS.iter().enumerate() {
                if j != i {
                    assert_eq!((other.get)(&p), (other.get)(&EditParams::default()), "{} moved {}", c.label, other.label);
                }
            }
        }
    }

    #[test]
    fn white_balance_reads_against_the_as_shot_values() {
        assert_eq!(kelvin(0.0, (5200.0, 3.0)), "5200 K");
        assert_eq!(tint(0.0, (5200.0, 3.0)), "+3");
    }
}
