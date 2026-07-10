use std::path::Path;

use serde::Serialize;

use crate::app::atomic_file;
use crate::app::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigStatus {
    #[default]
    Ok,
    Recovered,
    Corrupt,
}

impl ConfigStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Recovered => "recovered",
            Self::Corrupt => "corrupt",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StoreLoadHealth {
    pub status: ConfigStatus,
    pub unknown_source: Option<String>,
    pub detail: Option<String>,
    pub backup_available: bool,
}

impl StoreLoadHealth {
    pub fn ok() -> Self {
        Self::default()
    }

    pub fn recovered(detail: String) -> Self {
        Self {
            status: ConfigStatus::Recovered,
            detail: Some(detail),
            ..Self::default()
        }
    }

    pub fn corrupt(detail: String) -> Self {
        Self {
            status: ConfigStatus::Corrupt,
            detail: Some(detail),
            ..Self::default()
        }
    }

    pub fn with_backup(mut self, available: bool) -> Self {
        self.backup_available = available;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigHealth {
    pub settings_status: String,
    pub provenance_status: String,
    pub unknown_source: Option<String>,
    pub detail: Option<String>,
    /// True when `settings.json.bak` exists and can be restored.
    pub settings_backup_available: bool,
    /// True when `provenance.json.bak` exists and can be restored.
    pub provenance_backup_available: bool,
}

impl ConfigHealth {
    pub fn from_parts(settings: StoreLoadHealth, provenance: StoreLoadHealth) -> Self {
        let detail = [settings.detail, provenance.detail]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("；");
        Self {
            settings_status: settings.status.as_str().to_string(),
            provenance_status: provenance.status.as_str().to_string(),
            unknown_source: settings.unknown_source,
            detail: (!detail.is_empty()).then_some(detail),
            settings_backup_available: settings.backup_available,
            provenance_backup_available: provenance.backup_available,
        }
    }

    /// Probe the live data directory for `.bak` files and fold that into an
    /// already-built health snapshot (used after restore/reset/load).
    pub fn with_live_backup_flags(mut self) -> Self {
        self.settings_backup_available = backup_exists_for(paths::settings_path());
        self.provenance_backup_available = backup_exists_for(paths::provenance_path());
        self
    }

    pub fn is_ok(&self) -> bool {
        self.settings_status == "ok"
            && self.provenance_status == "ok"
            && self.unknown_source.is_none()
    }

    /// True when either store is degraded enough that the UI should surface a
    /// persistent recovery banner (corrupt/recovered/unknown source).
    pub fn needs_attention(&self) -> bool {
        !self.is_ok()
    }
}

fn backup_exists_for(path: Option<std::path::PathBuf>) -> bool {
    path.map(|p| atomic_file::backup_path(Path::new(&p)).exists())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_parts_joins_details_and_flags() {
        let settings = StoreLoadHealth::corrupt("settings broken".into()).with_backup(true);
        let provenance = StoreLoadHealth::recovered("provenance from bak".into()).with_backup(false);
        let health = ConfigHealth::from_parts(settings, provenance);
        assert_eq!(health.settings_status, "corrupt");
        assert_eq!(health.provenance_status, "recovered");
        assert!(health.settings_backup_available);
        assert!(!health.provenance_backup_available);
        assert!(health.detail.as_deref().unwrap().contains("settings broken"));
        assert!(health.detail.as_deref().unwrap().contains("provenance from bak"));
        assert!(health.needs_attention());
        assert!(!health.is_ok());
    }

    #[test]
    fn ok_when_both_stores_clean() {
        let health = ConfigHealth::from_parts(StoreLoadHealth::ok(), StoreLoadHealth::ok());
        assert!(health.is_ok());
        assert!(!health.needs_attention());
    }
}
