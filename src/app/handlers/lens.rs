// Lens corrections and defringe: profile toggles, manual sliders.
//
// Slider changes ride `update_pipeline` like every other slider (the undo
// step comes from the slider's on_release → CommitEdit); toggles are discrete
// and commit immediately.

use crate::app::handlers::develop::update_pipeline;
use crate::app::message::{DefringeParam, LensParam, LensToggle, Message};
use crate::app::state::RawEditor;
use iced::Task;

pub fn handle_lens_param_changed(editor: &mut RawEditor, which: LensParam, v: f32) -> Task<Message> {
    let l = &mut editor.session.current_edit_params.lens;
    match which {
        LensParam::ProfileDistortion => l.profile_distortion = v.clamp(0.0, 2.0),
        LensParam::ProfileVignetting => l.profile_vignetting = v.clamp(0.0, 2.0),
        LensParam::Distortion => l.distortion = v.clamp(-1.0, 1.0),
        LensParam::Vignetting => l.vignetting = v.clamp(-1.0, 1.0),
        LensParam::CaRed => l.ca_red = v.clamp(-1.0, 1.0),
        LensParam::CaBlue => l.ca_blue = v.clamp(-1.0, 1.0),
    }
    update_pipeline(editor)
}

pub fn handle_lens_toggled(editor: &mut RawEditor, which: LensToggle, on: bool) -> Task<Message> {
    let l = &mut editor.session.current_edit_params.lens;
    match which {
        LensToggle::Profile => l.profile = on,
        LensToggle::ProfileCa => l.profile_ca = on,
        LensToggle::Constrain => l.constrain = on,
    }
    let task = update_pipeline(editor);
    editor.commit_current_state();
    task
}

pub fn handle_defringe_changed(editor: &mut RawEditor, which: DefringeParam, v: f32) -> Task<Message> {
    let d = &mut editor.session.current_edit_params.defringe;
    match which {
        DefringeParam::PurpleAmount => d.purple_amount = v.clamp(0.0, 1.0),
        DefringeParam::PurpleHue => d.purple_hue = v.rem_euclid(360.0),
        DefringeParam::GreenAmount => d.green_amount = v.clamp(0.0, 1.0),
        DefringeParam::GreenHue => d.green_hue = v.rem_euclid(360.0),
    }
    update_pipeline(editor)
}
