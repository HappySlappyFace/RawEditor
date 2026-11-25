//! Camera profiles (`core::camera_profiles`): the per-camera default, and
//! getting more — RawTherapee's open-source profiles, imported files,
//! updates. (The per-image choice is an edit; the file picker is the app's.)
//!
//! A profile is applied when an image's GPU resources are built, so any
//! change that could alter which profile the open image resolves to —
//! choice, default, or the installed set — rebuilds them
//! ([`Session::camera_profile_stale`]). From the cached RAW when there is
//! one: no new decode.

use super::{blocking, Effects, Session, SessionEvent};
use crate::core::camera_profiles::{self as profiles, Catalog};

/// A camera-profile job in progress (`session::camera_profiles`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileJob {
    Downloading,
    Importing,
    Updating,
    Rescanning,
}

/// Camera-profile downloads, imports and updates.
#[derive(Debug, Default)]
pub struct CameraProfileState {
    pub busy: Option<ProfileJob>,
    /// The last job's outcome, for the panels to report.
    pub notice: Option<Result<String, String>>,
    /// A camera RawTherapee was found to have no profile for — the Develop
    /// prompt says so instead of offering the download again.
    pub unavailable: Option<String>,
}

impl Session {
    /// The open image's camera, as `(make, model)`.
    pub fn camera(&self) -> Option<(String, String)> {
        self.current_metadata.as_ref().map(|m| (m.make.clone(), m.model.clone()))
    }

    /// Were the open image's GPU resources built with a different profile
    /// choice than it now has (its own, or its camera's default)?
    pub fn camera_profile_stale(&self) -> bool {
        let (Some(resources), Some((make, model))) = (&self.image_resources, self.camera()) else {
            return false;
        };
        let Some(used) = &resources.profile_use else { return false };
        used.is_stale(self.current_edit_params.camera_profile, &self.camera_profile_defaults, &make, &model)
    }

    /// Preferences' per-camera default; `None` clears it back to automatic.
    /// The caller saves the setting.
    pub fn set_camera_profile_default(&mut self, camera: String, value: Option<String>) -> Effects {
        match value {
            Some(v) => self.camera_profile_defaults.insert(camera, v),
            None => self.camera_profile_defaults.remove(&camera),
        };
        if self.camera_profile_stale() {
            self.reload_selected_image()
        } else {
            Effects::none()
        }
    }

    /// The installed set changed: the open image may now resolve differently.
    fn camera_profiles_changed(&mut self) -> Effects {
        if self.image_resources.is_some() {
            self.reload_selected_image()
        } else {
            Effects::none()
        }
    }

    fn start_profile_job(&mut self, job: ProfileJob) -> bool {
        if self.camera_profiles.busy.is_some() {
            return false;
        }
        self.camera_profiles.busy = Some(job);
        self.camera_profiles.notice = None;
        true
    }

    /// RawTherapee's profile for the open image's camera.
    pub fn download_camera_profile(&mut self) -> Effects {
        let Some((make, model)) = self.camera() else { return Effects::none() };
        if !self.start_profile_job(ProfileJob::Downloading) {
            return Effects::none();
        }
        let camera_name = profiles::camera_name(&make, &model);
        blocking(
            "camera profile download",
            move || {
                let result = profiles::download_for_camera(&make, &model);
                if matches!(result, Ok(Some(_))) {
                    Catalog::rescan();
                }
                result
            },
            move |r| SessionEvent::CameraProfileDownloaded(camera_name, r.and_then(|inner| inner)),
        )
    }

    pub fn camera_profile_downloaded(&mut self, camera_name: String, result: Result<Option<String>, String>) -> Effects {
        self.camera_profiles.busy = None;
        match result {
            Ok(Some(file)) => {
                self.status = format!("Camera profile downloaded: {file}");
                self.camera_profiles.notice = Some(Ok(format!("Downloaded {file}")));
                self.camera_profiles.unavailable = None;
                self.camera_profiles_changed()
            }
            Ok(None) => {
                let name = camera_name;
                self.camera_profiles.notice = Some(Err(format!("RawTherapee has no profile for {name}")));
                self.camera_profiles.unavailable = Some(name);
                Effects::none()
            }
            Err(e) => {
                tracing::error!("Camera profile download failed: {e}");
                self.camera_profiles.notice = Some(Err(format!("Download failed: {e}")));
                Effects::none()
            }
        }
    }

    /// Import the picked `.dcp` files.
    pub fn import_camera_profiles(&mut self, files: Vec<std::path::PathBuf>) -> Effects {
        if files.is_empty() || !self.start_profile_job(ProfileJob::Importing) {
            return Effects::none();
        }
        blocking(
            "camera profile import",
            move || {
                let outcome = profiles::import(&files);
                if !outcome.0.is_empty() {
                    Catalog::rescan();
                }
                outcome
            },
            |r| match r {
                Ok((ok, failed)) => SessionEvent::CameraProfilesImported(ok, failed),
                Err(e) => SessionEvent::CameraProfilesImported(Vec::new(), vec![e]),
            },
        )
    }

    pub fn camera_profiles_imported(&mut self, ok: Vec<String>, failed: Vec<String>) -> Effects {
        self.camera_profiles.busy = None;
        self.camera_profiles.notice = Some(match (ok.len(), failed.is_empty()) {
            (_, false) => Err(format!(
                "Imported {} — could not import: {}",
                ok.len(),
                failed.join("; ")
            )),
            (1, true) => Ok(format!("Imported {}", ok[0])),
            (n, true) => Ok(format!("Imported {n} profiles")),
        });
        if ok.is_empty() {
            Effects::none()
        } else {
            self.camera_profiles_changed()
        }
    }

    /// Check RawTherapee for newer versions of the downloaded profiles.
    pub fn update_camera_profiles(&mut self) -> Effects {
        if !self.start_profile_job(ProfileJob::Updating) {
            return Effects::none();
        }
        blocking(
            "camera profile update",
            || {
                let report = profiles::update_downloaded();
                if report.as_ref().is_ok_and(|r| !r.updated.is_empty()) {
                    Catalog::rescan();
                }
                report
            },
            |r| SessionEvent::CameraProfilesUpdated(r.and_then(|inner| inner)),
        )
    }

    pub fn camera_profiles_updated(&mut self, result: Result<profiles::UpdateReport, String>) -> Effects {
        self.camera_profiles.busy = None;
        let changed = result.as_ref().is_ok_and(|r| !r.updated.is_empty());
        self.camera_profiles.notice = Some(match result {
            Ok(r) if r.checked == 0 => Ok("No downloaded profiles to update".to_string()),
            Ok(r) if r.updated.is_empty() => Ok(format!(
                "All {} downloaded profile{} up to date",
                r.checked,
                if r.checked == 1 { " is" } else { "s are" }
            )),
            Ok(r) => Ok(format!("Updated {}", r.updated.join(", "))),
            Err(e) => Err(format!("Could not check for updates: {e}")),
        });
        if changed {
            self.camera_profiles_changed()
        } else {
            Effects::none()
        }
    }

    pub fn rescan_camera_profiles(&mut self) -> Effects {
        if !self.start_profile_job(ProfileJob::Rescanning) {
            return Effects::none();
        }
        blocking("camera profile rescan", Catalog::rescan, |_| SessionEvent::CameraProfilesScanned(true))
    }

    /// Scan the folders for display if nothing has needed them yet (the
    /// Camera Profiles page asks on opening).
    pub fn camera_profiles_ensure_loaded() -> Effects {
        if Catalog::loaded().is_some() {
            return Effects::none();
        }
        blocking(
            "camera profile scan",
            || {
                Catalog::global();
            },
            |_| SessionEvent::CameraProfilesScanned(false),
        )
    }

    pub fn camera_profiles_scanned(&mut self, changed: bool) -> Effects {
        if self.camera_profiles.busy == Some(ProfileJob::Rescanning) {
            self.camera_profiles.busy = None;
        }
        if changed {
            self.camera_profiles_changed()
        } else {
            Effects::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing open: changing what is installed or a default rebuilds nothing.
    #[test]
    fn with_no_image_open_nothing_is_rebuilt() {
        let mut s = Session::default();
        assert!(s.set_camera_profile_default("NIKON D3300".into(), Some("builtin".into())).is_empty());
        s.camera_profiles.busy = Some(ProfileJob::Importing);
        assert!(s.camera_profiles_imported(vec!["a.dcp".into()], vec![]).is_empty());
        assert_eq!(s.camera_profiles.busy, None);
        assert!(s.download_camera_profile().is_empty(), "no open image, no camera to download for");
    }

    #[test]
    fn an_empty_pick_starts_nothing() {
        let mut s = Session::default();
        assert!(s.import_camera_profiles(Vec::new()).is_empty());
        assert_eq!(s.camera_profiles.busy, None);
    }

    use crate::session::testkit::{deliver, labels, open_in_develop, raw, session_with_images};

    fn notice(s: &Session) -> Result<&str, &str> {
        match s.camera_profiles.notice.as_ref().expect("a notice") {
            Ok(m) => Ok(m.as_str()),
            Err(m) => Err(m.as_str()),
        }
    }

    /// One job at a time: a second request while one runs starts nothing and
    /// keeps the first one's state.
    #[test]
    fn one_job_at_a_time() {
        let mut s = Session::default();
        assert_eq!(labels(&s.update_camera_profiles()), ["camera profile update"]);
        assert_eq!(s.camera_profiles.busy, Some(ProfileJob::Updating));
        assert!(s.rescan_camera_profiles().is_empty());
        assert!(s.import_camera_profiles(vec!["a.dcp".into()]).is_empty());
        assert_eq!(s.camera_profiles.busy, Some(ProfileJob::Updating));

        // A scan that isn't the rescan job must not end the running job.
        let _ = s.camera_profiles_scanned(false);
        assert_eq!(s.camera_profiles.busy, Some(ProfileJob::Updating));
    }

    #[test]
    fn a_download_reports_each_outcome_and_remembers_a_missing_camera() {
        let mut s = Session::default();
        s.camera_profiles.busy = Some(ProfileJob::Downloading);
        let _ = s.camera_profile_downloaded("Testkit X".into(), Ok(None));
        assert_eq!(s.camera_profiles.busy, None);
        assert_eq!(notice(&s), Err("RawTherapee has no profile for Testkit X"));
        assert_eq!(s.camera_profiles.unavailable.as_deref(), Some("Testkit X"));

        let _ = s.camera_profile_downloaded("Testkit X".into(), Err("offline".into()));
        assert_eq!(notice(&s), Err("Download failed: offline"));

        let _ = s.camera_profile_downloaded("Testkit X".into(), Ok(Some("Testkit X.dcp".into())));
        assert_eq!(notice(&s), Ok("Downloaded Testkit X.dcp"));
        assert_eq!(s.camera_profiles.unavailable, None, "the prompt still says there is none");
    }

    /// A new profile can change what the open image resolves to: it is
    /// rebuilt, from its cached RAW.
    #[test]
    fn a_downloaded_profile_rebuilds_the_open_image() {
        let mut s = session_with_images(1);
        let _ = open_in_develop(&mut s, 1);
        let _ = deliver(&mut s, SessionEvent::RawDataLoaded(1, Ok(raw(1))));
        assert_eq!(s.camera(), Some(("Testkit".into(), "synthetic 1".into())));
        assert_eq!(labels(&s.download_camera_profile()), ["camera profile download"]);
        // Without GPU resources there is nothing built to rebuild.
        assert!(s.camera_profile_downloaded("x".into(), Ok(Some("x.dcp".into()))).is_empty());
    }

    #[test]
    fn imports_report_what_went_in_and_what_did_not() {
        let mut s = Session::default();
        let _ = s.camera_profiles_imported(vec!["a.dcp".into()], vec![]);
        assert_eq!(notice(&s), Ok("Imported a.dcp"));
        let _ = s.camera_profiles_imported(vec!["a.dcp".into(), "b.dcp".into()], vec![]);
        assert_eq!(notice(&s), Ok("Imported 2 profiles"));
        let _ = s.camera_profiles_imported(vec!["a.dcp".into()], vec!["b.txt: not a DCP".into()]);
        assert_eq!(notice(&s), Err("Imported 1 — could not import: b.txt: not a DCP"));
    }

    #[test]
    fn update_reports_read_naturally() {
        let report = |checked: usize, updated: &[&str]| {
            Ok(profiles::UpdateReport { checked, updated: updated.iter().map(|s| s.to_string()).collect(), ..Default::default() })
        };
        let mut s = Session::default();
        let _ = s.camera_profiles_updated(report(0, &[]));
        assert_eq!(notice(&s), Ok("No downloaded profiles to update"));
        let _ = s.camera_profiles_updated(report(1, &[]));
        assert_eq!(notice(&s), Ok("All 1 downloaded profile is up to date"));
        let _ = s.camera_profiles_updated(report(3, &[]));
        assert_eq!(notice(&s), Ok("All 3 downloaded profiles are up to date"));
        let _ = s.camera_profiles_updated(report(3, &["A.dcp", "B.dcp"]));
        assert_eq!(notice(&s), Ok("Updated A.dcp, B.dcp"));
        let _ = s.camera_profiles_updated(Err("rate limited".into()));
        assert_eq!(notice(&s), Err("Could not check for updates: rate limited"));
        assert_eq!(s.camera_profiles.busy, None);
    }
}
