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
    if let Err(err) = std::fs::create_dir_all(&root) {
        log::error!(
            "failed to create staging directory path={} prefix={prefix} error={err}",
            root.display()
        );
        return Err(AppError::Internal(format!("创建暂存目录失败: {err}")));
    }
    if let Err(err) = set_owner_only(&root) {
        log::error!(
            "failed to secure staging directory path={} prefix={prefix} error={err}",
            root.display()
        );
        return Err(err);
    }
    log::info!(
        "created staging directory path={} prefix={prefix}",
        root.display()
    );
    Ok(StagingDir { root })
}

/// Root of the persistent download cache — SEPARATE from the per-run `update-*`
/// staging dirs. A unique staging dir is deleted wholesale on pause/cancel
/// (`StagingDir::discard`), which is exactly what used to eat a paused
/// download's `.part` and make "再次更新会继续下载" a lie. Download artifacts
/// live here instead, so the engine's `.part` (kept on pause, removed on
/// cancel) survives across perform/install calls and the next run resumes it.
pub fn download_cache_root() -> PathBuf {
    staging_root().join("downloads")
}

/// Stable on-disk path for the artifact at `url`. Keyed by a per-URL hash so a
/// changed target (new build / different mirror) never resumes onto a stale
/// partial — a different URL is a different file. The human-readable
/// `file_name` is preserved (sanitized) as a suffix so the staged artifact is
/// still recognizable on disk and keeps its extension for downstream tooling.
pub fn download_cache_path(url: &str, file_name: &str) -> Result<PathBuf, AppError> {
    let root = download_cache_root();
    std::fs::create_dir_all(&root)
        .map_err(|e| AppError::Internal(format!("创建下载缓存目录失败: {e}")))?;
    set_owner_only(&root)?;
    Ok(root.join(format!(
        "{:016x}-{}",
        fnv1a64(url.as_bytes()),
        sanitize_file_name(file_name)
    )))
}

/// FNV-1a (64-bit). A fixed, toolchain-independent digest so a cached download's
/// directory name stays identical across manager updates — unlike `DefaultHasher`
/// (SipHash), whose seed/impl can shift between Rust versions and would orphan
/// the cache on every upgrade. Collision resistance isn't security-critical: the
/// artifact's size + EdDSA/SHA-256 verification is the real gate; this only
/// namespaces per-URL partials so different targets never resume onto each other.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Drop every cached download. Used when the user cancels from the paused state
/// (the engine already removed the `.part` on an in-flight cancel, but a paused
/// `.part` is still on disk) and after a successful update consumes the
/// artifact. Only one update runs at a time, so clearing the whole dir is safe.
/// Returns `Ok` if the cache is already gone, `Err` only on a real removal
/// failure so a paused-state cancel can surface "didn't actually discard".
pub fn clear_download_cache() -> std::io::Result<()> {
    match std::fs::remove_dir_all(download_cache_root()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Keep download cache file names to a safe, path-separator-free charset. The
/// caller's `file_name` comes from a URL tail or MSIX moniker; this defends
/// against an upstream path-traversal-ish name escaping the cache dir.
fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() {
        "download.bin".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn cleanup_stale_staging(ops: &OperationManager) -> CleanupSummary {
    let mut summary = CleanupSummary::default();
    if ops.is_busy() {
        summary.skipped_busy = true;
        log::info!("staging cleanup skipped skipped_busy=true");
        return summary;
    }

    // Never delete paths still referenced by pending / NeedsManual install txs.
    let protected = crate::app::install_tx::protected_paths();

    let root = staging_root();
    let now = SystemTime::now();
    if let Ok(entries) = std::fs::read_dir(&root) {
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
            if crate::app::install_tx::path_is_protected(&path, &protected) {
                log::info!("staging cleanup skipped protected path={}", path.display());
                continue;
            }
            if !is_stale(&path, now) {
                continue;
            }
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {
                    summary.removed += 1;
                    let path_display = path.display();
                    log::debug!("staging cleanup removed path={path_display}");
                }
                Err(err) => {
                    summary.failed += 1;
                    let path_display = path.display();
                    log::debug!("staging cleanup failed path={path_display} error={err}");
                }
            }
        }
    } else {
        let path = root.display();
        log::debug!("staging cleanup found no root path={path}");
    }

    // Prune stale cached downloads too. A paused `.part` younger than STALE_AFTER
    // is left intact so a resume still finds it; only abandoned partials/artifacts
    // are reclaimed. Guarded by the same `is_busy` check above, so an in-flight
    // download (op active) is never touched.
    if let Ok(entries) = std::fs::read_dir(download_cache_root()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !is_stale(&path, now) {
                continue;
            }
            if crate::app::install_tx::path_is_protected(&path, &protected) {
                continue;
            }
            summary.scanned += 1;
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    summary.removed += 1;
                    // Older releases derived the cache suffix from the raw URL
                    // tail, so a historical filename can contain a presigned
                    // query. Never carry cache paths across the log boundary.
                    log::debug!("download cache cleanup removed artifact");
                }
                Err(err) => {
                    summary.failed += 1;
                    log::debug!("download cache cleanup failed error={err}");
                }
            }
        }
    }

    log::info!(
        "staging cleanup summary scanned={} removed={} failed={}",
        summary.scanned,
        summary.removed,
        summary.failed
    );
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
    use super::{
        cleanup_stale_staging, clear_download_cache, create_unique_staging, download_cache_path,
        download_cache_root,
    };
    use crate::app::oplock::{OperationKind, OperationManager};
    use std::fs;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    };

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);
    static DOWNLOAD_CACHE_LOCK: Mutex<()> = Mutex::new(());

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
    fn download_cache_path_is_stable_per_url_and_collision_free() {
        let _guard = DOWNLOAD_CACHE_LOCK.lock().unwrap();
        // Same URL → same path across calls (so a second run resumes the .part).
        let a1 = download_cache_path("https://m.example/codex-1.zip", "codex-1.zip").unwrap();
        let a2 = download_cache_path("https://m.example/codex-1.zip", "codex-1.zip").unwrap();
        assert_eq!(a1, a2);
        // Different URL → different path (never resume onto a stale partial).
        let b = download_cache_path("https://m.example/codex-2.zip", "codex-2.zip").unwrap();
        assert_ne!(a1, b);
        // The cache dir is the dedicated `downloads` root, not an `update-*` dir.
        assert_eq!(a1.parent().unwrap(), download_cache_root());
        // A hostile file name can't escape the cache dir: every path separator
        // is neutralized, so the result is a single component inside the cache
        // root (a residual ".." with no separator around it is just text).
        let evil = download_cache_path("https://m.example/x", "../../etc/passwd").unwrap();
        assert_eq!(evil.parent().unwrap(), download_cache_root());
        let evil_name = evil.file_name().unwrap().to_str().unwrap();
        assert!(!evil_name.contains('/') && !evil_name.contains('\\'));
    }

    #[test]
    fn clear_download_cache_removes_the_root() {
        let _guard = DOWNLOAD_CACHE_LOCK.lock().unwrap();
        let p = download_cache_path("https://m.example/clear-me.zip", "clear-me.zip").unwrap();
        fs::write(&p, b"partial").unwrap();
        assert!(p.exists());
        clear_download_cache().unwrap();
        assert!(!p.exists());
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
