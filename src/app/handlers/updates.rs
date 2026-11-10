//! Update checks (`core::updates`): the first-run prompt, the startup check,
//! Check for Updates, and the title-bar badge.
//!
//! The app makes no request until the user answers the first-run prompt.
//! After that it checks once per launch only if they said yes. An automatic
//! check is silent: a newer version lights the badge, and a failure is only
//! logged.

use iced::Task;

use crate::app::message::Message;
use crate::app::state::{Modal, RawEditor};
use crate::core::updates::Available;

#[derive(Debug, Clone)]
pub struct UpdatesState {
    pub checking: bool,
    /// A newer release with a download for this platform, from any check
    /// this session. It drives the title-bar badge.
    pub available: Option<Available>,
    /// The last manual check's outcome, which `Modal::Update` and
    /// Preferences show: `Ok(None)` means up to date.
    pub shown: Option<Result<Option<Available>, String>>,
    /// The first-run prompt's checkbox, which starts ticked.
    pub prompt_choice: bool,
}

impl Default for UpdatesState {
    fn default() -> Self {
        Self { checking: false, available: None, shown: None, prompt_choice: true }
    }
}

/// Once the catalogue is open: ask the first-run question, or run the
/// startup check if the user agreed to it.
pub fn on_startup(editor: &mut RawEditor) -> Task<Message> {
    if crate::core::updates::packaged().is_some() {
        return Task::none();
    }
    match editor.update_check {
        None => {
            if editor.active_modal == Modal::None {
                editor.active_modal = Modal::UpdatePrompt;
            }
            Task::none()
        }
        Some(true) => start(editor, false),
        Some(false) => Task::none(),
    }
}

fn start(editor: &mut RawEditor, manual: bool) -> Task<Message> {
    if editor.updates.checking || crate::core::updates::packaged().is_some() {
        return Task::none();
    }
    editor.updates.checking = true;
    Task::perform(
        async {
            tokio::task::spawn_blocking(crate::core::updates::check)
                .await
                .unwrap_or_else(|e| Err(format!("update check failed: {e}")))
        },
        move |result| Message::UpdateChecked { result, manual },
    )
}

/// Help › Check for Updates opens the result dialog. Preferences' Check Now
/// keeps Preferences open and shows the result there.
pub fn handle_check(editor: &mut RawEditor) -> Task<Message> {
    editor.updates.shown = None;
    if editor.active_modal != Modal::Preferences {
        editor.active_modal = Modal::Update;
    }
    start(editor, true)
}

pub fn handle_checked(
    editor: &mut RawEditor,
    result: Result<Option<Available>, String>,
    manual: bool,
) -> Task<Message> {
    editor.updates.checking = false;
    match &result {
        Ok(Some(a)) => {
            tracing::info!("update: {} is available (this is {})", a.version, crate::core::updates::current_version());
            editor.updates.available = Some(a.clone());
        }
        Ok(None) => {
            tracing::info!("update: {} is the newest release", crate::core::updates::current_version());
            editor.updates.available = None;
        }
        Err(e) => tracing::warn!("update check: {e}"),
    }
    if manual {
        editor.updates.shown = Some(result);
    }
    Task::none()
}

pub fn handle_prompt_toggled(editor: &mut RawEditor, on: bool) -> Task<Message> {
    editor.updates.prompt_choice = on;
    Task::none()
}

/// Save the first-run answer and, if it is yes, check straight away: this is
/// the startup the user just agreed to.
pub fn handle_prompt_confirmed(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::UpdatePrompt {
        return Task::none();
    }
    let yes = editor.updates.prompt_choice;
    editor.update_check = Some(yes);
    editor.save_preferences();
    editor.active_modal = Modal::None;
    if yes {
        start(editor, false)
    } else {
        Task::none()
    }
}

pub fn handle_toggle(editor: &mut RawEditor) -> Task<Message> {
    editor.update_check = Some(!editor.update_check.unwrap_or(false));
    editor.save_preferences();
    Task::none()
}

pub fn handle_open_page(editor: &mut RawEditor) -> Task<Message> {
    let url = editor
        .updates
        .available
        .as_ref()
        .map(|a| a.url.clone())
        .unwrap_or_else(|| crate::core::updates::RELEASES_URL.to_string());
    crate::app::handlers::window::open_url(&url);
    editor.status = format!("Opened {url}");
    editor.active_modal = Modal::None;
    Task::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> RawEditor {
        let (mut e, _) = RawEditor::new();
        e.update_check = None;
        e.active_modal = Modal::None;
        e
    }

    fn available() -> Available {
        Available { version: "9.9.9".into(), url: "https://github.com/HappySlappyFace/RawEditor/releases/tag/v9.9.9".into(), published: String::new() }
    }

    #[test]
    fn first_startup_asks_and_makes_no_request() {
        let mut e = editor();
        let _ = on_startup(&mut e);
        assert_eq!(e.active_modal, Modal::UpdatePrompt);
        assert!(!e.updates.checking, "no request before the user answers");
        assert!(e.updates.prompt_choice, "the prompt starts at yes");
    }

    #[test]
    fn declining_never_checks() {
        let mut e = editor();
        e.update_check = Some(false);
        let _ = on_startup(&mut e);
        assert_eq!(e.active_modal, Modal::None);
        assert!(!e.updates.checking);
    }

    #[test]
    fn an_automatic_result_lights_the_badge_without_a_dialog() {
        let mut e = editor();
        let _ = handle_checked(&mut e, Ok(Some(available())), false);
        assert_eq!(e.updates.available, Some(available()));
        assert_eq!(e.active_modal, Modal::None);
        assert!(e.updates.shown.is_none());
        // A later up-to-date answer clears it.
        let _ = handle_checked(&mut e, Ok(None), false);
        assert!(e.updates.available.is_none());
    }

    #[test]
    fn a_manual_check_shows_its_result_and_keeps_preferences_open() {
        let mut e = editor();
        let _ = handle_check(&mut e);
        assert_eq!(e.active_modal, Modal::Update);
        let _ = handle_checked(&mut e, Err("offline".into()), true);
        assert_eq!(e.updates.shown, Some(Err("offline".into())));
        assert!(!e.updates.checking);

        e.active_modal = Modal::Preferences;
        let _ = handle_check(&mut e);
        assert_eq!(e.active_modal, Modal::Preferences);
        assert!(e.updates.shown.is_none(), "a new check clears the old result");
    }

    #[test]
    fn answering_the_prompt_saves_the_choice() {
        let mut e = editor();
        let _ = on_startup(&mut e);
        let _ = handle_prompt_toggled(&mut e, false);
        let _ = handle_prompt_confirmed(&mut e);
        assert_eq!(e.update_check, Some(false));
        assert_eq!(e.active_modal, Modal::None);
        assert!(!e.updates.checking);
    }
}
