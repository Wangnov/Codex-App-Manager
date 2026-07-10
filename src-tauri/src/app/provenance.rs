//! Provenance store: records which Codex installs are managed by THIS app.
//!
//! Lives OUTSIDE any Codex bundle (in the manager's data dir) so it never
//! disturbs Codex's code signature. Classification:
//!   - `managed`  — the detected install's path is in our store
//!   - `external` — a Codex exists we did not record (official Store/DMG)
//!   - `none`     — no Codex detected
//!
//! Adoption ("纳管") records an external install after explicit user consent.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::app::atomic_file::{self, LoadOutcome};
use crate::app::config_health::StoreLoadHealth;
use crate::app::paths;
use crate::errors::AppError;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRecord {
    pub path: String,
    pub build: u64,
    /// "adopted-external" | "manager-installed"
    pub source: String,
    pub adopted_at_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceStore {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub managed: Vec<ManagedRecord>,
}

impl Default for ProvenanceStore {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            managed: Vec::new(),
        }
    }
}

fn store_path() -> Option<PathBuf> {
    paths::provenance_path()
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl ProvenanceStore {
    pub fn load() -> Self {
        Self::load_with_health().0
    }

    pub fn load_with_health() -> (Self, StoreLoadHealth) {
        let Some(path) = store_path() else {
            log::error!("configuration corrupt which=provenance detail=missing-data-dir");
            return (
                Self::default(),
                StoreLoadHealth::corrupt("无法定位 provenance.json 数据目录".to_string()),
            );
        };
        let backup_available = atomic_file::backup_path(&path).exists();
        if !path.exists() && !backup_available {
            return (Self::default(), StoreLoadHealth::ok());
        }

        let (store, outcome) = atomic_file::read_with_recovery::<Self>(&path);
        let mut health = match outcome {
            LoadOutcome::Ok => StoreLoadHealth::ok(),
            LoadOutcome::RecoveredFromBak => {
                log::warn!(
                    "configuration recovered from backup which=provenance detail=provenance.json"
                );
                StoreLoadHealth::recovered("provenance.json 已从 .bak 备份恢复".to_string())
            }
            LoadOutcome::Corrupt => {
                log::error!("configuration corrupt which=provenance detail=unrecoverable");
                StoreLoadHealth::corrupt(
                    "provenance.json 损坏且 .bak 备份不可用，已使用空托管记录".to_string(),
                )
            }
        };
        health.backup_available = backup_available;

        let mut store = store.unwrap_or_default();
        if store.schema_version > CURRENT_SCHEMA_VERSION {
            health.detail = Some(format!(
                "provenance.json schema_version={} 高于当前支持版本 {}",
                store.schema_version, CURRENT_SCHEMA_VERSION
            ));
        }
        store.schema_version = CURRENT_SCHEMA_VERSION;
        (store, health)
    }

    pub fn save(&self) -> Result<(), AppError> {
        let path = store_path().ok_or_else(|| AppError::Internal("no data directory".into()))?;
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| AppError::Internal(format!("serialize provenance: {e}")))?;
        atomic_file::write_atomic(&path, &json)
            .map_err(|e| AppError::Internal(format!("write provenance: {e}")))
    }

    pub fn is_managed(&self, path: &str) -> bool {
        self.managed.iter().any(|r| r.path == path)
    }

    /// Stricter than [`is_managed`]: the path AND the build must match a record.
    /// Guards destructive actions against path reuse (e.g. a manually-installed
    /// official Codex landing where a managed one used to be) and against a stale
    /// record left by a failed save — those won't match the current build.
    pub fn is_managed_build(&self, path: &str, build: u64) -> bool {
        self.managed
            .iter()
            .any(|r| r.path == path && r.build == build)
    }

    pub fn remove(&mut self, path: &str) {
        self.managed.retain(|r| r.path != path);
    }

    /// Record (or refresh) a managed install, keyed by path.
    pub fn record(&mut self, path: String, build: u64, source: &str) {
        self.schema_version = CURRENT_SCHEMA_VERSION;
        self.managed.retain(|r| r.path != path);
        self.managed.push(ManagedRecord {
            path,
            build,
            source: source.to_string(),
            adopted_at_unix: now_unix(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_classify_and_dedup() {
        let mut store = ProvenanceStore::default();
        assert!(!store.is_managed("/Applications/Codex.app"));

        store.record(
            "/Applications/Codex.app".to_string(),
            3575,
            "adopted-external",
        );
        assert!(store.is_managed("/Applications/Codex.app"));
        assert_eq!(store.managed.len(), 1);

        // Re-recording the same path updates in place (no duplicate).
        store.record(
            "/Applications/Codex.app".to_string(),
            3600,
            "manager-installed",
        );
        assert_eq!(store.managed.len(), 1);
        assert_eq!(store.managed[0].build, 3600);
    }

    #[test]
    fn old_schema_defaults_schema_version() {
        let store: ProvenanceStore =
            serde_json::from_str(r#"{"managed":[]}"#).expect("old provenance parses");
        assert_eq!(store.schema_version, CURRENT_SCHEMA_VERSION);
    }
}
