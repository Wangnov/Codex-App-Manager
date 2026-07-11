//! Durable Windows manager-updater handoff state.
//!
//! `tauri-plugin-updater` launches NSIS and immediately terminates the old
//! process. A renderer event cannot be the only evidence during that gap, so
//! the backend fsyncs this record before entering `Update::install`. A newly
//! opened old build restores both the UI fence and the shared operation lock.

#[cfg(any(target_os = "windows", test))]
use std::{fs, path::Path};
use std::{
    io,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

#[cfg(any(target_os = "windows", test))]
use crate::app::atomic_file;
#[cfg(target_os = "windows")]
use crate::app::paths;

pub const HANDOFF_MAX_AGE_MS: u64 = 10 * 60 * 1_000;
const MAX_FUTURE_SKEW_MS: u64 = 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerUpdateHandoff {
    pub version: String,
    pub current_version: String,
    pub body: Option<String>,
    pub started_at_unix_ms: u64,
}

impl ManagerUpdateHandoff {
    pub fn is_expired_at(&self, now_unix_ms: u64) -> bool {
        self.started_at_unix_ms == 0
            || self.started_at_unix_ms > now_unix_ms.saturating_add(MAX_FUTURE_SKEW_MS)
            || now_unix_ms.saturating_sub(self.started_at_unix_ms) >= HANDOFF_MAX_AGE_MS
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagerUpdateHandoffStatus {
    None,
    Active(ManagerUpdateHandoff),
    Expired(ManagerUpdateHandoff),
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(target_os = "windows")]
fn handoff_path() -> io::Result<std::path::PathBuf> {
    paths::data_dir()
        .map(|dir| dir.join("manager-update-handoff.json"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "manager data directory unavailable",
            )
        })
}

pub fn persist_for_platform(record: &ManagerUpdateHandoff) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        persist_at(&handoff_path()?, record)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = record;
        Ok(())
    }
}

pub fn clear_for_platform() {
    #[cfg(target_os = "windows")]
    if let Ok(path) = handoff_path() {
        clear_at(&path);
    }
}

pub fn status_for_platform(current_version: &str) -> ManagerUpdateHandoffStatus {
    #[cfg(target_os = "windows")]
    {
        handoff_path()
            .ok()
            .map_or(ManagerUpdateHandoffStatus::None, |path| {
                status_at(&path, current_version, now_unix_ms())
            })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = current_version;
        ManagerUpdateHandoffStatus::None
    }
}

#[cfg(any(target_os = "windows", test))]
fn persist_at(path: &Path, record: &ManagerUpdateHandoff) -> io::Result<()> {
    let bytes = serde_json::to_vec(record)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    atomic_file::write_atomic(path, &bytes)
}

#[cfg(any(target_os = "windows", test))]
fn load_at(path: &Path) -> Option<ManagerUpdateHandoff> {
    let backup = atomic_file::backup_path(path);
    if !path.exists() && !backup.exists() {
        return None;
    }
    atomic_file::read_with_recovery(path).0
}

#[cfg(any(target_os = "windows", test))]
fn clear_at(path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(atomic_file::backup_path(path));
}

#[cfg(any(target_os = "windows", test))]
fn status_at(path: &Path, current_version: &str, now: u64) -> ManagerUpdateHandoffStatus {
    let Some(record) = load_at(path) else {
        clear_at(path);
        return ManagerUpdateHandoffStatus::None;
    };
    if record.version == current_version || record.current_version != current_version {
        clear_at(path);
        return ManagerUpdateHandoffStatus::None;
    }
    if record.is_expired_at(now) {
        clear_at(path);
        return ManagerUpdateHandoffStatus::Expired(record);
    }
    ManagerUpdateHandoffStatus::Active(record)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        clear_at, load_at, persist_at, status_at, ManagerUpdateHandoff, ManagerUpdateHandoffStatus,
        HANDOFF_MAX_AGE_MS,
    };

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn test_path(name: &str) -> std::path::PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!(
                "manager-handoff-{name}-{}-{id}",
                std::process::id()
            ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("handoff.json")
    }

    fn record(started_at_unix_ms: u64) -> ManagerUpdateHandoff {
        ManagerUpdateHandoff {
            version: "2.0.0".into(),
            current_version: "1.0.0".into(),
            body: Some("notes".into()),
            started_at_unix_ms,
        }
    }

    #[test]
    fn roundtrips_and_clears_both_primary_and_backup() {
        let path = test_path("roundtrip");
        persist_at(&path, &record(100)).unwrap();
        persist_at(&path, &record(200)).unwrap();
        assert_eq!(load_at(&path), Some(record(200)));

        clear_at(&path);
        assert!(load_at(&path).is_none());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn status_only_fences_the_exact_old_version_for_a_bounded_window() {
        let path = test_path("status");
        persist_at(&path, &record(1_000)).unwrap();
        assert!(matches!(
            status_at(&path, "1.0.0", 1_001),
            ManagerUpdateHandoffStatus::Active(_)
        ));

        assert!(matches!(
            status_at(&path, "1.0.0", 1_000 + HANDOFF_MAX_AGE_MS),
            ManagerUpdateHandoffStatus::Expired(_)
        ));
        assert!(load_at(&path).is_none());

        persist_at(&path, &record(2_000)).unwrap();
        assert_eq!(
            status_at(&path, "2.0.0", 2_001),
            ManagerUpdateHandoffStatus::None
        );
        assert!(load_at(&path).is_none());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn rejects_zero_and_implausibly_future_timestamps() {
        assert!(record(0).is_expired_at(1));
        assert!(record(61_001).is_expired_at(1));
    }
}
