//! Lens-profile database: download, update check and rescan
//! (`core::lensfun`), from Preferences › Lens Profiles and from the Lens
//! Corrections panel when no database is installed.
//!
//! An image's lens profile is looked up when its RAW is read, so whenever
//! the database changes the selected image is decoded again and the raw
//! cache is dropped — otherwise the panel would keep saying "no profile"
//! until the app restarted.

use super::{blocking, Effects, FollowUp, Session, SessionEvent};
use crate::core::lens::LensDb;

/// A lens-database job in progress (`session::lensfun`). One at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensfunJob {
    /// Asking lensfun what it currently publishes.
    Checking,
    Downloading,
    /// Re-reading the lens folders (files added by hand).
    Rescanning,
}

/// Lens-profile database downloads and checks, for Preferences › Lens
/// Profiles and the Lens Corrections panel.
#[derive(Debug, Default)]
pub struct LensfunState {
    pub busy: Option<LensfunJob>,
    /// The last check's answer.
    pub remote: Option<Result<crate::core::lensfun::Remote, String>>,
    /// The last download's outcome.
    pub installed: Option<Result<crate::core::lensfun::Installed, String>>,
}

impl Session {
    pub fn lensfun_check(&mut self) -> Effects {
        if self.lensfun.busy.is_some() {
            return Effects::none();
        }
        self.lensfun.busy = Some(LensfunJob::Checking);
        blocking("lensfun check", crate::core::lensfun::check, |r| {
            SessionEvent::LensfunChecked(r.and_then(|inner| inner))
        })
    }

    pub fn lensfun_checked(&mut self, result: Result<crate::core::lensfun::Remote, String>) {
        self.lensfun.busy = None;
        self.lensfun.remote = Some(result);
    }

    pub fn lensfun_download(&mut self) -> Effects {
        if self.lensfun.busy.is_some() {
            return Effects::none();
        }
        self.lensfun.busy = Some(LensfunJob::Downloading);
        self.lensfun.installed = None;
        blocking(
            "lensfun download",
            || {
                let installed = crate::core::lensfun::download_latest()?;
                // Swap the new database in here, off the UI thread.
                LensDb::reload();
                Ok(installed)
            },
            |r| SessionEvent::LensfunInstalled(r.and_then(|inner| inner)),
        )
    }

    pub fn lensfun_installed(&mut self, result: Result<crate::core::lensfun::Installed, String>) -> FollowUp {
        self.lensfun.busy = None;
        let ok = result.is_ok();
        match &result {
            Ok(i) => {
                self.status = format!(
                    "Lens profiles installed: {} lenses, published {}",
                    i.lenses,
                    crate::core::lensfun::format_date(i.timestamp)
                );
                // What was checked is now what is installed.
                self.lensfun.remote = None;
            }
            Err(e) => {
                tracing::error!("Lens profile download failed: {e}");
                self.status = format!("Lens profile download failed: {e}");
            }
        }
        self.lensfun.installed = Some(result);
        if ok {
            self.lens_database_changed()
        } else {
            FollowUp::Nothing
        }
    }

    pub fn lensfun_rescan(&mut self) -> Effects {
        if self.lensfun.busy.is_some() {
            return Effects::none();
        }
        self.lensfun.busy = Some(LensfunJob::Rescanning);
        blocking("lensfun rescan", LensDb::reload, |_| SessionEvent::LensDbLoaded(true))
    }

    /// Load the database for display if nothing has needed it yet (it is
    /// read lazily, by the first RAW decode). The Lens Profiles page asks on
    /// opening.
    pub fn lens_db_ensure_loaded() -> Effects {
        if LensDb::loaded().is_some() {
            return Effects::none();
        }
        blocking(
            "lens database load",
            || {
                LensDb::global();
            },
            |_| SessionEvent::LensDbLoaded(false),
        )
    }

    pub fn lens_db_loaded(&mut self, changed: bool) -> FollowUp {
        if self.lensfun.busy == Some(LensfunJob::Rescanning) {
            self.lensfun.busy = None;
        }
        if changed {
            self.lens_database_changed()
        } else {
            FollowUp::Nothing
        }
    }

    /// The database changed: decoded images carry the old lookup.
    fn lens_database_changed(&mut self) -> FollowUp {
        self.raw_cache.clear();
        self.current_raw = None;
        self.raw_cache_bytes = 0;
        FollowUp::ReloadSelectedImage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new database drops every decode (they carry the old lookup) and
    /// asks for the open image again; a failed one changes nothing.
    #[test]
    fn only_a_changed_database_drops_the_decodes() {
        let mut s = Session::default();
        s.raw_cache_bytes = 123;
        assert_eq!(s.lensfun_installed(Err("offline".into())), FollowUp::Nothing);
        assert_eq!(s.raw_cache_bytes, 123);
        assert_eq!(s.lens_db_loaded(false), FollowUp::Nothing, "a first read changes nothing");
        assert_eq!(s.lens_db_loaded(true), FollowUp::ReloadSelectedImage);
        assert_eq!(s.raw_cache_bytes, 0);
    }

    #[test]
    fn a_running_job_blocks_the_next() {
        let mut s = Session::default();
        assert_eq!(s.lensfun_check().labels().collect::<Vec<_>>(), ["lensfun check"]);
        assert!(s.lensfun_download().is_empty());
        assert!(s.lensfun_rescan().is_empty());
    }
}
