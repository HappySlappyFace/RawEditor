use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub cache_capacity: usize,
    pub raw_preload_budget_mb: u32,
    pub thumbnail_size: f32,
    pub auto_advance: bool,
    pub histogram_enabled: bool,
    pub preview_preload_behind: usize,
    pub preview_preload_ahead: usize,
    pub raw_preload_behind: usize,
    pub raw_preload_ahead: usize,
    /// Last per-category selection from the Copy Settings modal, so it
    /// survives a restart.
    ///
    /// `#[serde(default)]` is LOAD-BEARING, not decoration: no other field in
    /// this struct has it, so a settings.json written before this field
    /// existed would fail to parse — and `load_or_default` swallows that error
    /// and silently resets EVERY preference (cache size, preload windows,
    /// thumbnail size). Any field added here from now on needs the same.
    #[serde(default)]
    pub copy_categories: crate::core::types::CopyCategories,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            cache_capacity: 200,
            raw_preload_budget_mb: 1024,
            thumbnail_size: 220.0,
            auto_advance: false,
            histogram_enabled: true,
            preview_preload_behind: 10,
            preview_preload_ahead: 50,
            raw_preload_behind: 1,
            raw_preload_ahead: 4,
            copy_categories: crate::core::types::CopyCategories::default(),
        }
    }
}

impl AppSettings {
    pub fn load_or_default() -> Self {
        match Self::load() {
            Ok(settings) => settings,
            Err(e) => {
                tracing::warn!("Failed to load app settings, using defaults: {}", e);
                Self::default()
            }
        }
    }

    pub fn load() -> Result<Self, String> {
        let path = Self::settings_path();
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read settings file {}: {}", path.display(), e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse settings file {}: {}", path.display(), e))
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create settings directory {}: {}",
                    parent.display(),
                    e
                )
            })?;
        }

        let payload = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;
        std::fs::write(&path, payload)
            .map_err(|e| format!("Failed to write settings file {}: {}", path.display(), e))
    }

    pub fn settings_path() -> PathBuf {
        let mut path = dirs::data_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        path.push("raw-editor");
        path.push("settings.json");
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings file written before `copy_categories` existed must still
    /// load, keeping every other preference intact.
    ///
    /// Without `#[serde(default)]` this parse fails, `load_or_default`
    /// swallows the error, and the user silently loses their cache size,
    /// preload windows and thumbnail size on the next launch — a data-loss
    /// bug with no error message anywhere.
    #[test]
    fn legacy_settings_without_copy_categories_still_load() {
        let legacy = r#"{
            "cache_capacity": 350,
            "raw_preload_budget_mb": 2048,
            "thumbnail_size": 180.0,
            "auto_advance": true,
            "histogram_enabled": false,
            "preview_preload_behind": 5,
            "preview_preload_ahead": 25,
            "raw_preload_behind": 2,
            "raw_preload_ahead": 6
        }"#;

        let parsed: AppSettings =
            serde_json::from_str(legacy).expect("legacy settings must still parse");

        assert_eq!(parsed.cache_capacity, 350, "preferences were reset");
        assert_eq!(parsed.raw_preload_budget_mb, 2048);
        assert_eq!(parsed.thumbnail_size, 180.0);
        assert!(parsed.auto_advance);
        assert!(!parsed.histogram_enabled);
        assert_eq!(parsed.preview_preload_ahead, 25);
        assert_eq!(
            parsed.copy_categories,
            crate::core::types::CopyCategories::default()
        );
    }

    #[test]
    fn settings_round_trip() {
        let mut s = AppSettings::default();
        s.copy_categories.masks = true;
        s.cache_capacity = 123;
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.copy_categories, s.copy_categories);
        assert_eq!(back.cache_capacity, 123);
    }
}
