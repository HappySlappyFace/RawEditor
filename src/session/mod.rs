//! The editing session: the app's logic, with no UI toolkit in it.
//!
//! Built here, inside the app, area by area, and moved into
//! `crates/raw-editor-session` once nothing in it refers to iced. Its methods
//! change state and return [`Effects`]; the app runs them
//! (`app::update::run`) and hands each result back as a [`SessionEvent`]
//! (`Message::Session`). Decisions that need a dialog come back as values for
//! the app to act on; the session never opens one.

mod updates;

pub use updates::{UpdatesAtStartup, UpdatesState};

/// Work for the host to run; each item resolves to a [`SessionEvent`].
pub type Effects = raw_editor_session::Effects<SessionEvent>;

/// The result of a piece of work the session asked for.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// An update check finished (`manual`: Check for Updates was clicked).
    UpdateChecked { result: Result<Option<crate::core::updates::Available>, String>, manual: bool },
}

#[derive(Debug, Default)]
pub struct Session {
    /// `AppSettings::update_check`: `None` until the first-run prompt is answered.
    pub update_check: Option<bool>,
    /// The update check's progress and result.
    pub updates: UpdatesState,
}
