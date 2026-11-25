//! The detail loupe, the app's side: its pan and placement inputs. The
//! loupe and the noise-reduction hold are the session's (`session::loupe`).

use iced::Task;

use crate::app::message::Message;
use crate::app::state::{Modal, RawEditor};
use crate::app::update::run;

pub use crate::session::LOUPE_SIZE;

pub fn handle_pan(editor: &mut RawEditor, dx: f32, dy: f32) -> Task<Message> {
    run(editor.session.pan_loupe(dx, dy))
}

pub fn handle_toggle_target(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    editor.session.toggle_loupe_target();
    Task::none()
}

pub fn handle_target_picked(editor: &mut RawEditor, u: f32, v: f32) -> Task<Message> {
    run(editor.session.loupe_target_picked(u, v))
}
