//! Lens corrections and defringe (`session::lens`).

use crate::app::message::{DefringeParam, LensParam, LensToggle, Message};
use crate::app::state::RawEditor;
use crate::app::update::run;
use iced::Task;

pub fn handle_lens_param_changed(editor: &mut RawEditor, which: LensParam, v: f32) -> Task<Message> {
    run(editor.session.set_lens_param(which, v))
}

pub fn handle_lens_toggled(editor: &mut RawEditor, which: LensToggle, on: bool) -> Task<Message> {
    run(editor.session.set_lens_toggle(which, on))
}

pub fn handle_defringe_changed(editor: &mut RawEditor, which: DefringeParam, v: f32) -> Task<Message> {
    run(editor.session.set_defringe(which, v))
}
