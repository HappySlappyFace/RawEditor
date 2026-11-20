//! Camera colour profiles (DCP): which ones are installed, which one an image
//! uses, and downloading open-source ones.
//!
//! ## Where they come from
//! - The user's own `.dcp` files in `paths::profiles_dir()` — imported from
//!   Preferences › Camera Profiles, or copied in by hand. Never touched by
//!   the app beyond the import.
//! - Open-source profiles from the RawTherapee project (GPL-3.0-or-later),
//!   downloaded one camera at a time on request into
//!   [`downloaded_dir`]. The app owns that folder.
//!
//! ## Which one an image uses
//! [`ProfileChoice`] lives in the image's `EditParams` (so it undoes, copies
//! and pastes like any edit): `Auto`, `Builtin` (no profile — the matrix
//! pipeline), or a named file. `Auto` takes the per-camera default from the
//! settings, else the best installed match ([`resolve`]).
//!
//! A profile is resolved when an image's GPU resources are built, never per
//! render: its HueSatMap sets a texture's size, so switching profile means
//! rebuilding them (from the cached RAW — no new decode).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::raw::dcp::DcpProfile;

// ───────────────────────────── the choice ──────────────────────────────────

/// Room for a profile's file name. DCP names are short ("Nikon D3300
/// Camera Standard.dcp" is 31 bytes); longer names are refused rather than
/// truncated into something that names a different file.
const NAME_CAPACITY: usize = 120;

/// Which profile an image uses. Fixed-capacity so `EditParams` stays `Copy`
/// (as `BrushData` does); stored in saved edits as a string — `"auto"`,
/// `"builtin"` or the file name, which always ends in `.dcp` and so can't be
/// mistaken for either.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProfileChoice {
    kind: ChoiceKind,
    len: u8,
    name: [u8; NAME_CAPACITY],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChoiceKind {
    Auto,
    Builtin,
    Named,
}

impl ProfileChoice {
    pub const AUTO: Self = Self { kind: ChoiceKind::Auto, len: 0, name: [0; NAME_CAPACITY] };
    pub const BUILTIN: Self = Self { kind: ChoiceKind::Builtin, len: 0, name: [0; NAME_CAPACITY] };

    /// A named profile file; `None` if the name does not fit.
    pub fn named(file_name: &str) -> Option<Self> {
        let bytes = file_name.as_bytes();
        if bytes.is_empty() || bytes.len() > NAME_CAPACITY {
            return None;
        }
        let mut name = [0; NAME_CAPACITY];
        name[..bytes.len()].copy_from_slice(bytes);
        Some(Self { kind: ChoiceKind::Named, len: bytes.len() as u8, name })
    }

    pub fn is_auto(&self) -> bool {
        self.kind == ChoiceKind::Auto
    }

    pub fn is_builtin(&self) -> bool {
        self.kind == ChoiceKind::Builtin
    }

    /// The file name, for a named choice.
    pub fn file_name(&self) -> Option<&str> {
        (self.kind == ChoiceKind::Named)
            .then(|| std::str::from_utf8(&self.name[..self.len as usize]).ok())
            .flatten()
    }

    fn as_text(&self) -> &str {
        match self.kind {
            ChoiceKind::Auto => "auto",
            ChoiceKind::Builtin => "builtin",
            ChoiceKind::Named => self.file_name().unwrap_or("auto"),
        }
    }

    fn from_text(s: &str) -> Self {
        match s {
            "auto" | "" => Self::AUTO,
            "builtin" => Self::BUILTIN,
            name => Self::named(name).unwrap_or(Self::AUTO),
        }
    }
}

impl Default for ProfileChoice {
    fn default() -> Self {
        Self::AUTO
    }
}

impl std::fmt::Debug for ProfileChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProfileChoice({})", self.as_text())
    }
}

impl Serialize for ProfileChoice {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_text())
    }
}

impl<'de> Deserialize<'de> for ProfileChoice {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::from_text(&String::deserialize(d)?))
    }
}

// ──────────────────────────── installed ones ───────────────────────────────

/// Downloaded profiles: owned by the app, refreshed by [`update_downloaded`].
pub fn downloaded_dir() -> PathBuf {
    crate::core::paths::profiles_dir().join("downloaded")
}

const MANIFEST: &str = "manifest.json";
const ALIASES: &str = "camera_model_aliases.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    /// In the user's own profiles folder.
    Own,
    /// Downloaded by the app.
    Downloaded,
}

/// One installed profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileInfo {
    pub file_name: String,
    pub path: PathBuf,
    /// What to call it in a menu: its ProfileName tag, else the file name
    /// minus the camera.
    pub name: String,
    /// The camera it was made for: its UniqueCameraModel tag, else the file
    /// name.
    pub camera: String,
    /// Whether `camera` came from the tag (reliable) or the file name.
    pub camera_tagged: bool,
    pub origin: Origin,
}

/// Every installed profile, plus RawTherapee's camera-name aliases if they
/// have been downloaded.
#[derive(Debug, Default)]
pub struct Catalog {
    pub profiles: Vec<ProfileInfo>,
    aliases: HashMap<String, Vec<String>>,
}

static CATALOG: RwLock<Option<Arc<Catalog>>> = RwLock::new(None);

impl Catalog {
    /// The catalogue, scanning the folders on first use. Blocking.
    pub fn global() -> Arc<Catalog> {
        if let Some(c) = Self::loaded() {
            return c;
        }
        Self::rescan()
    }

    /// Scan the folders again — after a download, an import, or files added
    /// by hand. Blocking.
    pub fn rescan() -> Arc<Catalog> {
        let catalog = Arc::new(scan(&crate::core::paths::profiles_dir(), &downloaded_dir()));
        tracing::info!("camera profiles: {} installed", catalog.profiles.len());
        *CATALOG.write().unwrap_or_else(|e| e.into_inner()) = Some(catalog.clone());
        catalog
    }

    /// The catalogue if it has been scanned — never reads a file, so the UI
    /// thread can ask.
    pub fn loaded() -> Option<Arc<Catalog>> {
        CATALOG.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Profiles made for this camera, best first: the user's own before
    /// downloaded ones, "standard" renderings first within each.
    pub fn for_camera(&self, make: &str, model: &str) -> Vec<&ProfileInfo> {
        let mut found: Vec<&ProfileInfo> =
            self.profiles.iter().filter(|p| self.matches(p, make, model)).collect();
        found.sort_by_key(|p| {
            let lower = p.file_name.to_lowercase();
            let rank = if lower.contains("adobe standard") {
                0
            } else if lower.contains("standard") {
                1
            } else {
                2
            };
            (p.origin, rank, p.file_name.clone())
        });
        found
    }

    fn matches(&self, p: &ProfileInfo, make: &str, model: &str) -> bool {
        if p.camera_tagged {
            return self.same_camera(&p.camera, make, model);
        }
        // Untagged: the file name has to carry the model (the rule this
        // folder has always used, so existing setups keep working).
        // Whole tokens, so model "D5" does not match "Nikon D500.dcp".
        let file = crate::core::lens::tokens(&p.file_name);
        let model = crate::core::lens::tokens(model);
        !model.is_empty() && model.is_subset(&file)
    }

    /// `candidate` names this camera — directly, or as a RawTherapee alias
    /// (a Rebel T3i is a 600D).
    fn same_camera(&self, candidate: &str, make: &str, model: &str) -> bool {
        if same_camera(candidate, make, model) {
            return true;
        }
        self.aliases
            .iter()
            .filter(|(canonical, _)| same_camera(canonical, make, candidate))
            .flat_map(|(_, names)| names)
            .any(|alias| same_camera(alias, make, model))
    }

    /// This camera's default (`AppSettings::camera_profile_defaults`): the
    /// defaults are keyed by a profile's camera name ("Nikon D3300", as
    /// Preferences lists them), found here by the same matching as the
    /// profiles themselves — EXIF spells makers differently ("NIKON
    /// CORPORATION", "RICOH IMAGING" for a Pentax).
    pub fn default_for<'a>(
        &self,
        defaults: &'a HashMap<String, String>,
        make: &str,
        model: &str,
    ) -> Option<&'a String> {
        // Sorted, so two defaults matching one camera (a name and an alias)
        // resolve the same way every run instead of by HashMap order.
        defaults
            .iter()
            .filter(|(camera, _)| self.same_camera(camera, make, model))
            .min_by(|a, b| a.0.cmp(b.0))
            .map(|(_, v)| v)
    }

    /// The profile file named `file_name`, if installed (own first).
    pub fn find(&self, file_name: &str) -> Option<&ProfileInfo> {
        self.profiles
            .iter()
            .filter(|p| p.file_name == file_name)
            .min_by_key(|p| p.origin)
    }

    /// Every camera that has at least one profile, by display name.
    pub fn cameras(&self) -> Vec<String> {
        let mut cameras: Vec<String> = self.profiles.iter().map(|p| p.camera.clone()).collect();
        cameras.sort_by_key(|c| c.to_lowercase());
        cameras.dedup_by_key(|c| c.to_lowercase());
        cameras
    }
}

fn scan(own: &Path, downloaded: &Path) -> Catalog {
    let mut catalog = Catalog::default();
    for (dir, origin) in [(own, Origin::Own), (downloaded, Origin::Downloaded)] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dcp")))
            .collect();
        paths.sort();
        for path in paths {
            let Some(profile) = crate::raw::dcp::load_profile(&path) else { continue };
            let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if ProfileChoice::named(&file_name).is_none() {
                tracing::warn!("camera profile {file_name:?} skipped: its name is too long to choose");
                continue;
            }
            catalog.profiles.push(describe(&file_name, path, &profile, origin));
        }
    }
    if let Some(aliases) = std::fs::read_to_string(downloaded.join(ALIASES))
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
    {
        catalog.aliases = aliases;
    }
    catalog
}

fn describe(file_name: &str, path: PathBuf, profile: &DcpProfile, origin: Origin) -> ProfileInfo {
    let stem = file_name.strip_suffix(".dcp").or_else(|| file_name.strip_suffix(".DCP")).unwrap_or(file_name);
    let tagged = profile.camera_model.as_deref().map(str::trim).filter(|c| !c.is_empty());
    let camera = tagged.unwrap_or(stem).to_string();
    let name = profile
        .profile_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(&camera))
        .map(str::to_string)
        .unwrap_or_else(|| {
            // "Nikon D3300 Camera Standard" made for "Nikon D3300" → "Camera Standard".
            // RawTherapee names its profiles after the camera alone; say
            // where they came from instead of repeating the camera.
            let rest = stem.strip_prefix(camera.as_str()).map(str::trim).unwrap_or(stem);
            match (rest.is_empty(), origin) {
                (true, Origin::Downloaded) => "RawTherapee".to_string(),
                (true, Origin::Own) => stem.to_string(),
                (false, _) => rest.to_string(),
            }
        });
    ProfileInfo { file_name: file_name.to_string(), path, name, camera, camera_tagged: tagged.is_some(), origin }
}

/// Does `candidate` (a profile's camera name, or a file stem) name the
/// camera `make` + `model`? Compared as words, minus the maker's — so
/// "Nikon D3300" is "NIKON CORPORATION" + "NIKON D3300", and "D3100" is not.
pub fn same_camera(candidate: &str, make: &str, model: &str) -> bool {
    let maker: BTreeSet<String> = crate::core::lens::tokens(make);
    let strip = |s: &str| -> BTreeSet<String> {
        crate::core::lens::tokens(s).difference(&maker).cloned().collect()
    };
    let ours = strip(model);
    !ours.is_empty() && strip(candidate) == ours
}

/// A camera's name for menus and settings: the model, prefixed by the make
/// unless it already carries it ("NIKON D3300", "Canon EOS R5").
pub fn camera_name(make: &str, model: &str) -> String {
    let first = make.split_whitespace().next().unwrap_or("");
    if first.is_empty() || model.to_lowercase().starts_with(&first.to_lowercase()) {
        model.trim().to_string()
    } else {
        format!("{} {}", first, model.trim())
    }
}

/// The profile an image uses: its own choice, else the camera's default
/// (`defaults`, see [`Catalog::default_for`]), else the best installed match.
/// `None` → no profile (the matrix pipeline). Blocking on first use (scans
/// the folders); parsed profiles are cached by `raw::dcp::load_profile`.
pub fn resolve(
    make: &str,
    model: &str,
    choice: ProfileChoice,
    defaults: &HashMap<String, String>,
) -> Option<(ProfileInfo, Arc<DcpProfile>)> {
    let catalog = Catalog::global();
    let installed = |file: &str| catalog.find(file).cloned();
    let info = match choice.file_name().and_then(installed) {
        _ if choice.is_builtin() => return None,
        // A named profile that has since been removed falls back like Auto.
        Some(info) => info,
        None => match catalog.default_for(defaults, make, model).map(String::as_str) {
            Some("builtin") => return None,
            Some(file) if catalog.find(file).is_some() => installed(file)?,
            _ => catalog.for_camera(make, model).first().map(|p| (*p).clone())?,
        },
    };
    let profile = crate::raw::dcp::load_profile(&info.path)?;
    Some((info, profile))
}

/// The profile an image's GPU resources were built with, what chose it, and
/// the as-shot white balance read through it (a DCP's dual-illuminant
/// matrices define the image's Kelvin scale). Carried on `ImageResources`.
#[derive(Debug, Clone)]
pub struct ProfileUse {
    pub profile: Option<Arc<DcpProfile>>,
    pub choice: ProfileChoice,
    /// The per-camera default in force when it was chosen.
    pub camera_default: Option<String>,
    pub as_shot: (f32, f32),
}

impl ProfileUse {
    /// Pick the profile for `raw` with this choice and these defaults.
    /// Blocking on first use (see [`resolve`]).
    pub fn resolve(
        raw: &crate::raw::loader::RawDataResult,
        choice: ProfileChoice,
        defaults: &HashMap<String, String>,
    ) -> Self {
        let camera_default = Catalog::global().default_for(defaults, &raw.make, &raw.model).cloned();
        let profile = resolve(&raw.make, &raw.model, choice, defaults).map(|(_, profile)| profile);
        let as_shot = crate::color::as_shot_kelvin_tint_for(raw, profile.as_deref());
        Self { profile, choice, camera_default, as_shot }
    }

    /// Has the choice — the image's own, or its camera's default — moved
    /// since? Then the resources must be rebuilt to show it. Reads only the
    /// already-scanned catalogue (no file access), so the UI thread may ask.
    pub fn is_stale(&self, choice: ProfileChoice, defaults: &HashMap<String, String>, make: &str, model: &str) -> bool {
        let default_now = Catalog::loaded().and_then(|c| c.default_for(defaults, make, model).cloned());
        self.choice != choice || self.camera_default != default_now
    }
}

// ───────────────────────────── downloading ─────────────────────────────────

/// RawTherapee's profile folder. Listed through GitHub's API (one request);
/// files fetched from `raw.githubusercontent.com`. Only these two hosts are
/// contacted, and only when the user asks.
const RT_LISTING: &str = "https://api.github.com/repos/RawTherapee/RawTherapee/contents/rtdata/dcpprofiles?ref=dev";
const RT_FILES: &str = "https://raw.githubusercontent.com/RawTherapee/RawTherapee/dev/rtdata/dcpprofiles/";
/// Where the user can see the collection (Licenses page).
pub const RT_PAGE: &str = "https://github.com/RawTherapee/RawTherapee/tree/dev/rtdata/dcpprofiles";

/// RawTherapee's profiles are ~0.5 MB each; anything far larger is wrong.
const MAX_PROFILE: u64 = 16 << 20;

/// One profile RawTherapee publishes.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteProfile {
    pub file_name: String,
    /// Git blob hash — changes exactly when the file does.
    pub sha: String,
    pub size: u64,
}

/// RawTherapee's current collection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteCatalog {
    pub profiles: Vec<RemoteProfile>,
    pub aliases: HashMap<String, Vec<String>>,
}

impl RemoteCatalog {
    /// RawTherapee's profile for this camera (directly or through an alias).
    pub fn for_camera(&self, make: &str, model: &str) -> Option<&RemoteProfile> {
        let stem = |p: &RemoteProfile| p.file_name.trim_end_matches(".dcp").to_string();
        self.profiles
            .iter()
            .find(|p| same_camera(&stem(p), make, model))
            .or_else(|| {
                let canonical = self
                    .aliases
                    .iter()
                    .find(|(_, names)| names.iter().any(|n| same_camera(n, make, model)))?
                    .0;
                self.profiles.iter().find(|p| stem(p) == *canonical)
            })
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .https_only(true)
        .user_agent(format!("RawEditor/{} (camera profiles)", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// A plain `*.dcp` file name — no paths, nothing that could escape the
/// folder it is written to.
fn safe_dcp_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_CAPACITY
        && name.to_ascii_lowercase().ends_with(".dcp")
        && !name.contains(['/', '\\', '\0'])
        && !name.starts_with('.')
}

/// Percent-encode one URL path segment (file names carry spaces).
fn encode_segment(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn parse_listing(json: &str) -> Result<Vec<RemoteProfile>, String> {
    let entries: Vec<serde_json::Value> =
        serde_json::from_str(json).map_err(|e| format!("unexpected reply from GitHub: {e}"))?;
    Ok(entries
        .iter()
        .filter(|e| e["type"] == "file")
        .filter_map(|e| {
            let file_name = e["name"].as_str()?.to_string();
            safe_dcp_name(&file_name).then(|| RemoteProfile {
                file_name,
                sha: e["sha"].as_str().unwrap_or_default().to_string(),
                size: e["size"].as_u64().unwrap_or(0),
            })
        })
        .collect())
}

/// Fetch RawTherapee's current collection and aliases. Blocking.
pub fn remote_catalog() -> Result<RemoteCatalog, String> {
    let agent = agent();
    let listing = agent
        .get(RT_LISTING)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not reach GitHub: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("could not read GitHub's reply: {e}"))?;
    let profiles = parse_listing(&listing)?;
    // Optional: without it, renamed models simply don't match.
    let aliases = agent
        .get(&format!("{RT_FILES}{ALIASES}"))
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    Ok(RemoteCatalog { profiles, aliases })
}

/// The downloaded folder's record of what it holds: file name → blob hash.
fn read_manifest(dir: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(dir.join(MANIFEST))
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn write_manifest(dir: &Path, manifest: &HashMap<String, String>) -> Result<(), String> {
    let json = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    write_atomically(&dir.join(MANIFEST), json.as_bytes())
}

/// Write via a temporary file and a rename, so a crash never leaves half a
/// profile where a whole one is expected.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// Check that `bytes` are a usable DCP (written aside and parsed with the
/// real parser).
fn validate(bytes: &[u8], dir: &Path, name: &str) -> Result<(), String> {
    static PROBES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = PROBES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let probe = dir.join(format!(".{}-{n}.probe", std::process::id()));
    std::fs::write(&probe, bytes).map_err(|e| e.to_string())?;
    let parsed = crate::raw::dcp::parse_dcp(&probe);
    let _ = std::fs::remove_file(&probe);
    match parsed {
        Ok(p) if !p.has_forward_matrix() => Err(format!("{name} has no ForwardMatrix")),
        Ok(p) if p.color_matrix_1 != [1., 0., 0., 0., 1., 0., 0., 0., 1.] => Ok(()),
        Ok(_) => Err(format!("{name} has no colour matrix")),
        Err(e) => Err(format!("{name} is not a usable profile: {e}")),
    }
}

/// Download one of RawTherapee's profiles into `dir` (and keep the alias
/// table beside it). Blocking.
pub fn download(remote: &RemoteCatalog, profile: &RemoteProfile, dir: &Path) -> Result<(), String> {
    if !safe_dcp_name(&profile.file_name) {
        return Err(format!("refusing the file name {:?}", profile.file_name));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let bytes = agent()
        .get(&format!("{RT_FILES}{}", encode_segment(&profile.file_name)))
        .call()
        .map_err(|e| format!("download failed: {e}"))?
        .body_mut()
        .with_config()
        .limit(MAX_PROFILE)
        .read_to_vec()
        .map_err(|e| format!("download failed: {e}"))?;
    validate(&bytes, dir, &profile.file_name)?;
    write_atomically(&dir.join(&profile.file_name), &bytes)?;
    if !remote.aliases.is_empty() {
        if let Ok(json) = serde_json::to_string_pretty(&remote.aliases) {
            let _ = write_atomically(&dir.join(ALIASES), json.as_bytes());
        }
    }
    let mut manifest = read_manifest(dir);
    manifest.insert(profile.file_name.clone(), profile.sha.clone());
    write_manifest(dir, &manifest)?;
    tracing::info!("camera profile downloaded: {}", profile.file_name);
    Ok(())
}

/// Download RawTherapee's profile for this camera. `Ok(None)` when it has
/// none. Blocking.
pub fn download_for_camera(make: &str, model: &str) -> Result<Option<String>, String> {
    let remote = remote_catalog()?;
    let Some(profile) = remote.for_camera(make, model).cloned() else {
        return Ok(None);
    };
    download(&remote, &profile, &downloaded_dir())?;
    Ok(Some(profile.file_name))
}

/// What an update did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateReport {
    /// Downloaded profiles RawTherapee has changed since, now replaced.
    pub updated: Vec<String>,
    /// Downloaded profiles it no longer publishes (kept: images may use them).
    pub withdrawn: Vec<String>,
    /// Profiles whose download failed (the rest of the update still ran).
    pub failed: Vec<String>,
    pub checked: usize,
}

/// Bring every downloaded profile up to date with RawTherapee's. Blocking.
pub fn update_downloaded() -> Result<UpdateReport, String> {
    let dir = downloaded_dir();
    let manifest = read_manifest(&dir);
    let mut report = UpdateReport { checked: manifest.len(), ..Default::default() };
    if manifest.is_empty() {
        return Ok(report);
    }
    let remote = remote_catalog()?;
    for (name, sha) in &manifest {
        match remote.profiles.iter().find(|p| &p.file_name == name) {
            Some(p) if &p.sha != sha => {
                match download(&remote, p, &dir) {
                    Ok(()) => report.updated.push(name.clone()),
                    Err(e) => {
                        tracing::warn!("camera profile {name} not updated: {e}");
                        report.failed.push(name.clone());
                    }
                }
            }
            Some(_) => {}
            None => report.withdrawn.push(name.clone()),
        }
    }
    Ok(report)
}

/// Copy the user's chosen `.dcp` files into their profiles folder, checking
/// each parses first. Returns the names imported; files that fail are
/// reported, the rest still go in. Blocking.
pub fn import(files: &[PathBuf]) -> (Vec<String>, Vec<String>) {
    let dir = crate::core::paths::profiles_dir();
    let (mut imported, mut failed) = (Vec::new(), Vec::new());
    if let Err(e) = std::fs::create_dir_all(&dir) {
        failed.push(format!("{}: {e}", dir.display()));
        return (imported, failed);
    }
    for file in files {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let result = std::fs::read(file)
            .map_err(|e| format!("{name}: {e}"))
            .and_then(|bytes| {
                if !safe_dcp_name(&name) {
                    return Err(format!("{name}: not a .dcp file (or the name is too long)"));
                }
                validate(&bytes, &dir, &name)?;
                write_atomically(&dir.join(&name), &bytes)
            });
        match result {
            Ok(()) => imported.push(name),
            Err(e) => failed.push(e),
        }
    }
    (imported, failed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn untagged_profiles_match_whole_model_tokens() {
        let mk = |f: &str| ProfileInfo {
            file_name: f.into(),
            path: PathBuf::from(f),
            name: String::new(),
            camera: f.into(),
            camera_tagged: false,
            origin: Origin::Own,
        };
        let c = Catalog::default();
        assert!(!c.matches(&mk("Nikon D500.dcp"), "NIKON", "D5"));
        assert!(!c.matches(&mk("Nikon D5000.dcp"), "NIKON", "D5"));
        assert!(c.matches(&mk("Nikon D5 Camera Standard.dcp"), "NIKON", "D5"));
        assert!(c.matches(&mk("Canon EOS 5D Mark II.dcp"), "Canon", "EOS 5D"));
    }

    #[test]
    fn default_for_is_deterministic() {
        let c = Catalog::default();
        let mut d = HashMap::new();
        d.insert("Nikon D3300".to_string(), "b".to_string());
        d.insert("NIKON D3300".to_string(), "a".to_string());
        for _ in 0..20 {
            assert_eq!(c.default_for(&d, "NIKON CORPORATION", "NIKON D3300").map(String::as_str), Some("a"));
        }
    }

    use super::*;

    #[test]
    fn choices_round_trip_through_saved_edits() {
        for choice in [ProfileChoice::AUTO, ProfileChoice::BUILTIN, ProfileChoice::named("Nikon D3300.dcp").unwrap()] {
            let json = serde_json::to_string(&choice).unwrap();
            assert_eq!(serde_json::from_str::<ProfileChoice>(&json).unwrap(), choice, "{json}");
        }
        assert_eq!(serde_json::to_string(&ProfileChoice::AUTO).unwrap(), "\"auto\"");
        assert!(ProfileChoice::named(&"x".repeat(NAME_CAPACITY + 1)).is_none());
        assert_eq!(ProfileChoice::named("A.dcp").unwrap().file_name(), Some("A.dcp"));
    }

    #[test]
    fn cameras_match_by_model_words_not_maker_spelling() {
        assert!(same_camera("Nikon D3300", "NIKON CORPORATION", "NIKON D3300"));
        assert!(!same_camera("Nikon D3100", "NIKON CORPORATION", "NIKON D3300"));
        assert!(!same_camera("Nikon D330", "NIKON CORPORATION", "NIKON D3300"));
        assert!(same_camera("Canon EOS 5D Mark III", "Canon", "Canon EOS 5D Mark III"));
        assert!(!same_camera("Canon EOS 5D Mark II", "Canon", "Canon EOS 5D Mark III"));
        assert!(same_camera("Sony ILCE-7M3", "SONY", "ILCE-7M3"));
        assert!(same_camera("Pentax K-1", "RICOH IMAGING COMPANY, LTD.", "PENTAX K-1"));
        assert!(!same_camera("anything", "Canon", ""));
    }

    #[test]
    fn camera_names_carry_the_make_once() {
        assert_eq!(camera_name("NIKON CORPORATION", "NIKON D3300"), "NIKON D3300");
        assert_eq!(camera_name("SONY", "ILCE-7M3"), "SONY ILCE-7M3");
        assert_eq!(camera_name("Canon", "Canon EOS R5"), "Canon EOS R5");
    }

    #[test]
    fn remote_matching_follows_aliases() {
        let remote = RemoteCatalog {
            profiles: vec![
                RemoteProfile { file_name: "Canon EOS 600D.dcp".into(), sha: "a".into(), size: 1 },
                RemoteProfile { file_name: "Nikon D3100.dcp".into(), sha: "b".into(), size: 1 },
            ],
            aliases: HashMap::from([(
                "Canon EOS 600D".to_string(),
                vec!["Canon EOS Rebel T3i".to_string(), "Canon EOS Kiss X5".to_string()],
            )]),
        };
        assert_eq!(remote.for_camera("Canon", "Canon EOS 600D").map(|p| p.sha.as_str()), Some("a"));
        assert_eq!(remote.for_camera("Canon", "Canon EOS Rebel T3i").map(|p| p.sha.as_str()), Some("a"));
        assert!(remote.for_camera("NIKON CORPORATION", "NIKON D3300").is_none());
    }

    #[test]
    fn only_plain_dcp_names_are_written() {
        assert!(safe_dcp_name("Nikon D3100.dcp"));
        assert!(!safe_dcp_name("../evil.dcp"));
        assert!(!safe_dcp_name("dir/evil.dcp"));
        assert!(!safe_dcp_name(".hidden.dcp"));
        assert!(!safe_dcp_name("readme.txt"));
        assert_eq!(encode_segment("Nikon D3100.dcp"), "Nikon%20D3100.dcp");
    }

    #[test]
    fn listings_keep_only_dcp_files() {
        let json = r#"[
            {"name": "Nikon D3100.dcp", "type": "file", "sha": "abc", "size": 500},
            {"name": "camera_model_aliases.json", "type": "file", "sha": "x", "size": 9},
            {"name": "sub", "type": "dir", "sha": "y", "size": 0}
        ]"#;
        let listing = parse_listing(json).unwrap();
        assert_eq!(listing, vec![RemoteProfile { file_name: "Nikon D3100.dcp".into(), sha: "abc".into(), size: 500 }]);
        assert!(parse_listing(r#"{"message": "rate limited"}"#).is_err());
    }

    /// The real thing: run with
    /// `cargo test camera_profile_download -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn camera_profile_download_installs_a_usable_profile() {
        let remote = remote_catalog().expect("reachable");
        eprintln!("{} profiles, {} alias entries", remote.profiles.len(), remote.aliases.len());
        let profile = remote.for_camera("NIKON CORPORATION", "NIKON D3100").expect("RawTherapee has the D3100").clone();
        let dir = std::env::temp_dir().join(format!("raw-editor-dcp-live-{}", std::process::id()));
        download(&remote, &profile, &dir).expect("download");
        let catalog = scan(Path::new("/nonexistent"), &dir);
        eprintln!("{:#?}", catalog.profiles);
        let found = catalog.for_camera("NIKON CORPORATION", "NIKON D3100");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "RawTherapee");
        assert_eq!(read_manifest(&dir).get(&profile.file_name), Some(&profile.sha));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
