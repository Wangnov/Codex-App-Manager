use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;

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
    detached: bool,
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
        self.reclaim_stale_detached(&mut inner)?;
        let Some(active) = inner.active.as_ref() else {
            return Err(OperationError::InvalidToken);
        };
        if active.token != token.0 {
            return Err(OperationError::InvalidToken);
        }
        Self::unlock_lock_file(&mut inner)?;
        inner.active.take();
        Ok(())
    }

    pub fn validate(&self, token: &OperationToken) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        self.reclaim_stale_detached(&mut inner)?;
        match inner.active.as_ref() {
            Some(active) if active.token == token.0 => Ok(()),
            _ => Err(OperationError::InvalidToken),
        }
    }

    pub fn is_busy(&self) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        let _ = self.reclaim_stale_detached(&mut inner);
        inner.active.is_some()
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
            return Err(OperationError::BusySameProcess(active.kind.as_str()));
        }

        let started_unix = now_unix();
        let token = OperationToken(generate_token(started_unix));
        {
            let lock_file = Self::lock_file_mut(&mut inner)?;
            Self::try_lock_file(lock_file)?;
            let _ = write_lock_diagnostics(lock_file, kind, &token, started_unix);
        }
        inner.active = Some(ActiveOp {
            token: token.0.clone(),
            kind,
            started_unix,
            detached,
        });
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
        match file.try_lock_exclusive() {
            Ok(true) => Ok(()),
            Ok(false) => Err(OperationError::BusyOtherProcess),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                Err(OperationError::BusyOtherProcess)
            }
            Err(err) => Err(OperationError::Lock(err.to_string())),
        }
    }

    fn unlock_lock_file(inner: &mut Inner) -> Result<(), OperationError> {
        let file = Self::lock_file_mut(inner)?;
        file.unlock()
            .map_err(|err| OperationError::Lock(err.to_string()))
    }

    fn reclaim_stale_detached(&self, inner: &mut Inner) -> Result<(), OperationError> {
        if self.has_stale_detached(inner) {
            Self::unlock_lock_file(inner)?;
            inner.active.take();
        }
        Ok(())
    }

    fn has_stale_detached(&self, inner: &Inner) -> bool {
        inner.active.as_ref().is_some_and(|active| {
            active.detached
                && now_unix().saturating_sub(active.started_unix) >= self.stale_after_secs
        })
    }
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
    fn detached_token_times_out_and_allows_new_begin() {
        let path = lock_path("timeout");
        let manager = OperationManager::new_with_stale_after(path.clone(), 0);
        let _token = manager.begin_detached(OperationKind::Update).unwrap();

        let guard = manager.begin(OperationKind::Install).unwrap();
        assert_eq!(guard.kind(), OperationKind::Install);

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn second_manager_hits_cross_process_lock() {
        let path = lock_path("cross-process");
        let first = OperationManager::new(path.clone());
        let _guard = first.begin(OperationKind::Update).unwrap();
        let second = OperationManager::new(path.clone());

        assert!(matches!(
            second.begin(OperationKind::Install),
            Err(OperationError::BusyOtherProcess)
        ));

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
