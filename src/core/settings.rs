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
