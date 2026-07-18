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
const DEFAULT_PERIODIC_CHECK_INTERVAL_SECONDS: u64 = 15 * 60;
const MIN_PERIODIC_CHECK_INTERVAL_SECONDS: u64 = 60;
const MAX_PERIODIC_CHECK_INTERVAL_SECONDS: u64 = 7 * 24 * 60 * 60;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    System,
    Direct,
    Custom,
}

impl ProxyMode {
    pub fn from_raw(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "system" => Ok(Self::System),
            "direct" => Ok(Self::Direct),
            "custom" => Ok(Self::Custom),
            _ => Err(raw.to_string()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Direct => "direct",
            Self::Custom => "custom",
        }
    }
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

fn default_proxy_mode() -> ProxyMode {
    ProxyMode::System
}

fn default_proxy_mode_string() -> String {
    ProxyMode::System.as_str().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedCodexUpdate {
    pub platform: String,
    pub target: String,
    pub version: String,
    pub skipped_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default = "default_source")]
    pub source: UpdateSource,
    pub custom_url: String,
    /// Legacy compatibility alias retained for older frontends/settings files.
    pub auto_check: bool,
    /// Check once when the home view starts.
    #[serde(default = "default_true")]
    pub check_on_startup: bool,
    /// Keep checking while the manager is open.
    #[serde(default = "default_true")]
    pub periodic_check: bool,
    /// Periodic check cadence in seconds. Defaults to 15 minutes.
    #[serde(default = "default_periodic_check_interval_seconds")]
    pub periodic_check_interval_seconds: u64,
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
    /// Network proxy behavior for checks and package downloads.
    #[serde(default = "default_proxy_mode")]
    pub proxy_mode: ProxyMode,
    /// Proxy URL used when proxy_mode is custom.
    #[serde(default)]
    pub custom_proxy_url: String,
    /// Disable Codex App's own embedded updater checks/downloads.
    #[serde(default)]
    pub disable_codex_self_updates: bool,
    /// One exact Codex app update the user chose not to be reminded about.
    #[serde(default)]
    pub skipped_codex_update: Option<SkippedCodexUpdate>,
    /// Persistent Codex UI theme selection (theme id), applied whenever the
    /// manager launches Codex. None = stock appearance.
    #[serde(default)]
    pub codex_theme: Option<String>,
    /// Extra local directory scanned for theme packages (a theme-studio
    /// checkout during development). Packages here shadow managed ones by id.
    #[serde(default)]
    pub codex_theme_dir: Option<String>,
    /// Where managed skins live (downloads, imports). None = the platform
    /// default (`paths::default_skins_store_dir`). Changed via the theme
    /// page, which migrates existing skins to the new location.
    #[serde(default)]
    pub codex_theme_store_dir: Option<String>,
    /// User-defined local skin groups, in display order. Each holds an ordered
    /// list of skin ids; a skin may sit in several groups. The source partitions
    /// (store-installed vs dev-local) are derived from each skin's origin, not
    /// stored here.
    #[serde(default)]
    pub skin_groups: Vec<SkinGroup>,
}

/// A user-defined grouping of local skins.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub skin_ids: Vec<String>,
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
    auto_check: Option<bool>,
    check_on_startup: Option<bool>,
    periodic_check: Option<bool>,
    periodic_check_interval_seconds: Option<u64>,
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
    #[serde(default = "default_proxy_mode_string")]
    proxy_mode: String,
    #[serde(default)]
    custom_proxy_url: String,
    #[serde(default)]
    disable_codex_self_updates: bool,
    #[serde(default)]
    skipped_codex_update: Option<SkippedCodexUpdate>,
    #[serde(default)]
    codex_theme: Option<String>,
    #[serde(default)]
    codex_theme_dir: Option<String>,
    #[serde(default)]
    codex_theme_store_dir: Option<String>,
    #[serde(default)]
    skin_groups: Vec<SkinGroup>,
}

fn default_true() -> bool {
    true
}

fn default_periodic_check_interval_seconds() -> u64 {
    DEFAULT_PERIODIC_CHECK_INTERVAL_SECONDS
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
            check_on_startup: true,
            periodic_check: true,
            periodic_check_interval_seconds: DEFAULT_PERIODIC_CHECK_INTERVAL_SECONDS,
            ask_before: true,
            signed_only: true,
            confirm_close: true,
            windows_install_mode: default_windows_install_mode(),
            install_root: default_install_root(),
            proxy_mode: ProxyMode::System,
            custom_proxy_url: String::new(),
            disable_codex_self_updates: false,
            skipped_codex_update: None,
            codex_theme: None,
            codex_theme_dir: None,
            codex_theme_store_dir: None,
            skin_groups: Vec::new(),
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
        let proxy_mode = ProxyMode::from_raw(&self.proxy_mode).unwrap_or(ProxyMode::System);
        let newer_schema = (self.schema_version > CURRENT_SCHEMA_VERSION).then(|| {
            format!(
                "settings.json schema_version={} 高于当前支持版本 {}",
                self.schema_version, CURRENT_SCHEMA_VERSION
            )
        });
        let legacy_auto_check = self.auto_check.unwrap_or(true);
        let check_on_startup = self.check_on_startup.unwrap_or(legacy_auto_check);
        let periodic_check = self.periodic_check.unwrap_or(legacy_auto_check);
        let auto_check = self.auto_check.unwrap_or(periodic_check);
        (
            AppSettings {
                schema_version: self.schema_version,
                source,
                custom_url: self.custom_url,
                auto_check,
                check_on_startup,
                periodic_check,
                periodic_check_interval_seconds: self
                    .periodic_check_interval_seconds
                    .unwrap_or(DEFAULT_PERIODIC_CHECK_INTERVAL_SECONDS),
                ask_before: self.ask_before,
                signed_only: self.signed_only,
                confirm_close: self.confirm_close,
                windows_install_mode: self.windows_install_mode,
                install_root: self.install_root,
                proxy_mode,
                custom_proxy_url: self.custom_proxy_url,
                disable_codex_self_updates: self.disable_codex_self_updates,
                skipped_codex_update: self.skipped_codex_update,
                codex_theme: self.codex_theme,
                codex_theme_dir: self.codex_theme_dir,
                codex_theme_store_dir: self.codex_theme_store_dir,
                skin_groups: self.skin_groups,
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
        self.auto_check = self.periodic_check;
        self.periodic_check_interval_seconds = self.periodic_check_interval_seconds.clamp(
            MIN_PERIODIC_CHECK_INTERVAL_SECONDS,
            MAX_PERIODIC_CHECK_INTERVAL_SECONDS,
        );
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
        self.custom_url = self.custom_url.trim().to_string();
        self.custom_proxy_url = self.custom_proxy_url.trim().to_string();
        // Empty custom modes are not a real runtime choice (update paths fall
        // back silently). Coerce so UI selection, disk, and runtime agree.
        if self.source == UpdateSource::Custom && self.custom_url.is_empty() {
            self.source = UpdateSource::Auto;
        }
        if self.proxy_mode == ProxyMode::Custom && self.custom_proxy_url.is_empty() {
            self.proxy_mode = ProxyMode::System;
        }
        if let Some(skipped) = &mut self.skipped_codex_update {
            skipped.platform = skipped.platform.trim().to_ascii_lowercase();
            skipped.target = skipped.target.trim().to_string();
            skipped.version = skipped.version.trim().to_string();
            if !matches!(skipped.platform.as_str(), "macos" | "windows")
                || skipped.target.is_empty()
                || skipped.version.is_empty()
            {
                self.skipped_codex_update = None;
            }
        }
    }

    pub fn load() -> Self {
        Self::load_with_health().0
    }

    pub fn load_with_health() -> (Self, StoreLoadHealth) {
        let Some(path) = store_path() else {
            log::error!("configuration corrupt which=settings detail=missing-data-dir");
            return (
                Self::default(),
                StoreLoadHealth::corrupt("无法定位 settings.json 数据目录".to_string()),
            );
        };
        let backup_available = atomic_file::backup_path(&path).exists();
        if !path.exists() && !backup_available {
            return (Self::default(), StoreLoadHealth::ok());
        }

        let (raw, outcome) = atomic_file::read_with_recovery::<RawAppSettings>(&path);
        let mut health = match outcome {
            LoadOutcome::Ok => StoreLoadHealth::ok(),
            LoadOutcome::RecoveredFromBak => {
                log::warn!(
                    "configuration recovered from backup which=settings detail=settings.json"
                );
                StoreLoadHealth::recovered("settings.json 已从 .bak 备份恢复".to_string())
            }
            LoadOutcome::Corrupt => {
                log::error!("configuration corrupt which=settings detail=unrecoverable");
                StoreLoadHealth::corrupt(
                    "settings.json 损坏且 .bak 备份不可用，已使用默认配置".to_string(),
                )
            }
        };
        health.backup_available = backup_available;

        let mut settings = match raw {
            Some(raw) => {
                let (settings, unknown_source, newer_schema) = raw.into_settings();
                if let Some(raw) = unknown_source {
                    health.unknown_source = Some(raw.clone());
                    log::warn!("settings contains unknown source unknown_source={raw}");
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
    use super::{
        AppSettings, ProxyMode, RawAppSettings, UpdateSource, CURRENT_SCHEMA_VERSION,
        DEFAULT_PERIODIC_CHECK_INTERVAL_SECONDS, MAX_PERIODIC_CHECK_INTERVAL_SECONDS,
        MIN_PERIODIC_CHECK_INTERVAL_SECONDS,
    };

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
        assert!(settings.auto_check);
        assert!(settings.check_on_startup);
        assert!(settings.periodic_check);
        assert_eq!(
            settings.periodic_check_interval_seconds,
            DEFAULT_PERIODIC_CHECK_INTERVAL_SECONDS
        );
        assert_eq!(settings.proxy_mode, ProxyMode::System);
        assert_eq!(settings.custom_proxy_url, "");
        assert!(!settings.disable_codex_self_updates);
        assert!(unknown_source.is_none());
        assert!(newer_schema.is_none());
    }

    #[test]
    fn legacy_auto_check_false_disables_startup_and_periodic_checks() {
        let raw: RawAppSettings = serde_json::from_str(
            r#"{
                "source": "auto",
                "customUrl": "",
                "autoCheck": false,
                "askBefore": true,
                "signedOnly": true
            }"#,
        )
        .unwrap();
        let (settings, _, _) = raw.into_settings();
        assert!(!settings.auto_check);
        assert!(!settings.check_on_startup);
        assert!(!settings.periodic_check);
        assert_eq!(
            settings.periodic_check_interval_seconds,
            DEFAULT_PERIODIC_CHECK_INTERVAL_SECONDS
        );
    }

    #[test]
    fn normalizes_periodic_interval_bounds() {
        let mut too_low = AppSettings {
            periodic_check_interval_seconds: 0,
            ..AppSettings::default()
        };
        too_low.normalize();
        assert_eq!(
            too_low.periodic_check_interval_seconds,
            MIN_PERIODIC_CHECK_INTERVAL_SECONDS
        );

        let mut too_high = AppSettings {
            periodic_check_interval_seconds: MAX_PERIODIC_CHECK_INTERVAL_SECONDS + 1,
            ..AppSettings::default()
        };
        too_high.normalize();
        assert_eq!(
            too_high.periodic_check_interval_seconds,
            MAX_PERIODIC_CHECK_INTERVAL_SECONDS
        );
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
    fn normalizes_unknown_proxy_mode_to_system() {
        let raw: RawAppSettings = serde_json::from_str(
            r#"{
                "schemaVersion": 1,
                "source": "auto",
                "customUrl": "",
                "proxyMode": "surprise",
                "customProxyUrl": " socks5h://127.0.0.1:7890 ",
                "autoCheck": true,
                "askBefore": true,
                "signedOnly": true
            }"#,
        )
        .unwrap();
        let (mut settings, _, _) = raw.into_settings();
        settings.normalize();
        assert_eq!(settings.proxy_mode, ProxyMode::System);
        assert_eq!(settings.custom_proxy_url, "socks5h://127.0.0.1:7890");
    }

    #[test]
    fn empty_custom_source_and_proxy_coerce_to_real_defaults() {
        let mut settings = AppSettings {
            source: UpdateSource::Custom,
            custom_url: "   ".to_string(),
            proxy_mode: ProxyMode::Custom,
            custom_proxy_url: String::new(),
            ..AppSettings::default()
        };
        settings.normalize();
        assert_eq!(settings.source, UpdateSource::Auto);
        assert_eq!(settings.custom_url, "");
        assert_eq!(settings.proxy_mode, ProxyMode::System);
        assert_eq!(settings.custom_proxy_url, "");
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
