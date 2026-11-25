//! Update checks: the app's side — the first-run prompt, the dialogs and the
//! saved setting. The checking itself is the session's (`session::updates`).

use iced::Task;

use crate::app::message::Message;
use crate::app::state::{Modal, RawEditor};
use crate::app::update::run;
use crate::session::UpdatesAtStartup;

/// Once the catalogue is open: ask the first-run question, or run the
/// startup check if the user agreed to it.
pub fn on_startup(editor: &mut RawEditor) -> Task<Message> {
    match editor.session.updates_at_startup() {
        UpdatesAtStartup::Ask => {
            if editor.active_modal == Modal::None {
                editor.active_modal = Modal::UpdatePrompt;
            }
            Task::none()
        }
        UpdatesAtStartup::Check(effects) => run(effects),
        UpdatesAtStartup::Nothing => Task::none(),
    }
}

/// Help › Check for Updates opens the result dialog. Preferences' Check Now
/// keeps Preferences open and shows the result there.
pub fn handle_check(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::Preferences {
        editor.active_modal = Modal::Update;
    }
    run(editor.session.request_update_check())
}

pub fn handle_prompt_toggled(editor: &mut RawEditor, on: bool) -> Task<Message> {
    editor.update_prompt_choice = on;
    Task::none()
}

/// Save the first-run answer and, if it is yes, check straight away: this is
/// the startup the user just agreed to.
pub fn handle_prompt_confirmed(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::UpdatePrompt {
        return Task::none();
    }
    let effects = editor.session.answer_update_prompt(editor.update_prompt_choice);
    editor.save_preferences();
    editor.active_modal = Modal::None;
    run(effects)
}

pub fn handle_toggle(editor: &mut RawEditor) -> Task<Message> {
    editor.session.toggle_update_check();
    editor.save_preferences();
    Task::none()
}

pub fn handle_open_page(editor: &mut RawEditor) -> Task<Message> {
    let url = editor.session.update_page_url();
    crate::app::handlers::window::open_url(&url);
    editor.session.status = format!("Opened {url}");
    editor.active_modal = Modal::None;
    Task::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> RawEditor {
        let (mut e, _) = RawEditor::new();
        e.session.update_check = None;
        e.active_modal = Modal::None;
        e
    }

    fn available() -> crate::core::updates::Available {
        use crate::core::updates::Available;
        Available { version: "9.9.9".into(), url: "https://github.com/HappySlappyFace/RawEditor/releases/tag/v9.9.9".into(), published: String::new() }
    }

    #[test]
    fn first_startup_asks_and_makes_no_request() {
        let mut e = editor();
        let _ = on_startup(&mut e);
        assert_eq!(e.active_modal, Modal::UpdatePrompt);
        assert!(!e.session.updates.checking, "no request before the user answers");
        assert!(e.update_prompt_choice, "the prompt starts at yes");
    }

    #[test]
    fn declining_never_checks() {
        let mut e = editor();
        e.session.update_check = Some(false);
        let _ = on_startup(&mut e);
        assert_eq!(e.active_modal, Modal::None);
        assert!(!e.session.updates.checking);
    }

    #[test]
    fn an_automatic_result_lights_the_badge_without_a_dialog() {
        let mut e = editor();
        let _ = crate::app::update::update(&mut e, Message::Session(crate::session::SessionEvent::UpdateChecked { result: Ok(Some(available())), manual: false }));
        assert_eq!(e.session.updates.available, Some(available()));
        assert_eq!(e.active_modal, Modal::None);
        assert!(e.session.updates.shown.is_none());
        // A later up-to-date answer clears it.
        let _ = crate::app::update::update(&mut e, Message::Session(crate::session::SessionEvent::UpdateChecked { result: Ok(None), manual: false }));
        assert!(e.session.updates.available.is_none());
    }

    #[test]
    fn a_manual_check_shows_its_result_and_keeps_preferences_open() {
        let mut e = editor();
        let _ = handle_check(&mut e);
        assert_eq!(e.active_modal, Modal::Update);
        let _ = crate::app::update::update(&mut e, Message::Session(crate::session::SessionEvent::UpdateChecked { result: Err("offline".into()), manual: true }));
        assert_eq!(e.session.updates.shown, Some(Err("offline".into())));
        assert!(!e.session.updates.checking);

        e.active_modal = Modal::Preferences;
        let _ = handle_check(&mut e);
        assert_eq!(e.active_modal, Modal::Preferences);
        assert!(e.session.updates.shown.is_none(), "a new check clears the old result");
    }

    #[test]
    fn answering_the_prompt_saves_the_choice() {
        let mut e = editor();
        let _ = on_startup(&mut e);
        let _ = handle_prompt_toggled(&mut e, false);
        let _ = handle_prompt_confirmed(&mut e);
        assert_eq!(e.session.update_check, Some(false));
        assert_eq!(e.active_modal, Modal::None);
        assert!(!e.session.updates.checking);
    }
}

#[cfg(test)]
mod manual_click_tests {
    use super::*;

    /// A click on Check for Updates while the startup check runs still gets
    /// a result: the startup check's.
    #[test]
    fn a_click_during_the_startup_check_shows_its_result() {
        let (mut e, _) = RawEditor::new();
        e.session.updates.checking = true; // the startup check, in flight
        let _ = handle_check(&mut e);
        let _ = crate::app::update::update(&mut e, Message::Session(crate::session::SessionEvent::UpdateChecked { result: Ok(None), manual: false }));
        assert!(matches!(e.session.updates.shown, Some(Ok(None))));
        assert!(!e.session.updates.manual_pending);
    }
}
