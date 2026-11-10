//! Checking for a newer release.
//!
//! The release workflow attaches `latest.json` to every release, from a final
//! job that runs only after all platform builds have uploaded. The check
//! fetches it through GitHub's plain `releases/latest/download/` link rather
//! than the REST API: that link has no rate limit, while the API allows 60
//! unauthenticated requests an hour per IP (one office behind a NAT could use
//! that up).
//!
//! The check runs at startup only if the user said yes to the first-run prompt
//! (`AppSettings::update_check`), or when they click Check for Updates. It
//! downloads and installs nothing: an available update links to its release
//! page.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

pub const RELEASES_URL: &str = "https://github.com/HappySlappyFace/RawEditor/releases";
pub const MANIFEST_URL: &str =
    "https://github.com/HappySlappyFace/RawEditor/releases/latest/download/latest.json";

/// `latest.json`. Fields this version doesn't know are ignored, and missing
/// ones default, so the format can grow (signatures, for installing updates)
/// without breaking older apps.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub tag: String,
    /// The release page.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub published: String,
    /// Keyed by `platform()`.
    #[serde(default)]
    pub assets: HashMap<String, Asset>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Asset {
    pub file: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub bytes: u64,
}

/// A release newer than this build that has a download for this platform.
#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    pub version: String,
    pub url: String,
    pub published: String,
}

/// This build's key in `Manifest::assets`, e.g. `linux-x86_64`,
/// `macos-aarch64`. The release workflow uses the same names.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// What installed this copy, when that was a package manager: then the app
/// never checks for updates (the package manager does), never asks about
/// it, and writes no desktop entry of its own (the package ships one).
///
/// A source package names itself at build time (`RAWEDITOR_PACKAGED=AUR
/// cargo build …`). A binary package repackages the release build, which
/// can't, so on Linux a binary running from `/usr` counts too: only a
/// package manager puts one there.
pub fn packaged() -> Option<&'static str> {
    static PACKAGED: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *PACKAGED.get_or_init(|| {
        option_env!("RAWEDITOR_PACKAGED").or_else(|| {
            let exe = std::env::current_exe().ok()?;
            (cfg!(target_os = "linux") && exe.starts_with("/usr/")).then_some("your package manager")
        })
    })
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A semver-style version: `x.y.z` with an optional `-pre` suffix (and an
/// optional leading `v`). Build metadata (`+…`) is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    core: [u64; 3],
    pre: Option<String>,
}

impl Version {
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
            Some(_) => return None,
            None => (s, None),
        };
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let v = [parts.next()??, parts.next()??, parts.next()??];
        if parts.next().is_some() {
            return None;
        }
        Some(Self { core: v, pre })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    /// Release > its own pre-releases; pre-releases compare by their dotted
    /// identifiers, numbers numerically (semver §11, enough of it).
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        self.core.cmp(&other.core).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => Equal,
            (None, Some(_)) => Greater,
            (Some(_), None) => Less,
            (Some(a), Some(b)) => {
                let mut a = a.split('.');
                let mut b = b.split('.');
                loop {
                    match (a.next(), b.next()) {
                        (None, None) => return Equal,
                        (None, Some(_)) => return Less,
                        (Some(_), None) => return Greater,
                        (Some(x), Some(y)) => {
                            let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                                (Ok(x), Ok(y)) => x.cmp(&y),
                                (Ok(_), Err(_)) => Less,
                                (Err(_), Ok(_)) => Greater,
                                (Err(_), Err(_)) => x.cmp(y),
                            };
                            if o != Equal {
                                return o;
                            }
                        }
                    }
                }
            }
        })
    }
}

/// What `manifest` means for a build of `current` on `platform`: `Some` only
/// if it is newer AND has a download for that platform (never point someone
/// at a release they can't install).
pub fn evaluate(current: &str, platform: &str, manifest: &Manifest) -> Result<Option<Available>, String> {
    let latest = Version::parse(&manifest.version)
        .ok_or_else(|| format!("the release lists an unreadable version, \"{}\"", manifest.version))?;
    let Some(current) = Version::parse(current) else {
        return Err(format!("this build's version \"{current}\" is unreadable"));
    };
    if latest <= current || !manifest.assets.contains_key(platform) {
        return Ok(None);
    }
    let url = if manifest.url.starts_with("https://github.com/HappySlappyFace/RawEditor/") {
        manifest.url.clone()
    } else {
        // Only ever open this project's own pages from a fetched file.
        RELEASES_URL.to_string()
    };
    Ok(Some(Available { version: manifest.version.clone(), url, published: manifest.published.clone() }))
}

pub fn parse(json: &str) -> Result<Manifest, String> {
    serde_json::from_str(json).map_err(|e| format!("unreadable latest.json: {e}"))
}

/// Fetch `latest.json`. Blocking: run it inside `spawn_blocking`.
pub fn fetch() -> Result<Manifest, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .https_only(true)
        .user_agent(format!("RawEditor/{} (update check)", current_version()))
        .build()
        .into();
    let json = agent
        .get(MANIFEST_URL)
        .call()
        .map_err(|e| match e {
            // The newest release has no manifest yet: still building, or
            // published before the updater existed.
            ureq::Error::StatusCode(404) => "no release information published yet".to_string(),
            e => e.to_string(),
        })?
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse(&json)
}

/// `fetch` + `evaluate` for this build.
pub fn check() -> Result<Option<Available>, String> {
    let manifest = fetch()?;
    evaluate(current_version(), &platform(), &manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s} should parse"))
    }

    #[test]
    fn versions_order_like_semver() {
        assert!(v("0.9.23") > v("0.9.22"));
        assert!(v("0.10.0") > v("0.9.99"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("v0.9.23"), v("0.9.23"));
        assert_eq!(v("0.9.23+build.5"), v("0.9.23"));
        // A release outranks its own pre-releases, and the release workflow
        // turns tag v0.9.8E into 0.9.8-E.
        assert!(v("0.9.8") > v("0.9.8-E"));
        assert!(v("0.9.8-E") > v("0.9.7"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.2"));
        assert!(v("1.0.0-rc.1") > v("1.0.0-beta"));
        assert!(v("1.0.0-alpha.1") > v("1.0.0-alpha"));
    }

    #[test]
    fn junk_versions_do_not_parse() {
        for s in ["", "1.2", "1.2.3.4", "a.b.c", "1.2.3-", "latest"] {
            assert!(Version::parse(s).is_none(), "{s:?} parsed");
        }
    }

    fn manifest(version: &str, platforms: &[&str]) -> Manifest {
        Manifest {
            version: version.into(),
            tag: format!("v{version}"),
            url: format!("https://github.com/HappySlappyFace/RawEditor/releases/tag/v{version}"),
            published: "2026-11-08T16:00:00Z".into(),
            assets: platforms
                .iter()
                .map(|p| (p.to_string(), Asset { file: format!("{p}.zip"), sha256: String::new(), bytes: 1 }))
                .collect(),
        }
    }

    #[test]
    fn offers_only_newer_releases_built_for_this_platform() {
        let m = manifest("0.9.24", &["linux-x86_64", "windows-x86_64"]);
        let got = evaluate("0.9.23", "linux-x86_64", &m).unwrap().expect("newer");
        assert_eq!(got.version, "0.9.24");
        assert!(got.url.ends_with("/v0.9.24"));
        assert_eq!(evaluate("0.9.24", "linux-x86_64", &m), Ok(None));
        assert_eq!(evaluate("0.9.25", "linux-x86_64", &m), Ok(None));
        // No macOS download in that release: don't offer it there.
        assert_eq!(evaluate("0.9.23", "macos-aarch64", &m), Ok(None));
        assert!(evaluate("0.9.23", "linux-x86_64", &manifest("soon", &["linux-x86_64"])).is_err());
    }

    #[test]
    fn opens_only_this_projects_pages() {
        let mut m = manifest("0.9.24", &["linux-x86_64"]);
        m.url = "https://example.com/evil".into();
        let got = evaluate("0.9.23", "linux-x86_64", &m).unwrap().unwrap();
        assert_eq!(got.url, RELEASES_URL);
    }

    #[test]
    fn reads_the_workflows_manifest_and_ignores_unknown_fields() {
        let json = r#"{
            "format": 1,
            "version": "0.9.24",
            "tag": "v0.9.24",
            "url": "https://github.com/HappySlappyFace/RawEditor/releases/tag/v0.9.24",
            "published": "2026-11-09T10:00:00Z",
            "assets": {
                "linux-x86_64": {"file": "raw-editor-linux-v0.9.24.zip", "sha256": "ab", "bytes": 27766084},
                "macos-aarch64": {"file": "raw-editor-macos-arm64-v0.9.24.zip", "sha256": "cd", "bytes": 22096081, "signature": "later"}
            }
        }"#;
        let m = parse(json).unwrap();
        assert_eq!(m.version, "0.9.24");
        assert_eq!(m.assets["macos-aarch64"].bytes, 22096081);
        assert!(parse(r#"{"assets": {}}"#).is_err(), "a manifest without a version is refused");
    }

    #[test]
    fn this_build_has_a_readable_version_and_a_platform_key() {
        assert!(Version::parse(current_version()).is_some());
        assert!(platform().contains('-'));
    }
}
