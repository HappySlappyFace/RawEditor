//! Lens corrections and defringe: profile toggles, manual sliders.
//!
//! Slider changes ride `update_pipeline` like every other slider (the undo
//! step comes from the slider's release); toggles are discrete and commit
//! immediately.

use super::{Effects, Session};

/// Which lens-correction slider changed (see `LensParamChanged`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensParam {
    ProfileDistortion,
    ProfileVignetting,
    Distortion,
    Vignetting,
    CaRed,
    CaBlue,
}

/// Which lens-correction checkbox changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensToggle {
    Profile,
    ProfileCa,
    Constrain,
}

/// Which defringe slider changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefringeParam {
    PurpleAmount,
    PurpleHue,
    GreenAmount,
    GreenHue,
}

impl Session {
    pub fn set_lens_param(&mut self, which: LensParam, v: f32) -> Effects {
        self.edit(|p| {
            let l = &mut p.lens;
            match which {
                LensParam::ProfileDistortion => l.profile_distortion = v.clamp(0.0, 2.0),
                LensParam::ProfileVignetting => l.profile_vignetting = v.clamp(0.0, 2.0),
                LensParam::Distortion => l.distortion = v.clamp(-1.0, 1.0),
                LensParam::Vignetting => l.vignetting = v.clamp(-1.0, 1.0),
                LensParam::CaRed => l.ca_red = v.clamp(-1.0, 1.0),
                LensParam::CaBlue => l.ca_blue = v.clamp(-1.0, 1.0),
            }
        })
    }

    pub fn set_lens_toggle(&mut self, which: LensToggle, on: bool) -> Effects {
        self.edit_and_commit(|p| {
            let l = &mut p.lens;
            match which {
                LensToggle::Profile => l.profile = on,
                LensToggle::ProfileCa => l.profile_ca = on,
                LensToggle::Constrain => l.constrain = on,
            }
        })
    }

    pub fn set_defringe(&mut self, which: DefringeParam, v: f32) -> Effects {
        self.edit(|p| {
            let d = &mut p.defringe;
            match which {
                DefringeParam::PurpleAmount => d.purple_amount = v.clamp(0.0, 1.0),
                DefringeParam::PurpleHue => d.purple_hue = v.rem_euclid(360.0),
                DefringeParam::GreenAmount => d.green_amount = v.clamp(0.0, 1.0),
                DefringeParam::GreenHue => d.green_hue = v.rem_euclid(360.0),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::testkit::{open_in_develop, session_with_images};

    fn session() -> Session {
        let mut s = session_with_images(1);
        let _ = open_in_develop(&mut s, 1);
        s
    }

    #[test]
    fn sliders_stay_in_range_and_hues_wrap() {
        let mut s = session();
        let _ = s.set_lens_param(LensParam::ProfileDistortion, 5.0);
        let _ = s.set_lens_param(LensParam::Vignetting, -3.0);
        let _ = s.set_lens_param(LensParam::CaBlue, 0.25);
        let _ = s.set_defringe(DefringeParam::PurpleAmount, 1.5);
        let _ = s.set_defringe(DefringeParam::PurpleHue, 370.0);
        let _ = s.set_defringe(DefringeParam::GreenHue, -30.0);
        let (l, d) = (s.current_edit_params.lens, s.current_edit_params.defringe);
        assert_eq!((l.profile_distortion, l.vignetting, l.ca_blue), (2.0, -1.0, 0.25));
        assert_eq!(d.purple_amount, 1.0);
        assert!((d.purple_hue - 10.0).abs() < 1e-4 && (d.green_hue - 330.0).abs() < 1e-4, "{d:?}");
    }

    /// A slider's undo step comes from its release; a checkbox has none, so
    /// each toggle is a step of its own.
    #[test]
    fn a_toggle_is_an_undo_step_and_a_slider_tick_is_not() {
        let mut s = session();
        let start = s.current_edit_params.lens;
        let _ = s.set_lens_toggle(LensToggle::Constrain, !start.constrain);
        let _ = s.set_lens_param(LensParam::Distortion, 0.5);
        assert!(s.edits_dirty, "the slider change would not be saved");
        let _ = s.undo();
        assert_eq!(s.current_edit_params.lens.constrain, start.constrain, "the toggle was not a step");
        assert_eq!(s.current_edit_params.lens.distortion, start.distortion);
    }
}
