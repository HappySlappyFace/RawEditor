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
