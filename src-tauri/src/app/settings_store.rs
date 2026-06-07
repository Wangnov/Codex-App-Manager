//! Persisted app settings — chiefly the update source, so the user can point the
//! updater at the mirror, OpenAI directly, or a custom URL instead of a
//! hard-coded domain. Stored as JSON in the manager's data dir (outside any
//! Codex bundle), mirroring `provenance::ProvenanceStore`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::adapters::host;
use crate::domain::target::Target;
use crate::errors::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// "auto" | "mirror" | "official" | "custom"
    pub source: String,
    pub custom_url: String,
    pub auto_check: bool,
    pub ask_before: bool,
    /// Always true — surfaced read-only. We never install an unsigned bundle.
    pub signed_only: bool,
    /// Ask before closing (quitting) the window. Defaults true; tolerated as
    /// missing in an older settings.json via serde default.
    #[serde(default = "default_true")]
    pub confirm_close: bool,
    /// "msix" | "portable" — user-facing Windows install preference. MSIX can
    /// still fall back to portable when the machine blocks sideloading.
    #[serde(default = "default_windows_install_mode")]
    pub windows_install_mode: String,
    /// Portable Windows install root. Kept across uninstall so the next fresh
    /// portable install can default to the user's last chosen location.
    #[serde(default = "default_install_root")]
    pub install_root: String,
}

fn default_true() -> bool {
    true
}

fn default_windows_install_mode() -> String {
    "msix".to_string()
}

pub fn default_install_root() -> String {
    host::default_install_root(&Target::current())
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            source: "auto".to_string(),
            custom_url: String::new(),
            auto_check: true,
            ask_before: true,
            signed_only: true,
            confirm_close: true,
            windows_install_mode: default_windows_install_mode(),
            install_root: default_install_root(),
        }
    }
}

fn store_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("io.github", "wangnov", "codexappmanager")
        .map(|dirs| dirs.data_dir().join("settings.json"))
}

impl AppSettings {
    pub fn normalize(&mut self) {
        self.signed_only = true; // enforce regardless of what is on disk
        if !matches!(self.windows_install_mode.as_str(), "msix" | "portable") {
            self.windows_install_mode = default_windows_install_mode();
        }
        if self.install_root.trim().is_empty()
            || !PathBuf::from(self.install_root.trim()).is_absolute()
        {
            self.install_root = default_install_root();
        } else {
            self.install_root = self.install_root.trim().to_string();
        }
    }

    pub fn load() -> Self {
        let Some(path) = store_path() else {
            return Self::default();
        };
        let mut s: Self = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        s.normalize();
        s
    }

    pub fn save(&self) -> Result<(), AppError> {
        let path = store_path().ok_or_else(|| AppError::Internal("no data directory".into()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::Internal(format!("create data dir: {e}")))?;
        }
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| AppError::Internal(format!("serialize settings: {e}")))?;
        std::fs::write(&path, json).map_err(|e| AppError::Internal(format!("write settings: {e}")))
    }
}
