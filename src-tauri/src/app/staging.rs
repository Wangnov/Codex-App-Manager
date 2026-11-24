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
    crate::app::paths::staging_root()
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

/// Root of the persistent download cache — SEPARATE from per-run staging dirs
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
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
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
            let is_managed_staging_dir = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_managed_staging_dir_name);
            if !is_managed_staging_dir || !path.is_dir() {
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
                    let path_display = path.display();
                    log::debug!("download cache cleanup removed path={path_display}");
                }
                Err(err) => {
                    summary.failed += 1;
                    let path_display = path.display();
                    log::debug!("download cache cleanup failed path={path_display} error={err}");
                }
            }
        }
    }

    // Also reclaim stale Windows portable staging directories left beside
    // portable install roots (`.codex-app-manager-staging/portable-<uuid>`).
    // Extraction happens before an install transaction journal exists, so
    // active portable installs are protected by `ops.is_busy()` (the
    // OperationManager lease held across the entire win_perform_update /
    // win_install_historical_release lifecycle) plus STALE_AFTER age and
    // `install_tx::path_is_protected` for pending/NeedsManual recovery txs.
    for staging_dir in portable_staging_dirs() {
        cleanup_portable_staging_in_dir(&staging_dir, ops, &protected, now, &mut summary);
        if summary.skipped_busy {
            break;
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

fn is_managed_staging_dir_name(name: &str) -> bool {
    name.starts_with("update-") || name.starts_with("inspect-")
}

/// Strictly match `portable-<hyphenated-uuid>` directories created by
/// `codex_win_engine::portable::install_portable_from_msix_with_launch_arguments`.
fn is_portable_staging_dir_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("portable-") else {
        return false;
    };
    rest.len() == 36
        && Uuid::parse_str(rest)
            .is_ok_and(|parsed| parsed.hyphenated().to_string() == rest.to_ascii_lowercase())
}

#[cfg(windows)]
fn is_symlink_or_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    meta.file_type().is_symlink() || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

#[cfg(not(windows))]
fn is_symlink_or_reparse_point(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

#[cfg(any(target_os = "windows", test))]
fn portable_staging_dirs_from(install_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut dirs = Vec::new();
    for root in install_roots {
        if root.as_os_str().is_empty()
            || !root.is_absolute()
            || codex_win_engine::sys::is_msix_package_path(root)
        {
            continue;
        }
        let Some(parent) = root.parent().filter(|p| !p.as_os_str().is_empty()) else {
            continue;
        };
        let candidate = parent.join(codex_win_engine::PORTABLE_STAGING_DIR_NAME);
        let dedup_key = if cfg!(windows) {
            candidate.to_string_lossy().to_lowercase()
        } else {
            candidate.to_string_lossy().into_owned()
        };
        if seen.insert(dedup_key) {
            dirs.push(candidate);
        }
    }
    dirs
}

#[cfg(target_os = "windows")]
fn portable_staging_dirs() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let settings = crate::app::settings_store::AppSettings::load();
    if !settings.install_root.trim().is_empty() {
        roots.push(PathBuf::from(&settings.install_root));
    }
    let provenance = crate::app::provenance::ProvenanceStore::load();
    for record in provenance.managed {
        if !record.path.trim().is_empty() {
            roots.push(PathBuf::from(record.path));
        }
    }
    // Also cover the default portable location (`%LOCALAPPDATA%\Programs\Codex`)
    // in case the user changed `install_root` after a previous failed install,
    // except during packaged smoke tests which must stay isolated.
    if crate::app::paths::packaged_smoke_run_id().is_none() {
        let default_root = crate::app::settings_store::default_install_root();
        if !default_root.trim().is_empty() {
            roots.push(PathBuf::from(default_root));
        }
    }
    portable_staging_dirs_from(&roots)
}

#[cfg(not(target_os = "windows"))]
fn portable_staging_dirs() -> Vec<PathBuf> {
    Vec::new()
}

fn is_portable_staging_stale(path: &Path, now: SystemTime) -> bool {
    let Ok(root_meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    let Ok(mut latest) = root_meta.modified() else {
        return false;
    };
    for child in ["extracted", "payload"] {
        if let Ok(meta) = std::fs::symlink_metadata(path.join(child)) {
            if let Ok(modified) = meta.modified() {
                if modified > latest {
                    latest = modified;
                }
            }
        }
    }
    now.duration_since(latest)
        .map(|age| age >= STALE_AFTER)
        .unwrap_or(false)
}

fn cleanup_portable_staging_in_dir(
    staging_dir: &Path,
    ops: &OperationManager,
    protected: &[PathBuf],
    now: SystemTime,
    summary: &mut CleanupSummary,
) {
    if ops.is_busy() {
        summary.skipped_busy = true;
        return;
    }
    let Ok(dir_meta) = std::fs::symlink_metadata(staging_dir) else {
        return;
    };
    if !dir_meta.is_dir() || is_symlink_or_reparse_point(&dir_meta) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(staging_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if ops.is_busy() {
            summary.skipped_busy = true;
            return;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_dir() || is_symlink_or_reparse_point(&meta) {
            continue;
        }
        let is_portable_dir = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_portable_staging_dir_name);
        if !is_portable_dir {
            continue;
        }
        summary.scanned += 1;
        if crate::app::install_tx::path_is_protected(&path, protected) {
            log::info!(
                "portable staging cleanup skipped protected path={}",
                path.display()
            );
            continue;
        }
        if !is_portable_staging_stale(&path, now) {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                summary.removed += 1;
                log::debug!(
                    "portable staging cleanup removed path={}",
                    path.display()
                );
            }
            Err(err) => {
                summary.failed += 1;
                log::debug!(
                    "portable staging cleanup failed path={} error={err}",
                    path.display()
                );
            }
        }
    }
    // Remove the `.codex-app-manager-staging` container ONLY when it is empty
    // and not protected by any pending install transaction.
    if !crate::app::install_tx::path_is_protected(staging_dir, protected) {
        let _ = std::fs::remove_dir(staging_dir);
    }
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
        cleanup_portable_staging_in_dir, cleanup_stale_staging, clear_download_cache,
        create_unique_staging, download_cache_path, download_cache_root,
        is_managed_staging_dir_name, is_portable_staging_dir_name, portable_staging_dirs_from,
        CleanupSummary, STALE_AFTER,
    };
    use crate::app::oplock::{OperationKind, OperationManager};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    };
    use std::time::{Duration, SystemTime};
    use uuid::Uuid;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);
    static DOWNLOAD_CACHE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn cleanup_recognizes_update_and_inspection_staging_dirs_only() {
        assert!(is_managed_staging_dir_name("update-123"));
        assert!(is_managed_staging_dir_name("inspect-123"));
        assert!(!is_managed_staging_dir_name("downloads"));
        assert!(!is_managed_staging_dir_name("other-123"));
    }

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

    #[test]
    fn portable_staging_name_requires_strict_hyphenated_uuid() {
        let id = Uuid::new_v4();
        assert!(is_portable_staging_dir_name(&format!("portable-{id}")));
        assert!(is_portable_staging_dir_name(&format!(
            "portable-{}",
            id.hyphenated().to_string().to_uppercase()
        )));
        for invalid in [
            "",
            "portable-",
            "portable-123",
            "portable-custom",
            &format!("portable-{}", id.simple()),
            &format!("portable-{{{id}}}"),
            &format!("update-{id}"),
            "other",
        ] {
            assert!(!is_portable_staging_dir_name(invalid), "{invalid:?}");
        }
    }

    #[test]
    fn portable_staging_dirs_from_dedups_and_skips_windowsapps() {
        let base = lock_path("portable-roots").parent().unwrap().to_path_buf();
        let root_a = base.join("Programs").join("Codex");
        let root_b = base.join("Programs").join("CodexAlt");
        let root_c = base.join("Custom").join("Codex");
        let msix_root = base.join("WindowsApps").join("OpenAI.Codex");

        let dirs = portable_staging_dirs_from(&[
            root_a,
            root_b,
            root_c,
            msix_root,
            PathBuf::from("relative/Codex"),
        ]);
        assert_eq!(
            dirs,
            vec![
                base.join("Programs")
                    .join(codex_win_engine::PORTABLE_STAGING_DIR_NAME),
                base.join("Custom")
                    .join(codex_win_engine::PORTABLE_STAGING_DIR_NAME),
            ]
        );
    }

    #[test]
    fn portable_staging_cleanup_removes_stale_uuid_and_empty_container() {
        let test_dir = lock_path("portable-stale").parent().unwrap().to_path_buf();
        let staging_dir = test_dir.join(codex_win_engine::PORTABLE_STAGING_DIR_NAME);
        let stale_run = staging_dir.join(format!("portable-{}", Uuid::new_v4()));
        fs::create_dir_all(stale_run.join("extracted")).unwrap();
        fs::write(stale_run.join("extracted/huge.bin"), b"payload").unwrap();

        let manager = OperationManager::new(lock_path("portable-stale-op"));
        let mut summary = CleanupSummary::default();
        let future_now = SystemTime::now() + STALE_AFTER + Duration::from_secs(60);
        cleanup_portable_staging_in_dir(&staging_dir, &manager, &[], future_now, &mut summary);

        assert!(!summary.skipped_busy);
        assert_eq!(summary.scanned, 1);
        assert_eq!(summary.removed, 1);
        assert_eq!(summary.failed, 0);
        assert!(!stale_run.exists());
        assert!(
            !staging_dir.exists(),
            "empty .codex-app-manager-staging container must be removed"
        );
        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn portable_staging_cleanup_skips_fresh_protected_non_uuid_and_busy() {
        let test_dir = lock_path("portable-guards").parent().unwrap().to_path_buf();
        let staging_dir = test_dir.join(codex_win_engine::PORTABLE_STAGING_DIR_NAME);
        let stale_removable = staging_dir.join(format!("portable-{}", Uuid::new_v4()));
        let stale_protected = staging_dir.join(format!("portable-{}", Uuid::new_v4()));
        let non_uuid = staging_dir.join("portable-custom-keep");
        for dir in [&stale_removable, &stale_protected, &non_uuid] {
            fs::create_dir_all(dir).unwrap();
        }

        let protected = vec![stale_protected.clone(), stale_protected.join("payload")];
        let manager = OperationManager::new(lock_path("portable-guards-op"));
        let future_now = SystemTime::now() + STALE_AFTER + Duration::from_secs(60);

        // 1. Busy manager skips everything immediately.
        {
            let _guard = manager.begin(OperationKind::Install).unwrap();
            let mut busy_summary = CleanupSummary::default();
            cleanup_portable_staging_in_dir(
                &staging_dir,
                &manager,
                &protected,
                future_now,
                &mut busy_summary,
            );
            assert!(busy_summary.skipped_busy);
            assert_eq!(busy_summary.removed, 0);
            assert!(stale_removable.exists());
        }

        // 2. Fresh timestamp skips removal of all entries.
        let mut fresh_summary = CleanupSummary::default();
        cleanup_portable_staging_in_dir(
            &staging_dir,
            &manager,
            &protected,
            SystemTime::now(),
            &mut fresh_summary,
        );
        assert_eq!(fresh_summary.scanned, 2);
        assert_eq!(fresh_summary.removed, 0);
        assert!(stale_removable.exists());
        assert!(stale_protected.exists());

        // 3. Stale timestamp removes ONLY the unprotected strict-UUID run;
        //    protected run, non-UUID dir, and non-empty container survive.
        let mut stale_summary = CleanupSummary::default();
        cleanup_portable_staging_in_dir(
            &staging_dir,
            &manager,
            &protected,
            future_now,
            &mut stale_summary,
        );
        assert_eq!(stale_summary.scanned, 2);
        assert_eq!(stale_summary.removed, 1);
        assert_eq!(stale_summary.failed, 0);
        assert!(!stale_removable.exists());
        assert!(stale_protected.exists());
        assert!(non_uuid.exists());
        assert!(staging_dir.exists());

        let _ = fs::remove_dir_all(test_dir);
    }

    #[cfg(unix)]
    #[test]
    fn portable_staging_cleanup_never_follows_symlinks() {
        use std::os::unix::fs::symlink;

        let test_dir = lock_path("portable-symlink").parent().unwrap().to_path_buf();
        let external = test_dir.join("external-target");
        let staging_dir = test_dir.join(codex_win_engine::PORTABLE_STAGING_DIR_NAME);
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("keep.txt"), b"must stay").unwrap();
        fs::create_dir_all(&staging_dir).unwrap();

        let symlink_run = staging_dir.join(format!("portable-{}", Uuid::new_v4()));
        symlink(&external, &symlink_run).unwrap();

        let manager = OperationManager::new(lock_path("portable-symlink-op"));
        let mut summary = CleanupSummary::default();
        let future_now = SystemTime::now() + STALE_AFTER + Duration::from_secs(60);
        cleanup_portable_staging_in_dir(&staging_dir, &manager, &[], future_now, &mut summary);

        assert_eq!(summary.scanned, 0);
        assert_eq!(summary.removed, 0);
        assert!(fs::symlink_metadata(&symlink_run).is_ok());
        assert_eq!(fs::read(external.join("keep.txt")).unwrap(), b"must stay");

        let _ = fs::remove_dir_all(test_dir);
    }
}
