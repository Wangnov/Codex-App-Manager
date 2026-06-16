//! Persisted app settings — chiefly the update source, so the user can point the
//! updater at the mirror, OpenAI directly, or a custom URL instead of a
//! hard-coded domain. Stored as JSON in the manager's data dir (outside any
//! Codex bundle), mirroring `provenance::ProvenanceStore`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::adapters::host;
use crate::app::atomic_file::{self, LoadOutcome};
use crate::app::config_health::StoreLoadHealth;
use crate::app::paths;
use crate::domain::target::Target;
use crate::errors::AppError;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateSource {
    Auto,
    Mirror,
    Official,
    Custom,
}

impl UpdateSource {
    pub fn from_raw(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "mirror" => Ok(Self::Mirror),
            "official" => Ok(Self::Official),
            "custom" => Ok(Self::Custom),
            _ => Err(raw.to_string()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Mirror => "mirror",
            Self::Official => "official",
            Self::Custom => "custom",
        }
    }
}

fn default_source() -> UpdateSource {
    UpdateSource::Auto
}

fn default_source_string() -> String {
    UpdateSource::Auto.as_str().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default = "default_source")]
    pub source: UpdateSource,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAppSettings {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    #[serde(default = "default_source_string")]
    source: String,
    #[serde(default)]
    custom_url: String,
    #[serde(default = "default_true")]
    auto_check: bool,
    #[serde(default = "default_true")]
    ask_before: bool,
    #[serde(default = "default_true")]
    signed_only: bool,
    #[serde(default = "default_true")]
    confirm_close: bool,
    #[serde(default = "default_windows_install_mode")]
    windows_install_mode: String,
    #[serde(default = "default_install_root")]
    install_root: String,
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
            schema_version: CURRENT_SCHEMA_VERSION,
            source: UpdateSource::Auto,
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
    paths::settings_path()
}

impl RawAppSettings {
    fn into_settings(self) -> (AppSettings, Option<String>, Option<String>) {
        let (source, unknown_source) = match UpdateSource::from_raw(&self.source) {
            Ok(source) => (source, None),
            Err(raw) => (UpdateSource::Auto, Some(raw)),
        };
        let newer_schema = (self.schema_version > CURRENT_SCHEMA_VERSION).then(|| {
            format!(
                "settings.json schema_version={} 高于当前支持版本 {}",
                self.schema_version, CURRENT_SCHEMA_VERSION
            )
        });
        (
            AppSettings {
                schema_version: self.schema_version,
                source,
                custom_url: self.custom_url,
                auto_check: self.auto_check,
                ask_before: self.ask_before,
                signed_only: self.signed_only,
                confirm_close: self.confirm_close,
                windows_install_mode: self.windows_install_mode,
                install_root: self.install_root,
            },
            unknown_source,
            newer_schema,
        )
    }
}

fn append_detail(health: &mut StoreLoadHealth, detail: String) {
    match &mut health.detail {
        Some(existing) => {
            existing.push('；');
            existing.push_str(&detail);
        }
        None => health.detail = Some(detail),
    }
}

impl AppSettings {
    pub fn normalize(&mut self) {
        self.schema_version = CURRENT_SCHEMA_VERSION;
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
        Self::load_with_health().0
    }

    pub fn load_with_health() -> (Self, StoreLoadHealth) {
        let Some(path) = store_path() else {
            return (
                Self::default(),
                StoreLoadHealth::corrupt("无法定位 settings.json 数据目录".to_string()),
            );
        };
        if !path.exists() && !atomic_file::backup_path(&path).exists() {
            return (Self::default(), StoreLoadHealth::ok());
        }

        let (raw, outcome) = atomic_file::read_with_recovery::<RawAppSettings>(&path);
        let mut health = match outcome {
            LoadOutcome::Ok => StoreLoadHealth::ok(),
            LoadOutcome::RecoveredFromBak => {
                StoreLoadHealth::recovered("settings.json 已从 .bak 备份恢复".to_string())
            }
            LoadOutcome::Corrupt => StoreLoadHealth::corrupt(
                "settings.json 损坏且 .bak 备份不可用，已使用默认配置".to_string(),
            ),
        };

        let mut settings = match raw {
            Some(raw) => {
                let (settings, unknown_source, newer_schema) = raw.into_settings();
                if let Some(raw) = unknown_source {
                    health.unknown_source = Some(raw.clone());
                    append_detail(&mut health, format!("未知更新源 {raw:?} 已归一为 auto"));
                }
                if let Some(detail) = newer_schema {
                    append_detail(&mut health, detail);
                }
                settings
            }
            None => Self::default(),
        };
        settings.normalize();
        (settings, health)
    }

    pub fn save(&self) -> Result<(), AppError> {
        let path = store_path().ok_or_else(|| AppError::Internal("no data directory".into()))?;
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| AppError::Internal(format!("serialize settings: {e}")))?;
        atomic_file::write_atomic(&path, &json)
            .map_err(|e| AppError::Internal(format!("write settings: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::{AppSettings, RawAppSettings, UpdateSource, CURRENT_SCHEMA_VERSION};

    #[test]
    fn old_schema_defaults_schema_version() {
        let raw: RawAppSettings = serde_json::from_str(
            r#"{
                "source": "mirror",
                "customUrl": "",
                "autoCheck": true,
                "askBefore": true,
                "signedOnly": true
            }"#,
        )
        .unwrap();
        let (settings, unknown_source, newer_schema) = raw.into_settings();
        assert_eq!(settings.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(settings.source, UpdateSource::Mirror);
        assert!(unknown_source.is_none());
        assert!(newer_schema.is_none());
    }

    #[test]
    fn unknown_source_normalizes_to_auto_with_warning() {
        let raw: RawAppSettings = serde_json::from_str(
            r#"{
                "schemaVersion": 1,
                "source": "surprise",
                "customUrl": "",
                "autoCheck": true,
                "askBefore": true,
                "signedOnly": true
            }"#,
        )
        .unwrap();
        let (mut settings, unknown_source, _) = raw.into_settings();
        settings.normalize();
        assert_eq!(settings.source, UpdateSource::Auto);
        assert_eq!(unknown_source.as_deref(), Some("surprise"));
    }

    #[test]
    fn app_settings_serializes_source_as_lowercase_string() {
        let settings = AppSettings {
            source: UpdateSource::Custom,
            ..AppSettings::default()
        };
        let value = serde_json::to_value(settings).unwrap();
        assert_eq!(value["source"], "custom");
    }
}
