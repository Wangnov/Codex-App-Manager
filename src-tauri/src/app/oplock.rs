use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::{FileExt as Fs4FileExt, TryLockError};

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);
const DEFAULT_STALE_AFTER_SECS: u64 = 5 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    Install,
    Update,
    Uninstall,
    SetInstallRoot,
    Adopt,
}

impl OperationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Uninstall => "uninstall",
            Self::SetInstallRoot => "set-install-root",
            Self::Adopt => "adopt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OperationToken(pub String);

#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    #[error("已有操作正在进行（{0}），请等待完成后再试")]
    BusySameProcess(&'static str),
    #[error("另一个 Codex 管理器实例正在执行操作，请关闭多余窗口后重试")]
    BusyOtherProcess,
    #[error("操作令牌无效或已过期，请重新发起操作")]
    InvalidToken,
    #[error("无法获取操作锁：{0}")]
    Lock(String),
}

#[derive(Clone)]
pub struct OperationManager {
    inner: Arc<Mutex<Inner>>,
    stale_after_secs: u64,
}

struct Inner {
    active: Option<ActiveOp>,
    lock_file: Result<File, String>,
}

struct ActiveOp {
    token: String,
    kind: OperationKind,
    started_unix: u64,
    /// Detached tokens start unclaimed; the first successful `validate` claims them.
    /// Claimed leases are not subject to wall-clock stale reclaim.
    detached: bool,
    claimed: bool,
    /// Number of live `validate` holders (DetachedGuard instances). `end` only
    /// unlocks when the last holder releases, so concurrent guards cannot free
    /// the lock while another worker still thinks it owns the lease.
    holders: u32,
}

#[must_use = "持有 guard 才代表持有操作锁；提前 drop 会立即释放锁"]
pub struct OperationGuard {
    manager: Arc<Mutex<Inner>>,
    token: OperationToken,
    kind: OperationKind,
}

impl OperationGuard {
    pub fn token(&self) -> &OperationToken {
        &self.token
    }

    pub fn kind(&self) -> OperationKind {
        self.kind
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        let Ok(mut inner) = self.manager.lock() else {
            return;
        };
        if inner
            .active
            .as_ref()
            .is_some_and(|active| active.token == self.token.0)
        {
            let _ = OperationManager::unlock_lock_file(&mut inner);
            inner.active.take();
            log::info!(
                "released operation lock kind={} token_prefix={}",
                self.kind.as_str(),
                token_prefix(&self.token.0)
            );
        }
    }
}

impl OperationManager {
    pub fn new(lock_path: PathBuf) -> Self {
        Self::new_with_stale_after(lock_path, DEFAULT_STALE_AFTER_SECS)
    }

    fn new_with_stale_after(lock_path: PathBuf, stale_after_secs: u64) -> Self {
        let lock_file = Self::open_lock_file(&lock_path);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                active: None,
                lock_file,
            })),
            stale_after_secs,
        }
    }

    pub fn begin(&self, kind: OperationKind) -> Result<OperationGuard, OperationError> {
        let token = self.begin_inner(kind, false)?;
        Ok(OperationGuard {
            manager: Arc::clone(&self.inner),
            token,
            kind,
        })
    }

    pub fn begin_detached(&self, kind: OperationKind) -> Result<OperationToken, OperationError> {
        self.begin_inner(kind, true)
    }

    pub fn end(&self, token: OperationToken) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        // Matching token: release a holder (and unlock only on the last one)
        // BEFORE stale reclaim, so an unclaimed-but-expired correct token still
        // ends cleanly instead of self-reclaiming into InvalidToken.
        if let Some(active) = inner.active.as_mut() {
            if active.token == token.0 {
                let active_kind = active.kind;
                if active.holders > 0 {
                    active.holders -= 1;
                }
                if active.holders > 0 {
                    log::debug!(
                        "released operation lease holder kind={} remaining={} token_prefix={}",
                        active_kind.as_str(),
                        active.holders,
                        token_prefix(&token.0)
                    );
                    return Ok(());
                }
                Self::unlock_lock_file(&mut inner)?;
                log::info!(
                    "ended operation lock kind={} token_prefix={}",
                    active_kind.as_str(),
                    token_prefix(&token.0)
                );
                inner.active.take();
                return Ok(());
            }
        }
        self.reclaim_stale_detached(&mut inner)?;
        log::warn!("end_operation received invalid token");
        Err(OperationError::InvalidToken)
    }

    pub fn validate(&self, token: &OperationToken) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        // Match+claim before stale reclaim so a live worker presenting a still-active
        // token is never dropped mid-validate. Abandoned unclaimed tokens are only
        // reclaimed on begin/is_busy/end (or validate of a non-matching token).
        if let Some(active) = inner.active.as_mut() {
            if active.token == token.0 {
                // First successful validate claims a detached lease so long-running
                // tasks are no longer reclaimed solely by wall-clock age.
                if active.detached && !active.claimed {
                    active.claimed = true;
                    log::info!(
                        "claimed detached operation lease kind={} token_prefix={}",
                        active.kind.as_str(),
                        token_prefix(&token.0)
                    );
                }
                active.holders = active.holders.saturating_add(1);
                return Ok(());
            }
        }
        self.reclaim_stale_detached(&mut inner)?;
        log::warn!("operation token validation failed");
        Err(OperationError::InvalidToken)
    }

    pub fn is_busy(&self) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if self.reclaim_stale_detached(&mut inner).is_err() {
            return true;
        }
        if inner.active.is_some() {
            return true;
        }
        let Ok(lock_file) = Self::lock_file_mut(&mut inner) else {
            return false;
        };
        match Fs4FileExt::try_lock(lock_file) {
            Ok(()) => {
                let _ = Fs4FileExt::unlock(lock_file);
                false
            }
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Error(_)) => false,
        }
    }

    fn begin_inner(
        &self,
        kind: OperationKind,
        detached: bool,
    ) -> Result<OperationToken, OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        self.reclaim_stale_detached(&mut inner)?;
        if let Some(active) = inner.active.as_ref() {
            log::warn!(
                "operation lock rejected same process active_kind={} requested_kind={}",
                active.kind.as_str(),
                kind.as_str()
            );
            return Err(OperationError::BusySameProcess(active.kind.as_str()));
        }

        let started_unix = now_unix();
        let token = OperationToken(generate_token(started_unix));
        {
            let lock_file = Self::lock_file_mut(&mut inner)?;
            if let Err(err) = Self::try_lock_file(lock_file) {
                if matches!(err, OperationError::BusyOtherProcess) {
                    log::warn!(
                        "operation lock rejected other process requested_kind={}",
                        kind.as_str()
                    );
                }
                return Err(err);
            }
            let _ = write_lock_diagnostics(lock_file, kind, &token, started_unix);
        }
        // Attached guards are claimed immediately; detached tokens stay unclaimed
        // until the first successful `validate` (DetachedGuard path).
        let claimed = !detached;
        inner.active = Some(ActiveOp {
            token: token.0.clone(),
            kind,
            started_unix,
            detached,
            claimed,
            holders: 0,
        });
        log::info!(
            "acquired operation lock kind={} token_prefix={} detached={detached} claimed={claimed}",
            kind.as_str(),
            token_prefix(&token.0)
        );
        Ok(token)
    }

    fn open_lock_file(lock_path: &Path) -> Result<File, String> {
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|e| e.to_string())
    }

    fn lock_file_mut(inner: &mut Inner) -> Result<&mut File, OperationError> {
        inner
            .lock_file
            .as_mut()
            .map_err(|err| OperationError::Lock(err.clone()))
    }

    fn try_lock_file(file: &File) -> Result<(), OperationError> {
        match Fs4FileExt::try_lock(file) {
            Ok(()) => Ok(()),
            Err(TryLockError::WouldBlock) => Err(OperationError::BusyOtherProcess),
            Err(TryLockError::Error(err)) => Err(OperationError::Lock(err.to_string())),
        }
    }

    fn unlock_lock_file(inner: &mut Inner) -> Result<(), OperationError> {
        let file = Self::lock_file_mut(inner)?;
        Fs4FileExt::unlock(file).map_err(|err| OperationError::Lock(err.to_string()))
    }

    fn reclaim_stale_detached(&self, inner: &mut Inner) -> Result<(), OperationError> {
        if let Some(active) = self.stale_unclaimed_detached(inner) {
            let age_secs = now_unix().saturating_sub(active.started_unix);
            log::info!(
                "reclaiming stale unclaimed detached operation kind={} age_secs={age_secs}",
                active.kind.as_str()
            );
            Self::unlock_lock_file(inner)?;
            inner.active.take();
        }
        Ok(())
    }

    /// Only unclaimed detached tokens expire by wall-clock age.
    /// Claimed leases remain valid for the full task lifetime until `end`/Drop.
    fn stale_unclaimed_detached<'a>(&self, inner: &'a Inner) -> Option<&'a ActiveOp> {
        inner.active.as_ref().filter(|active| {
            active.detached
                && !active.claimed
                && now_unix().saturating_sub(active.started_unix) >= self.stale_after_secs
        })
    }
}

fn token_prefix(token: &str) -> &str {
    token.get(..8).unwrap_or(token)
}

fn generate_token(started_unix: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{:x}-{:x}-{:x}",
        std::process::id(),
        nanos ^ started_unix as u128,
        counter
    )
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_lock_diagnostics(
    file: &mut File,
    kind: OperationKind,
    token: &OperationToken,
    started_unix: u64,
) -> io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    writeln!(file, "pid={}", std::process::id())?;
    writeln!(file, "kind={}", kind.as_str())?;
    writeln!(file, "token={}", token.0)?;
    writeln!(file, "started_unix={started_unix}")?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::{OperationError, OperationKind, OperationManager, OperationToken};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn lock_path(name: &str) -> std::path::PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("oplock-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("operation.lock")
    }

    #[test]
    fn begin_validate_and_drop_release_lock() {
        let path = lock_path("basic");
        let manager = OperationManager::new(path.clone());
        let guard = manager.begin(OperationKind::Update).unwrap();
        assert!(manager.is_busy());
        assert!(manager.validate(guard.token()).is_ok());
        assert!(matches!(
            manager.validate(&OperationToken("wrong".to_string())),
            Err(OperationError::InvalidToken)
        ));
        assert!(matches!(
            manager.begin(OperationKind::Install),
            Err(OperationError::BusySameProcess("update"))
        ));

        drop(guard);
        assert!(!manager.is_busy());
        assert!(manager.begin(OperationKind::Install).is_ok());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn detached_token_must_be_ended_once() {
        let path = lock_path("detached");
        let manager = OperationManager::new(path.clone());
        let token = manager.begin_detached(OperationKind::Adopt).unwrap();

        assert!(matches!(
            manager.end(OperationToken("wrong".to_string())),
            Err(OperationError::InvalidToken)
        ));
        assert!(manager.is_busy());
        manager.end(token.clone()).unwrap();
        assert!(!manager.is_busy());
        assert!(matches!(
            manager.end(token),
            Err(OperationError::InvalidToken)
        ));

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn unclaimed_detached_past_stale_can_be_reclaimed() {
        let path = lock_path("unclaimed-timeout");
        let manager = OperationManager::new_with_stale_after(path.clone(), 0);
        let token = manager.begin_detached(OperationKind::Update).unwrap();

        // Unclaimed + past stale: reclaim allows a new begin.
        let guard = manager.begin(OperationKind::Install).unwrap();
        assert_eq!(guard.kind(), OperationKind::Install);
        // Original unclaimed token is gone after reclaim.
        assert!(matches!(
            manager.validate(&token),
            Err(OperationError::InvalidToken)
        ));
        assert!(matches!(
            manager.end(token),
            Err(OperationError::InvalidToken)
        ));

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn claimed_detached_past_stale_is_not_reclaimed() {
        let path = lock_path("claimed-no-timeout");
        let manager = OperationManager::new_with_stale_after(path.clone(), 0);
        let token = manager.begin_detached(OperationKind::Update).unwrap();

        // Claim via validate before any reclaim path runs with a zero threshold.
        manager.validate(&token).unwrap();

        // Past wall-clock stale threshold, but claimed → still busy / blocked.
        assert!(manager.is_busy());
        assert!(matches!(
            manager.begin(OperationKind::Install),
            Err(OperationError::BusySameProcess("update"))
        ));
        assert!(matches!(
            manager.begin_detached(OperationKind::Uninstall),
            Err(OperationError::BusySameProcess("update"))
        ));
        // Lease remains valid under the original token.
        manager.validate(&token).unwrap();

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn validate_claims_detached_and_rejects_wrong_token() {
        let path = lock_path("claim-validate");
        // Non-zero stale so a wrong-token validate does not reclaim the unclaimed op.
        let manager = OperationManager::new_with_stale_after(path.clone(), 60);
        let token = manager.begin_detached(OperationKind::Adopt).unwrap();

        assert!(matches!(
            manager.validate(&OperationToken("wrong".to_string())),
            Err(OperationError::InvalidToken)
        ));
        // Wrong token must not claim or clear the unclaimed op.
        assert!(manager.is_busy());

        manager.validate(&token).unwrap();
        // Second validate adds another holder (concurrent DetachedGuard) — still busy.
        manager.validate(&token).unwrap();

        assert!(matches!(
            manager.begin(OperationKind::Install),
            Err(OperationError::BusySameProcess("adopt"))
        ));
        // First end only drops one holder; lock stays until the last end.
        manager.end(token.clone()).unwrap();
        assert!(manager.is_busy());
        manager.end(token).unwrap();
        assert!(!manager.is_busy());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn end_releases_claimed_detached_lease() {
        let path = lock_path("end-claimed");
        let manager = OperationManager::new_with_stale_after(path.clone(), 0);
        let token = manager.begin_detached(OperationKind::Install).unwrap();
        manager.validate(&token).unwrap();

        manager.end(token.clone()).unwrap();
        assert!(!manager.is_busy());
        assert!(matches!(
            manager.validate(&token),
            Err(OperationError::InvalidToken)
        ));
        assert!(manager.begin(OperationKind::Update).is_ok());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn drop_path_releases_claimed_detached_via_end() {
        // Mirrors DetachedGuard: validate claims, Drop ends.
        let path = lock_path("drop-claimed");
        let manager = OperationManager::new_with_stale_after(path.clone(), 0);
        let token = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&token).unwrap();

        // Simulate DetachedGuard Drop.
        manager.end(token).unwrap();
        assert!(!manager.is_busy());
        let next = manager.begin_detached(OperationKind::Install).unwrap();
        assert!(manager.validate(&next).is_ok());
        manager.end(next).unwrap();

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn concurrent_begin_while_claimed_is_blocked() {
        let path = lock_path("concurrent-claimed");
        let manager = OperationManager::new(path.clone());
        let token = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&token).unwrap();

        assert!(matches!(
            manager.begin(OperationKind::Install),
            Err(OperationError::BusySameProcess("update"))
        ));
        assert!(matches!(
            manager.begin_detached(OperationKind::Uninstall),
            Err(OperationError::BusySameProcess("update"))
        ));

        manager.end(token).unwrap();
        assert!(manager.begin(OperationKind::Install).is_ok());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn second_manager_hits_cross_process_lock() {
        let path = lock_path("cross-process");
        let first = OperationManager::new(path.clone());
        let _guard = first.begin(OperationKind::Update).unwrap();
        let second = OperationManager::new(path.clone());

        assert!(second.is_busy());
        assert!(matches!(
            second.begin(OperationKind::Install),
            Err(OperationError::BusyOtherProcess)
        ));

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
