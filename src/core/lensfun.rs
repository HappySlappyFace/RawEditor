//! Downloading and updating the lensfun database (Preferences › Lens
//! Profiles, and the Lens Corrections panel when none is installed).
//!
//! lensfun publishes its database the way its own `lensfun-update-data`
//! reads it: `versions.json` — `[timestamp, [format versions], [mirror base
//! URLs]]` — next to one `version_<n>.tar.bz2` per format, a flat archive of
//! XML files. The app fetches the newest format it reads, checks that it
//! parses, and swaps it into `paths::lensfun_updates_dir` in one rename, so a
//! failed or partial download never replaces a working database.
//!
//! Only the fixed HTTPS locations below are contacted, and only when the user
//! asks; the mirror list in `versions.json` is not followed. Everything here
//! blocks: run it inside `spawn_blocking`.

use std::io::Read;
use std::path::{Component, Path};
use std::time::Duration;

/// Where lensfun publishes the database, in the order its updater tries them.
pub const LOCATIONS: [&str; 2] = ["https://lensfun.github.io/db/", "https://wilson.bronger.org/lensfun-db/"];

/// Database formats `core::lens` reads, best first. Version 2 is current
/// lensfun; version 1 carries the same calibrations in the older layout.
const FORMATS: [u32; 2] = [2, 1];

/// The archive is ~0.4 MB and unpacks to ~10 MB: generous ceilings that
/// still stop a misbehaving server from filling memory or disk.
const MAX_ARCHIVE: u64 = 32 << 20;
const MAX_FILE: u64 = 16 << 20;
const MAX_FILES: usize = 2000;
/// Total unpacked size: the real database is a few MB.
const MAX_TOTAL: u64 = 64 << 20;

/// What a location currently offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Remote {
    /// Base URL it came from (ends with `/`).
    pub location: String,
    /// When this database was published, seconds since the epoch.
    pub timestamp: i64,
    /// The format this app will download from it.
    pub format: u32,
}

/// The result of an install.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub timestamp: i64,
    pub files: usize,
    pub lenses: usize,
    pub cameras: usize,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .https_only(true)
        .user_agent(format!("RawEditor/{} (lens profiles)", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Parse `versions.json`: `[timestamp, [versions…], [alternatives…]]`.
/// Returns the timestamp and the best format this app reads, if any.
fn parse_versions(json: &str) -> Result<(i64, u32), String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("unreadable versions.json: {e}"))?;
    let list = value.as_array().ok_or("versions.json is not a list")?;
    let timestamp = list.first().and_then(|t| t.as_i64()).ok_or("versions.json has no timestamp")?;
    let offered: Vec<u64> = list
        .get(1)
        .and_then(|v| v.as_array())
        .map(|v| v.iter().filter_map(|n| n.as_u64()).collect())
        .unwrap_or_default();
    let format = FORMATS
        .into_iter()
        .find(|f| offered.contains(&(*f as u64)))
        .ok_or_else(|| format!("no database format this app reads (offered: {offered:?})"))?;
    Ok((timestamp, format))
}

/// Ask the locations in order and take the first that answers, as lensfun's
/// own updater does; a dead second mirror then costs nothing. Blocking.
pub fn check() -> Result<Remote, String> {
    let agent = agent();
    let mut errors = Vec::new();
    let mut best: Option<Remote> = None;
    for location in LOCATIONS {
        let fetched = agent
            .get(&format!("{location}versions.json"))
            .call()
            .map_err(|e| e.to_string())
            .and_then(|mut r| r.body_mut().read_to_string().map_err(|e| e.to_string()))
            .and_then(|json| parse_versions(&json));
        match fetched {
            Ok((timestamp, format)) => {
                best = Some(Remote { location: location.to_string(), timestamp, format });
                break;
            }
            Err(e) => {
                tracing::warn!("lensfun: {location}: {e}");
                errors.push(e);
            }
        }
    }
    best.ok_or_else(|| format!("could not reach the lensfun database ({})", errors.join("; ")))
}

/// A tar member's plain file name, if it is a top-level `*.xml` — never a
/// path. Anything nested, absolute or with `..` is refused, so an archive
/// cannot write outside the folder it is unpacked into.
fn xml_name(path: &Path) -> Option<String> {
    let mut parts = path.components().filter(|c| !matches!(c, Component::CurDir));
    let (Some(Component::Normal(name)), None) = (parts.next(), parts.next()) else {
        return None;
    };
    let name = name.to_str()?;
    name.to_ascii_lowercase().ends_with(".xml").then(|| name.to_string())
}

/// Unpack a `version_N.tar.bz2` into `(file name, XML)` pairs.
fn unpack(archive: &[u8]) -> Result<Vec<(String, String)>, String> {
    let mut tar = tar::Archive::new(bzip2_rs::DecoderReader::new(archive));
    let mut files = Vec::new();
    let mut total = 0u64;
    for entry in tar.entries().map_err(|e| format!("corrupt archive: {e}"))? {
        let entry = entry.map_err(|e| format!("corrupt archive: {e}"))?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let Some(name) = entry.path().ok().and_then(|p| xml_name(&p)) else { continue };
        if entry.size() > MAX_FILE {
            return Err(format!("{name} is implausibly large"));
        }
        let mut xml = String::new();
        entry
            .take(MAX_FILE)
            .read_to_string(&mut xml)
            .map_err(|e| format!("{name}: {e}"))?;
        total += xml.len() as u64;
        if total > MAX_TOTAL {
            return Err("archive unpacks to an implausible size".to_string());
        }
        files.push((name, xml));
        if files.len() > MAX_FILES {
            return Err("archive holds implausibly many files".to_string());
        }
    }
    Ok(files)
}

/// Finish an update that died between `swap_in`'s two renames: when the live
/// folder is missing but the previous one was set aside, put it back. Called
/// before the database is read.
pub fn restore_interrupted(target: &Path) {
    let Some(parent) = target.parent() else { return };
    let old = parent.join(".updates-old");
    if !target.exists() && old.exists() {
        match std::fs::rename(&old, target) {
            Ok(()) => tracing::info!("lensfun: restored the database from an interrupted update"),
            Err(e) => tracing::warn!("lensfun: could not restore {}: {e}", old.display()),
        }
    }
}

/// Write `files` + `timestamp.txt` as the new contents of `target`,
/// replacing what was there only once everything is on disk.
fn swap_in(target: &Path, files: &[(String, String)], timestamp: i64) -> Result<(), String> {
    let parent = target.parent().ok_or("lensfun folder has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let staging = parent.join(".updates-new");
    let old = parent.join(".updates-old");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let write = || -> std::io::Result<()> {
        for (name, xml) in files {
            std::fs::write(staging.join(name), xml)?;
        }
        std::fs::write(staging.join("timestamp.txt"), timestamp.to_string())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("could not write the database: {e}"));
    }
    let _ = std::fs::remove_dir_all(&old);
    let had_previous = target.exists();
    if had_previous {
        std::fs::rename(target, &old).map_err(|e| format!("could not replace the old database: {e}"))?;
    }
    if let Err(e) = std::fs::rename(&staging, target) {
        if had_previous {
            let _ = std::fs::rename(&old, target);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("could not install the database: {e}"));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Download `remote`'s database, check it, install it into `target` and
/// report what it holds. Blocking.
pub fn install(remote: &Remote, target: &Path) -> Result<Installed, String> {
    let url = format!("{}version_{}.tar.bz2", remote.location, remote.format);
    let archive = agent()
        .get(&url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?
        .body_mut()
        .with_config()
        .limit(MAX_ARCHIVE)
        .read_to_vec()
        .map_err(|e| format!("download failed: {e}"))?;
    let files = unpack(&archive)?;

    // A database that doesn't parse must not replace one that does.
    if files.iter().all(|(_, xml)| crate::core::lens::parse(xml).0.is_empty()) {
        return Err("the downloaded database holds no lens profiles".to_string());
    }

    swap_in(target, &files, remote.timestamp)?;
    // Counted as the app will use it: a lens listed in several files (one
    // per mount) is one lens.
    let db = crate::core::lens::LensDb::load(&[target.to_path_buf()]);
    tracing::info!("lensfun: installed {} files ({} lenses) from {url}", files.len(), db.lenses.len());
    Ok(Installed {
        timestamp: remote.timestamp,
        files: files.len(),
        lenses: db.lenses.len(),
        cameras: db.cameras.len(),
    })
}

/// The newest database [`check`] finds, installed into the app's own
/// folder. Blocking.
pub fn download_latest() -> Result<Installed, String> {
    install(&check()?, &crate::core::paths::lensfun_updates_dir())
}

/// A publication timestamp as a date, e.g. "8 Oct 2026".
pub fn format_date(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|d| d.format("%-d %b %Y").to_string())
        .unwrap_or_else(|| "unknown date".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_json_picks_the_newest_format_we_read() {
        assert_eq!(parse_versions("[1791482221, [0, 1, 2], []]"), Ok((1791482221, 2)));
        assert_eq!(parse_versions("[5, [1], [\"https://x/\"]]"), Ok((5, 1)));
        assert!(parse_versions("[5, [3, 4], []]").is_err());
        assert!(parse_versions("{\"oops\": 1}").is_err());
        assert!(parse_versions("not json").is_err());
    }

    #[test]
    fn only_top_level_xml_files_are_unpacked() {
        assert_eq!(xml_name(Path::new("slr-nikon.xml")).as_deref(), Some("slr-nikon.xml"));
        assert_eq!(xml_name(Path::new("./slr-nikon.xml")).as_deref(), Some("slr-nikon.xml"));
        assert_eq!(xml_name(Path::new("../evil.xml")), None);
        assert_eq!(xml_name(Path::new("/etc/evil.xml")), None);
        assert_eq!(xml_name(Path::new("sub/dir.xml")), None);
        assert_eq!(xml_name(Path::new("timestamp.txt")), None);
    }

    /// An archive built the way lensfun's is: flat, XML at the top level,
    /// plus things that must be skipped.
    fn archive(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, body.as_bytes()).unwrap();
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn unpacking_a_bzip2_archive_keeps_only_its_xml() {
        // bzip2 of the tar, made with the system tool (the crate only decodes).
        let tar = archive(&[("a.xml", "<lensdatabase/>"), ("readme.txt", "hi")]);
        let dir = std::env::temp_dir().join(format!("raw-editor-lensfun-unpack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tar_path = dir.join("db.tar");
        std::fs::write(&tar_path, &tar).unwrap();
        let ok = std::process::Command::new("bzip2").arg("-kf").arg(&tar_path).status();
        if !ok.is_ok_and(|s| s.success()) {
            eprintln!("no bzip2 tool, skipping");
            return;
        }
        let compressed = std::fs::read(dir.join("db.tar.bz2")).unwrap();
        let files = unpack(&compressed).unwrap();
        assert_eq!(files, vec![("a.xml".to_string(), "<lensdatabase/>".to_string())]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn swapping_in_replaces_the_old_database_whole() {
        let root = std::env::temp_dir().join(format!("raw-editor-lensfun-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let target = root.join("updates");
        swap_in(&target, &[("old.xml".into(), "1".into())], 1).unwrap();
        swap_in(&target, &[("new.xml".into(), "2".into())], 2).unwrap();
        assert!(!target.join("old.xml").exists(), "a stale file survived the update");
        assert_eq!(std::fs::read_to_string(target.join("new.xml")).unwrap(), "2");
        assert_eq!(std::fs::read_to_string(target.join("timestamp.txt")).unwrap(), "2");
        assert!(!root.join(".updates-new").exists() && !root.join(".updates-old").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_interrupted_swap_is_restored() {
        let root = std::env::temp_dir().join(format!("raw-editor-lensfun-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let target = root.join("updates");
        std::fs::create_dir_all(root.join(".updates-old")).unwrap();
        std::fs::write(root.join(".updates-old").join("a.xml"), "1").unwrap();
        restore_interrupted(&target);
        assert_eq!(std::fs::read_to_string(target.join("a.xml")).unwrap(), "1");
        // A live folder is never replaced by the leftover.
        std::fs::create_dir_all(root.join(".updates-old")).unwrap();
        restore_interrupted(&target);
        assert!(target.join("a.xml").exists() && root.join(".updates-old").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dates_read_naturally() {
        assert_eq!(format_date(1791482221), "8 Oct 2026");
    }

    /// The real thing, end to end: run with
    /// `cargo test lensfun_download -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn lensfun_download_installs_a_database_that_loads() {
        let remote = check().expect("reachable");
        eprintln!("remote: {remote:?} ({})", format_date(remote.timestamp));
        let dir = std::env::temp_dir().join(format!("raw-editor-lensfun-live-{}", std::process::id()));
        let target = dir.join("updates");
        let installed = install(&remote, &target).expect("install");
        eprintln!("installed: {installed:?}");
        let db = crate::core::lens::LensDb::load(std::slice::from_ref(&target));
        assert_eq!(db.lenses.len(), installed.lenses);
        assert_eq!(db.sources[0].timestamp, Some(remote.timestamp));
        // The test camera's lens is in it.
        assert!(db.find("Sigma", "17-50mm F2.8 EX DC OS HSM").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
