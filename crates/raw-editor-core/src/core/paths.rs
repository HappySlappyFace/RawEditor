//! The two folders RawEditor keeps on disk, in one place.
//!
//! Every path below used to be rebuilt by hand at each call site
//! (`dirs::data_dir().or_else(dirs::home_dir)…join("raw-editor")`), with
//! slightly different fallbacks. Preferences › Folders opens these, so they
//! must be where the files actually go — hence one definition.
//!
//! | folder | Linux | macOS | Windows |
//! |---|---|---|---|
//! | [`data_dir`]  | `~/.local/share/raw-editor` | `~/Library/Application Support/raw-editor` | `%APPDATA%\raw-editor` |
//! | [`cache_dir`] | `~/.cache/raw-editor` | `~/Library/Caches/raw-editor` | `%LOCALAPPDATA%\raw-editor` |

use std::path::PathBuf;

const APP_DIR: &str = "raw-editor";

/// The catalogue (`raw_editor.db`), `settings.json`, DCP profiles
/// (`profiles/`), the user's own lensfun files (`lensfun/`) and the logs.
/// Everything here is the user's: losing it loses work.
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR)
}

/// Generated previews (thumbnail / instant / working tiers and edited
/// proxies). Disposable: missing files are re-queued at the next launch.
/// `None` when the platform reports neither a cache nor a home directory.
pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().or_else(dirs::home_dir).map(|dir| dir.join(APP_DIR))
}

/// Where DCP camera profiles are looked up.
pub fn profiles_dir() -> PathBuf {
    data_dir().join("profiles")
}

/// The user's own lensfun XML files. Searched before everything else, so a
/// hand-made calibration overrides the downloaded one.
pub fn lensfun_dir() -> PathBuf {
    data_dir().join("lensfun")
}

/// The lensfun database as downloaded by the app (`core::lensfun`). Owned
/// by the app — replaced wholesale on update — which is why it is not
/// `lensfun_dir` itself, where the user's files live.
pub fn lensfun_updates_dir() -> PathBuf {
    lensfun_dir().join("updates")
}

/// The AI models (`ml::models`), downloaded on request. Owned by the app;
/// in the data folder rather than the cache because re-downloading them is
/// slow and cache-clearing tools would take them.
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// AI mask bitmaps (`ml::masks`): `<image id>/<mask id>.png`. User work —
/// a saved edit refers to them.
pub fn ai_masks_dir() -> PathBuf {
    data_dir().join("ai_masks")
}

/// One of the folders Preferences › Folders can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppFolder {
    /// [`data_dir`]
    Data,
    /// [`cache_dir`]
    Cache,
    /// [`lensfun_dir`]
    Lensfun,
    /// [`profiles_dir`]
    Profiles,
    /// [`models_dir`]
    Models,
}

impl AppFolder {
    pub fn path(self) -> Option<PathBuf> {
        match self {
            AppFolder::Data => Some(data_dir()),
            AppFolder::Cache => cache_dir(),
            AppFolder::Lensfun => Some(lensfun_dir()),
            AppFolder::Profiles => Some(profiles_dir()),
            AppFolder::Models => Some(models_dir()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two must never coincide: the cache is advertised as safe to
    /// delete, the data folder holds the catalogue.
    #[test]
    fn data_and_cache_are_separate_raw_editor_folders() {
        let data = data_dir();
        assert!(data.ends_with(APP_DIR));
        assert!(profiles_dir().starts_with(&data));
        if let Some(cache) = cache_dir() {
            assert!(cache.ends_with(APP_DIR));
            assert_ne!(cache, data);
        }
    }
}
