//! Update checks (`core::updates`): the startup check, Check for Updates,
//! and what the title-bar badge shows.
//!
//! No request is made until the user answers the first-run prompt. After
//! that the app checks once per launch only if they said yes. An automatic
//! check is silent: a newer version lights the badge, and a failure is only
//! logged.

use super::{Effects, Session, SessionEvent};
use crate::core::updates::Available;

#[derive(Debug, Clone, Default)]
pub struct UpdatesState {
    pub checking: bool,
    /// A newer release with a download for this platform, from any check
    /// this session. It drives the title-bar badge.
    pub available: Option<Available>,
    /// The last manual check's outcome, which the update dialog and
    /// Preferences show: `Ok(None)` means up to date.
    pub shown: Option<Result<Option<Available>, String>>,
    /// Check for Updates was clicked while another check was running: that
    /// check's result is then shown too, whatever it was started as.
    pub manual_pending: bool,
}

/// What startup should do about updates.
#[derive(Debug)]
pub enum UpdatesAtStartup {
    /// The first-run question has not been answered: ask it.
    Ask,
    /// Check now.
    Check(Effects),
    /// Declined, or a packaged install (its package manager updates it).
    Nothing,
}

impl Session {
    pub fn updates_at_startup(&mut self) -> UpdatesAtStartup {
        if crate::core::updates::packaged().is_some() {
            return UpdatesAtStartup::Nothing;
        }
        match self.update_check {
            None => UpdatesAtStartup::Ask,
            Some(true) => UpdatesAtStartup::Check(self.start_update_check(false)),
            Some(false) => UpdatesAtStartup::Nothing,
        }
    }

    fn start_update_check(&mut self, manual: bool) -> Effects {
        if self.updates.checking || crate::core::updates::packaged().is_some() {
            return Effects::none();
        }
        self.updates.checking = true;
        Effects::perform("update check", async move {
            let result = tokio::task::spawn_blocking(crate::core::updates::check)
                .await
                .unwrap_or_else(|e| Err(format!("update check failed: {e}")));
            SessionEvent::UpdateChecked { result, manual }
        })
    }

    /// Check for Updates: check, and show the result.
    pub fn request_update_check(&mut self) -> Effects {
        self.updates.shown = None;
        // Starting does nothing while a check runs (the startup one, say):
        // remember the click so that check's result is shown.
        self.updates.manual_pending = self.updates.checking;
        self.start_update_check(true)
    }

    pub fn update_checked(&mut self, result: Result<Option<Available>, String>, manual: bool) {
        self.updates.checking = false;
        let manual = manual || std::mem::take(&mut self.updates.manual_pending);
        match &result {
            Ok(Some(a)) => {
                tracing::info!("update: {} is available (this is {})", a.version, crate::core::updates::current_version());
                self.updates.available = Some(a.clone());
            }
            Ok(None) => {
                tracing::info!("update: {} is the newest release", crate::core::updates::current_version());
                self.updates.available = None;
            }
            Err(e) => tracing::warn!("update check: {e}"),
        }
        if manual {
            self.updates.shown = Some(result);
        }
    }

    /// The first-run answer. A yes checks straight away: this is the startup
    /// the user just agreed to. The caller saves the setting.
    pub fn answer_update_prompt(&mut self, yes: bool) -> Effects {
        self.update_check = Some(yes);
        if yes {
            self.start_update_check(false)
        } else {
            Effects::none()
        }
    }

    /// Preferences' checkbox. The caller saves the setting.
    pub fn toggle_update_check(&mut self) {
        self.update_check = Some(!self.update_check.unwrap_or(false));
    }

    /// The release page to open: the newer release's, or the list.
    pub fn update_page_url(&self) -> String {
        self.updates
            .available
            .as_ref()
            .map(|a| a.url.clone())
            .unwrap_or_else(|| crate::core::updates::RELEASES_URL.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unanswered_prompt_asks_and_asks_for_no_work() {
        let mut s = Session::default();
        assert!(matches!(s.updates_at_startup(), UpdatesAtStartup::Ask));
        assert!(!s.updates.checking);
    }

    #[test]
    fn a_check_is_one_job_and_a_second_while_it_runs_adds_none() {
        let mut s = Session { update_check: Some(true), ..Default::default() };
        let UpdatesAtStartup::Check(fx) = s.updates_at_startup() else { panic!("expected a check") };
        assert_eq!(fx.labels().collect::<Vec<_>>(), ["update check"]);
        assert!(s.updates.checking);
        assert!(s.request_update_check().is_empty(), "one check at a time");
        assert!(s.updates.manual_pending, "the running check's result will be shown");
    }

    #[test]
    fn answering_no_asks_for_no_work() {
        let mut s = Session::default();
        assert!(s.answer_update_prompt(false).is_empty());
        assert_eq!(s.update_check, Some(false));
        assert!(matches!(s.updates_at_startup(), UpdatesAtStartup::Nothing));
    }
}
