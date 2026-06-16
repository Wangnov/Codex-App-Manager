use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use uuid::Uuid;

use crate::app::oplock::OperationManager;
use crate::errors::AppError;

const STALE_AFTER: Duration = Duration::from_secs(30 * 60);

pub struct StagingDir {
    root: PathBuf,
}

impl StagingDir {
    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn join(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn discard(self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }

    pub fn keep(self) -> PathBuf {
        self.root
    }
}

#[derive(Debug, Clone, Default)]
pub struct CleanupSummary {
    pub scanned: usize,
    pub removed: usize,
    pub failed: usize,
    pub skipped_busy: bool,
}

pub fn staging_root() -> PathBuf {
    std::env::temp_dir()
        .join("codex-app-manager")
        .join("staging")
}

pub fn create_unique_staging(prefix: &str) -> Result<StagingDir, AppError> {
    let root = staging_root().join(format!("{prefix}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root)
        .map_err(|e| AppError::Internal(format!("创建暂存目录失败: {e}")))?;
    set_owner_only(&root)?;
    Ok(StagingDir { root })
}

pub fn cleanup_stale_staging(ops: &OperationManager) -> CleanupSummary {
    let mut summary = CleanupSummary::default();
    if ops.is_busy() {
        summary.skipped_busy = true;
        return summary;
    }

    let root = staging_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return summary;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_update_dir = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("update-"));
        if !is_update_dir || !path.is_dir() {
            continue;
        }
        summary.scanned += 1;
        if !is_stale(&path, now) {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => summary.removed += 1,
            Err(_) => summary.failed += 1,
        }
    }
    summary
}

fn is_stale(path: &Path, now: SystemTime) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    now.duration_since(modified)
        .map(|age| age >= STALE_AFTER)
        .unwrap_or(false)
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(path, permissions)
        .map_err(|e| AppError::Internal(format!("设置暂存目录权限失败: {e}")))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{cleanup_stale_staging, create_unique_staging};
    use crate::app::oplock::{OperationKind, OperationManager};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn lock_path(name: &str) -> std::path::PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("staging-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("operation.lock")
    }

    #[test]
    fn create_unique_staging_uses_distinct_update_dirs() {
        let first = create_unique_staging("update").unwrap();
        let second = create_unique_staging("update").unwrap();
        assert_ne!(first.path(), second.path());
        assert!(first.path().is_dir());
        assert!(second.path().is_dir());
        assert!(first
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .starts_with("update-"));
        first.discard();
        second.discard();
    }

    #[cfg(unix)]
    #[test]
    fn create_unique_staging_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let staging = create_unique_staging("update").unwrap();
        let mode = fs::metadata(staging.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        staging.discard();
    }

    #[test]
    fn cleanup_skips_everything_while_operation_is_busy() {
        let staging = create_unique_staging("update").unwrap();
        let manager = OperationManager::new(lock_path("busy"));
        let _guard = manager.begin(OperationKind::Update).unwrap();
        let summary = cleanup_stale_staging(&manager);
        assert!(summary.skipped_busy);
        assert!(staging.path().exists());
        staging.discard();
    }
}
