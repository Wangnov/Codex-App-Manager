use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::{FileExt as Fs4FileExt, TryLockError};

use crate::app::op_phase::{OperationPhase, QuitPolicy};
use crate::app::release_install::{ReleaseArchitecture, ReleasePackageFormat};

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);
const DEFAULT_STALE_AFTER_SECS: u64 = 5 * 60;
const COMPLETION_HISTORY_LIMIT: usize = 16;

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
    /// Last transfer that ended because the user paused it. Unlike `active`,
    /// this survives lease release so a replacement renderer cannot miss the
    /// short active-paused window between the abort signal and guard drop.
    paused: Option<OperationSnapshot>,
    completed: VecDeque<OperationCompletion>,
    lock_file: Result<File, String>,
}

/// Byte-transfer progress mirrored into the active lease so a reloaded
/// frontend can restore the progress screen without waiting for the next event.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationProgress {
    pub downloaded: u64,
    pub total: u64,
    pub source: String,
}

/// Exact historical/offline selection required to safely resume after a
/// renderer reload. It intentionally contains no URL or digest: the resumed
/// command re-resolves GitHub metadata (or re-verifies the local package) before
/// it trusts any bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalResumeContext {
    pub selection: HistoricalResumeSelection,
    pub block_updates: bool,
    /// Exact install snapshot the user confirmed before the original transfer.
    /// A renderer reload may refresh status while the download is paused; resume
    /// must still submit this frozen expectation so an out-of-band install/change
    /// is rejected instead of silently becoming the new destructive target.
    pub expectation: HistoricalResumeExpectation,
    /// One-shot portable install destination selected by the user. Windows
    /// resumes must use the same root instead of silently falling back to the
    /// persisted default.
    pub install_root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "platform",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum HistoricalResumeExpectation {
    Macos {
        current_path: Option<String>,
        current_build: Option<u64>,
    },
    Windows {
        current_path: Option<String>,
        current_version: Option<String>,
        current_source: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalResumeSelection {
    pub release_tag: String,
    pub version: String,
    pub asset_name: String,
    pub architecture: ReleaseArchitecture,
    pub format: ReleasePackageFormat,
    pub package_version: Option<String>,
    pub local_path: Option<String>,
    pub local_file_name: Option<String>,
}

/// Resume metadata for the ordinary latest-channel flow. Windows uses one
/// backend `Update` lease for both first install and in-place update, so the
/// lease kind alone cannot reconstruct the renderer mode or a one-shot portable
/// destination after reload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationResumeKind {
    Install,
    Perform,
}

/// Exact ordinary latest-channel target the user started downloading. A later
/// check may observe a newer release, but resume must keep submitting this
/// frozen expectation so the backend either continues the confirmed target or
/// rejects it as stale instead of silently switching releases.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "platform",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum OperationResumeExpectation {
    Macos {
        current_build: Option<u64>,
        target_build: u64,
        install_path: Option<String>,
        current_version: Option<String>,
        target_version: String,
    },
    Windows {
        current_version: Option<String>,
        target_version: String,
        package_moniker: String,
        route: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationResumeContext {
    pub kind: OperationResumeKind,
    pub install_root: Option<String>,
    pub expectation: OperationResumeExpectation,
}

/// Public view of the currently held same-process operation, if any.
/// Queried on frontend mount so a renderer reload can reattach to in-flight work.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationSnapshot {
    /// Operation token id (same string the destructive commands use).
    pub id: String,
    pub kind: OperationKind,
    pub phase: OperationPhase,
    pub progress: Option<OperationProgress>,
    pub paused: bool,
    /// Present only for historical/offline installs. A reattached paused screen
    /// must use this exact target instead of falling back to the latest channel.
    pub historical: Option<HistoricalResumeContext>,
    /// Present for ordinary flows whose UI intent cannot be recovered from the
    /// operation lease kind alone (notably Windows first installs).
    pub resume: Option<OperationResumeContext>,
    /// Whether cancel is a meaningful UI action right now.
    pub cancellable: bool,
    /// Whether the phase may be interrupted (pause/cancel/quit-after-cancel).
    pub interruptible: bool,
}

/// Terminal backend evidence retained across renderer reloads. The frontend
/// may release a fresh-install safety guard only for `FailedBeforeCommit` or a
/// positively proven `RolledBack`; once an unresolved mutation is observed (or
/// an ambiguous platform call starts), an invoke rejection cannot prove disk state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationCompletionState {
    Succeeded,
    FailedBeforeCommit,
    RolledBack,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationCompletion {
    pub id: String,
    pub kind: OperationKind,
    pub phase: OperationPhase,
    pub state: OperationCompletionState,
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
    /// Progress through the op lifecycle; drives quit policy.
    phase: OperationPhase,
    /// True once the operation has definitely changed installed app state.
    /// Completion classification must use this evidence, not the quit phase:
    /// `Committing` may be published before the first write so closing the app
    /// is blocked across the final pre-write crash window.
    mutation_started: bool,
    /// True only after a mutation was observed and the engine subsequently
    /// proved that the install root was restored to its pre-operation state.
    mutation_rolled_back: bool,
    /// True once a platform call has begun whose failure cannot prove that the
    /// installed app is unchanged (for example Add-AppxPackage).
    outcome_ambiguous: bool,
    /// Last reported download progress (if any).
    progress: Option<OperationProgress>,
    /// True after a pause was requested while the lease is still held.
    paused: bool,
    /// Sticky pause intent for this operation. A downloader can publish one last
    /// progress sample after the pause signal but before observing its cancel
    /// flag; that sample must not erase the terminal resumable snapshot.
    pause_requested: bool,
    /// Set only after the worker returns the downloader's explicit cancellation
    /// marker. `pause_active_download` can win a narrow race after curl has exited
    /// but before its guard drops; request acceptance alone therefore cannot prove
    /// the operation actually terminated because of the pause.
    pause_confirmed: bool,
    /// This operation reuses the exact historical or ordinary target retained
    /// by the last confirmed pause. Progress alone cannot prove the resumed
    /// install will finish, so keep that old context until a terminal outcome is
    /// known.
    resumes_retained_pause: bool,
    historical: Option<HistoricalResumeContext>,
    resume: Option<OperationResumeContext>,
}

fn snapshot_from_active(active: &ActiveOp) -> OperationSnapshot {
    let interruptible = active.phase.interruptible();
    OperationSnapshot {
        id: active.token.clone(),
        kind: active.kind,
        phase: active.phase,
        progress: active.progress.clone(),
        paused: active.paused,
        historical: active.historical.clone(),
        resume: active.resume.clone(),
        cancellable: interruptible,
        interruptible,
    }
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
            // Linearize latch cleanup with removal of the owning lease. A cancel
            // command takes the same mutex for both snapshots, so it either sees
            // this owner and is followed by this cleanup, or sees no owner and
            // immediately rolls its speculative latch back.
            clear_cancel_latches();
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
                paused: None,
                completed: VecDeque::new(),
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
                clear_cancel_latches();
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
        self.validate_inner(token, None)
    }

    /// Claim a detached token and publish its first phase in one critical section.
    /// Destructive operations use this to eliminate the validate→commit window.
    pub fn validate_with_phase(
        &self,
        token: &OperationToken,
        phase: OperationPhase,
    ) -> Result<(), OperationError> {
        self.validate_inner(token, Some(phase))
    }

    fn validate_inner(
        &self,
        token: &OperationToken,
        phase: Option<OperationPhase>,
    ) -> Result<(), OperationError> {
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
                if let Some(phase) = phase {
                    log::info!(
                        "claimed operation at phase kind={} phase={} token_prefix={}",
                        active.kind.as_str(),
                        phase.as_str(),
                        token_prefix(&token.0)
                    );
                    active.phase = phase;
                }
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

    /// Current phase of the active same-process op, or `Idle` when free.
    pub fn phase(&self) -> OperationPhase {
        let Ok(inner) = self.inner.lock() else {
            return OperationPhase::Idle;
        };
        inner
            .active
            .as_ref()
            .map(|active| active.phase)
            .unwrap_or(OperationPhase::Idle)
    }

    /// Kind of the active same-process op, if any.
    pub fn active_kind(&self) -> Option<OperationKind> {
        let Ok(inner) = self.inner.lock() else {
            return None;
        };
        inner.active.as_ref().map(|active| active.kind)
    }

    /// Advance the phase for a validated token. No-op-safe if the token is gone.
    pub fn set_phase(
        &self,
        token: &OperationToken,
        phase: OperationPhase,
    ) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        let Some(active) = inner.active.as_mut() else {
            return Err(OperationError::InvalidToken);
        };
        if active.token != token.0 {
            return Err(OperationError::InvalidToken);
        }
        if active.phase != phase {
            log::info!(
                "operation phase kind={} from={} to={} token_prefix={}",
                active.kind.as_str(),
                active.phase.as_str(),
                phase.as_str(),
                token_prefix(&token.0)
            );
            active.phase = phase;
        }
        Ok(())
    }

    /// Record positive evidence that installed app state has changed.
    pub fn mark_mutation_started(&self, token: &OperationToken) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        let Some(active) = inner.active.as_mut() else {
            return Err(OperationError::InvalidToken);
        };
        if active.token != token.0 {
            return Err(OperationError::InvalidToken);
        }
        if !active.mutation_started {
            log::info!(
                "operation mutation started kind={} token_prefix={}",
                active.kind.as_str(),
                token_prefix(&token.0)
            );
            active.mutation_started = true;
        }
        // Any later mutation invalidates rollback evidence from an earlier
        // attempt in the same operation.
        active.mutation_rolled_back = false;
        Ok(())
    }

    /// Record positive evidence that every observed portable install-root
    /// mutation was restored. This does not clear `outcome_ambiguous`: a prior
    /// platform call such as Add-AppxPackage may still have changed system state.
    pub fn mark_mutation_rolled_back(&self, token: &OperationToken) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        let Some(active) = inner.active.as_mut() else {
            return Err(OperationError::InvalidToken);
        };
        if active.token != token.0 {
            return Err(OperationError::InvalidToken);
        }
        // Some rollback callbacks also clear a Prepared journal before the
        // first rename. They are safe failures, but not "rolled back" unless
        // this operation actually recorded a mutation first.
        if active.mutation_started {
            log::info!(
                "operation mutation rolled back kind={} token_prefix={}",
                active.kind.as_str(),
                token_prefix(&token.0)
            );
            active.mutation_rolled_back = true;
        }
        Ok(())
    }

    /// Record that a platform mutation call has started and its eventual error
    /// cannot establish the pre-call disk state.
    pub fn mark_outcome_ambiguous(&self, token: &OperationToken) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        let Some(active) = inner.active.as_mut() else {
            return Err(OperationError::InvalidToken);
        };
        if active.token != token.0 {
            return Err(OperationError::InvalidToken);
        }
        if !active.outcome_ambiguous {
            log::info!(
                "operation outcome became ambiguous kind={} token_prefix={}",
                active.kind.as_str(),
                token_prefix(&token.0)
            );
            active.outcome_ambiguous = true;
        }
        Ok(())
    }

    /// Unified quit/close policy for window close, menu quit, and quit command.
    ///
    /// Reads busy/phase/kind under a **single** mutex hold so a concurrent
    /// `end`/`set_phase` cannot TOCTOU the snapshot between separate queries.
    pub fn quit_policy(&self, force_quit: bool, confirm_close: bool) -> QuitPolicy {
        if force_quit {
            return QuitPolicy::Allow;
        }
        let Ok(mut inner) = self.inner.lock() else {
            // Poisoned mutex: refuse exit rather than risk killing mid-swap.
            return QuitPolicy::evaluate(
                false,
                confirm_close,
                true,
                OperationPhase::Finishing,
                None,
            );
        };
        // Reclaim abandoned unclaimed tokens before deciding.
        let _ = self.reclaim_stale_detached(&mut inner);

        let (busy, phase, kind) = if let Some(active) = inner.active.as_ref() {
            (true, active.phase, Some(active.kind))
        } else {
            // Cross-process lock without a local ActiveOp: treat as
            // non-interruptible finishing so we never kill another instance mid-swap.
            let other_busy = match Self::lock_file_mut(&mut inner) {
                Ok(lock_file) => match Fs4FileExt::try_lock(lock_file) {
                    Ok(()) => {
                        let _ = Fs4FileExt::unlock(lock_file);
                        false
                    }
                    Err(TryLockError::WouldBlock) => true,
                    Err(TryLockError::Error(_)) => false,
                },
                Err(_) => false,
            };
            if other_busy {
                (true, OperationPhase::Finishing, None)
            } else {
                (false, OperationPhase::Idle, None)
            }
        };
        QuitPolicy::evaluate(force_quit, confirm_close, busy, phase, kind)
    }

    /// Linearize an allowed or user-confirmed quit with the active phase.
    ///
    /// `prepare_exit` runs while the operation mutex is held when policy is
    /// already `Allow`, or after an explicit confirmation. Callers use it to arm
    /// platform cancellation latches and the force-exit flag before a worker can
    /// advance to `Committing`. A worker that won the mutex first is observed as
    /// blocked; a worker that advances afterward observes the final checkpoint.
    pub fn prepare_quit(
        &self,
        confirm_close: bool,
        confirmed: bool,
        prepare_exit: impl FnOnce(),
    ) -> QuitPolicy {
        let Ok(mut inner) = self.inner.lock() else {
            return QuitPolicy::evaluate(
                false,
                confirm_close,
                true,
                OperationPhase::Finishing,
                None,
            );
        };
        let _ = self.reclaim_stale_detached(&mut inner);

        let (busy, phase, kind) = if let Some(active) = inner.active.as_ref() {
            (true, active.phase, Some(active.kind))
        } else {
            let other_busy = match Self::lock_file_mut(&mut inner) {
                Ok(lock_file) => match Fs4FileExt::try_lock(lock_file) {
                    Ok(()) => {
                        let _ = Fs4FileExt::unlock(lock_file);
                        false
                    }
                    Err(TryLockError::WouldBlock) => true,
                    Err(TryLockError::Error(_)) => false,
                },
                Err(_) => false,
            };
            if other_busy {
                (true, OperationPhase::Finishing, None)
            } else {
                (false, OperationPhase::Idle, None)
            }
        };
        let policy = QuitPolicy::evaluate(false, confirm_close, busy, phase, kind);
        let should_prepare = matches!(policy, QuitPolicy::Allow)
            || (confirmed && matches!(policy, QuitPolicy::Confirm));
        if should_prepare {
            // An armed detached token has not entered its command yet. Remove it
            // under this same mutex so a delayed validate cannot start destructive
            // work after force-exit preparation. Claimed workers keep their lease
            // and must honor the phase-linearized abort checkpoint instead.
            let abandon_unclaimed = inner
                .active
                .as_ref()
                .is_some_and(|active| active.detached && !active.claimed);
            if abandon_unclaimed {
                let _ = Self::unlock_lock_file(&mut inner);
                inner.active.take();
            }
            prepare_exit();
        }
        policy
    }

    /// Snapshot of the local active operation, for frontend reattach after
    /// renderer reload / remount. `None` when free (or only a cross-process lock).
    pub fn snapshot(&self) -> Option<OperationSnapshot> {
        let Ok(inner) = self.inner.lock() else {
            return None;
        };
        inner.active.as_ref().map(snapshot_from_active)
    }

    /// Last completed pause, retained until bytes flow again, the user
    /// explicitly discards/cancels it, or a later operation succeeds.
    pub fn paused_snapshot(&self) -> Option<OperationSnapshot> {
        let Ok(inner) = self.inner.lock() else {
            return None;
        };
        if inner.active.is_some() {
            return None;
        }
        inner.paused.clone()
    }

    pub fn clear_paused_snapshot(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.paused = None;
        }
    }

    /// Run a platform cancel signal while the active lease is locked and still
    /// known to be interruptible. This is the linearization point between an
    /// operation ending and a new one beginning: a process-global engine latch
    /// can never be armed for operation A after operation B has taken its lease.
    pub fn request_cancellation(
        &self,
        token: &OperationToken,
        request: impl FnOnce() -> bool,
    ) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if !inner
            .active
            .as_ref()
            .is_some_and(|active| active.token == token.0 && active.phase.interruptible())
        {
            return false;
        }
        let requested = request();
        if requested {
            if let Some(active) = inner.active.as_mut() {
                active.paused = false;
                active.pause_requested = false;
                active.pause_confirmed = false;
            }
            inner.paused = None;
        }
        requested
    }

    /// Signal pause and mark the same active lease while holding the operation
    /// mutex. Without this critical section a late pause for operation A could
    /// observe the process-global downloader of newly-started operation B.
    pub fn request_pause(&self, token: &OperationToken, request: impl FnOnce() -> bool) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        let Some(active) = inner.active.as_mut() else {
            return false;
        };
        if active.token != token.0 || !active.phase.interruptible() {
            return false;
        }
        let requested = request();
        if requested {
            active.paused = true;
            active.pause_requested = true;
            active.pause_confirmed = false;
        }
        requested
    }

    /// Confirm that the worker ended with the downloader's explicit
    /// `download cancelled` result after a pause request. Only this evidence may
    /// publish a resumable terminal snapshot.
    pub fn confirm_pause(&self, token: &OperationToken) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        let Some(active) = inner.active.as_mut() else {
            return Err(OperationError::InvalidToken);
        };
        if active.token != token.0 {
            return Err(OperationError::InvalidToken);
        }
        if active.pause_requested {
            active.pause_confirmed = true;
        }
        Ok(())
    }

    /// Record the terminal outcome before the lease is released. Keeping a
    /// small token-keyed history lets a replacement renderer distinguish a
    /// true no-mutation failure from an ambiguous platform/mutation error.
    pub fn record_completion(
        &self,
        token: &OperationToken,
        succeeded: bool,
    ) -> Result<OperationCompletion, OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        let (completion, paused) = {
            let Some(active) = inner.active.as_ref() else {
                return Err(OperationError::InvalidToken);
            };
            if active.token != token.0 {
                return Err(OperationError::InvalidToken);
            }
            let completion = OperationCompletion {
                id: active.token.clone(),
                kind: active.kind,
                phase: active.phase,
                state: if succeeded {
                    OperationCompletionState::Succeeded
                } else if active.outcome_ambiguous {
                    OperationCompletionState::OutcomeUnknown
                } else if active.mutation_started && active.mutation_rolled_back {
                    OperationCompletionState::RolledBack
                } else if active.mutation_started {
                    OperationCompletionState::OutcomeUnknown
                } else {
                    OperationCompletionState::FailedBeforeCommit
                },
            };
            let paused = (!succeeded && active.pause_requested && active.pause_confirmed)
                .then(|| snapshot_from_active(active));
            (completion, paused)
        };
        if let Some(paused) = paused {
            inner.paused = Some(paused);
        } else if matches!(
            completion.state,
            OperationCompletionState::Succeeded | OperationCompletionState::OutcomeUnknown
        ) {
            // Success consumes the retained target. OutcomeUnknown may have
            // changed the installed app, so offering the old pause again would
            // permit a destructive retry against an unproven expectation.
            inner.paused = None;
        }
        inner.completed.push_back(completion.clone());
        while inner.completed.len() > COMPLETION_HISTORY_LIMIT {
            inner.completed.pop_front();
        }
        Ok(completion)
    }

    pub fn completion(&self, token: &OperationToken) -> Option<OperationCompletion> {
        let Ok(inner) = self.inner.lock() else {
            return None;
        };
        inner
            .completed
            .iter()
            .rev()
            .find(|completion| completion.id == token.0)
            .cloned()
    }

    /// Record the latest download progress for a validated token.
    pub fn set_progress(
        &self,
        token: &OperationToken,
        progress: OperationProgress,
    ) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        let Some(active) = inner.active.as_mut() else {
            return Err(OperationError::InvalidToken);
        };
        if active.token != token.0 {
            return Err(OperationError::InvalidToken);
        }
        // For the SAME operation, curl may report one final progress sample
        // after pause was requested; keep the sticky intent so completion still
        // publishes a resumable snapshot. An exact historical resume also keeps
        // the prior terminal snapshot: receiving bytes does not prove that the
        // later checksum/signature/install steps will finish successfully.
        if !active.pause_requested {
            active.paused = false;
        }
        active.progress = Some(progress);
        if !active.pause_requested && !active.resumes_retained_pause {
            inner.paused = None;
        }
        Ok(())
    }

    /// Mark the active op as paused (or clear the flag). Used when the UI
    /// requests pause so a reloaded frontend can restore the paused screen.
    pub fn set_paused(&self, token: &OperationToken, paused: bool) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        let Some(active) = inner.active.as_mut() else {
            return Err(OperationError::InvalidToken);
        };
        if active.token != token.0 {
            return Err(OperationError::InvalidToken);
        }
        active.paused = paused;
        active.pause_requested = paused;
        active.pause_confirmed = paused;
        Ok(())
    }

    pub fn set_historical_context(
        &self,
        token: &OperationToken,
        context: HistoricalResumeContext,
    ) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        let Some(active_kind) = inner
            .active
            .as_ref()
            .filter(|active| active.token == token.0)
            .map(|active| active.kind)
        else {
            return Err(OperationError::InvalidToken);
        };
        let resumes_retained_pause = inner.paused.as_ref().is_some_and(|paused| {
            paused.kind == active_kind && paused.historical.as_ref() == Some(&context)
        });
        let active = inner.active.as_mut().ok_or(OperationError::InvalidToken)?;
        active.resumes_retained_pause = resumes_retained_pause;
        active.historical = Some(context);
        active.resume = None;
        Ok(())
    }

    pub fn set_resume_context(
        &self,
        token: &OperationToken,
        context: OperationResumeContext,
    ) -> Result<(), OperationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| OperationError::Lock("operation mutex poisoned".to_string()))?;
        let Some(active_kind) = inner
            .active
            .as_ref()
            .filter(|active| active.token == token.0)
            .map(|active| active.kind)
        else {
            return Err(OperationError::InvalidToken);
        };
        let resumes_retained_pause = inner.paused.as_ref().is_some_and(|paused| {
            paused.kind == active_kind && paused.resume.as_ref() == Some(&context)
        });
        let active = inner.active.as_mut().ok_or(OperationError::InvalidToken)?;
        active.resumes_retained_pause = resumes_retained_pause;
        active.resume = Some(context);
        active.historical = None;
        Ok(())
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
            phase: OperationPhase::Preparing,
            mutation_started: false,
            mutation_rolled_back: false,
            outcome_ambiguous: false,
            progress: None,
            paused: false,
            pause_requested: false,
            pause_confirmed: false,
            resumes_retained_pause: false,
            historical: None,
            resume: None,
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
            clear_cancel_latches();
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

#[cfg(test)]
pub(crate) static CANCEL_LATCH_TEST_LOCK: Mutex<()> = Mutex::new(());

fn clear_cancel_latches() {
    // The abort latches are process-global. Production OperationManager access is
    // serialized by `inner`, while Rust's test harness may run unrelated managers
    // and direct latch tests concurrently in one process. Share a test-only lock
    // with those direct tests so a guard drop cannot clear their asserted value.
    #[cfg(test)]
    let _test_guard = CANCEL_LATCH_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::app::mac_update::clear_update_abort();
    crate::app::win_update::clear_win_update_abort();
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
    use super::{
        HistoricalResumeContext, HistoricalResumeExpectation, HistoricalResumeSelection,
        OperationCompletionState, OperationError, OperationKind, OperationManager, OperationToken,
    };
    use crate::app::release_install::{ReleaseArchitecture, ReleasePackageFormat};
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

    #[test]
    fn set_phase_drives_quit_policy_for_point_of_no_return() {
        use crate::app::op_phase::{OperationPhase, QuitPolicy};

        let path = lock_path("phase-quit");
        let manager = OperationManager::new(path.clone());
        let token = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&token).unwrap();

        assert!(matches!(
            manager.quit_policy(false, false),
            QuitPolicy::Allow
        ));

        manager
            .set_phase(&token, OperationPhase::Committing)
            .unwrap();
        assert!(matches!(
            manager.quit_policy(false, false),
            QuitPolicy::Block {
                phase: OperationPhase::Committing,
                ..
            }
        ));
        // Force quit still wins.
        assert_eq!(manager.quit_policy(true, true), QuitPolicy::Allow);

        manager.end(token).unwrap();
        assert_eq!(manager.phase(), OperationPhase::Idle);

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn snapshot_exposes_progress_phase_and_flags() {
        use super::OperationProgress;
        use crate::app::op_phase::OperationPhase;

        let path = lock_path("snapshot");
        let manager = OperationManager::new(path.clone());
        assert!(manager.snapshot().is_none());

        let token = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&token).unwrap();

        let snap = manager.snapshot().expect("active snapshot");
        assert_eq!(snap.id, token.0);
        assert_eq!(snap.kind, OperationKind::Update);
        assert_eq!(snap.phase, OperationPhase::Preparing);
        assert!(snap.progress.is_none());
        assert!(!snap.paused);
        assert!(snap.historical.is_none());
        assert!(snap.resume.is_none());
        assert!(snap.cancellable);
        assert!(snap.interruptible);

        manager
            .set_progress(
                &token,
                OperationProgress {
                    downloaded: 50,
                    total: 100,
                    source: "example.test".into(),
                },
            )
            .unwrap();
        manager
            .set_phase(&token, OperationPhase::Downloading)
            .unwrap();
        manager.set_paused(&token, true).unwrap();
        let historical = HistoricalResumeContext {
            selection: HistoricalResumeSelection {
                release_tag: "codex-app-26.623.101652".into(),
                version: "26.623.101652".into(),
                asset_name: "Codex-mac-arm64.dmg".into(),
                architecture: ReleaseArchitecture::Arm64,
                format: ReleasePackageFormat::Dmg,
                package_version: None,
                local_path: None,
                local_file_name: None,
            },
            block_updates: true,
            expectation: HistoricalResumeExpectation::Macos {
                current_path: Some("/Applications/Codex.app".into()),
                current_build: Some(5813),
            },
            install_root: None,
        };
        manager
            .set_historical_context(&token, historical.clone())
            .unwrap();

        let snap = manager.snapshot().expect("progress snapshot");
        assert_eq!(snap.progress.as_ref().map(|p| p.downloaded), Some(50));
        assert_eq!(snap.phase, OperationPhase::Downloading);
        assert!(snap.paused);
        assert_eq!(snap.historical, Some(historical));
        assert!(snap.cancellable);
        assert!(snap.interruptible);

        // A final progress sample from the same downloader must not erase the
        // already-accepted pause request.
        manager
            .set_progress(
                &token,
                OperationProgress {
                    downloaded: 60,
                    total: 100,
                    source: "example.test".into(),
                },
            )
            .unwrap();
        assert!(manager.snapshot().unwrap().paused);

        manager
            .set_phase(&token, OperationPhase::Committing)
            .unwrap();
        let snap = manager.snapshot().unwrap();
        assert!(!snap.cancellable);
        assert!(!snap.interruptible);

        manager.end(token).unwrap();
        assert!(manager.snapshot().is_none());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn ordinary_resume_context_survives_terminal_pause() {
        use super::{
            OperationProgress, OperationResumeContext, OperationResumeExpectation,
            OperationResumeKind,
        };
        use crate::app::op_phase::OperationPhase;

        let path = lock_path("ordinary-terminal-pause");
        let manager = OperationManager::new(path.clone());
        let token = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&token).unwrap();
        let resume = OperationResumeContext {
            kind: OperationResumeKind::Install,
            install_root: Some(r"D:\Selected\Codex".into()),
            expectation: OperationResumeExpectation::Windows {
                current_version: None,
                target_version: "2.0.0".into(),
                package_moniker: "Codex_2.0.0_x64".into(),
                route: "msix-sideload".into(),
            },
        };
        manager.set_resume_context(&token, resume.clone()).unwrap();
        manager
            .set_phase(&token, OperationPhase::Downloading)
            .unwrap();
        manager
            .set_progress(
                &token,
                OperationProgress {
                    downloaded: 64,
                    total: 128,
                    source: "github.com".into(),
                },
            )
            .unwrap();
        manager.set_paused(&token, true).unwrap();
        manager.record_completion(&token, false).unwrap();
        manager.end(token).unwrap();

        let paused = manager.paused_snapshot().expect("terminal pause");
        assert_eq!(paused.kind, OperationKind::Update);
        assert_eq!(paused.resume, Some(resume));
        assert!(paused.historical.is_none());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn paused_snapshot_survives_safe_resume_failure_until_success_or_unknown() {
        use super::OperationProgress;
        use crate::app::op_phase::OperationPhase;

        let path = lock_path("terminal-pause");
        let manager = OperationManager::new(path.clone());
        let pause_once = |manager: &OperationManager| {
            let token = manager.begin_detached(OperationKind::Install).unwrap();
            manager.validate(&token).unwrap();
            manager
                .set_phase(&token, OperationPhase::Downloading)
                .unwrap();
            manager
                .set_progress(
                    &token,
                    OperationProgress {
                        downloaded: 64,
                        total: 128,
                        source: "github.com".into(),
                    },
                )
                .unwrap();
            manager.set_paused(&token, true).unwrap();
            // The downloader can race one final size sample after accepting the
            // pause signal. Completion must still retain the resume context.
            manager
                .set_progress(
                    &token,
                    OperationProgress {
                        downloaded: 65,
                        total: 128,
                        source: "github.com".into(),
                    },
                )
                .unwrap();
            let historical = HistoricalResumeContext {
                selection: HistoricalResumeSelection {
                    release_tag: "codex-app-0.9.0".into(),
                    version: "0.9.0".into(),
                    asset_name: "OpenAI.Codex_0.9.0.0_x64.msix".into(),
                    architecture: ReleaseArchitecture::X64,
                    format: ReleasePackageFormat::Msix,
                    package_version: Some("0.9.0.0".into()),
                    local_path: None,
                    local_file_name: None,
                },
                block_updates: true,
                expectation: HistoricalResumeExpectation::Windows {
                    current_path: None,
                    current_version: None,
                    current_source: None,
                },
                install_root: Some(r"C:\Codex".into()),
            };
            manager
                .set_historical_context(&token, historical.clone())
                .unwrap();
            manager.record_completion(&token, false).unwrap();
            assert!(manager.paused_snapshot().is_none(), "active lease wins");
            manager.end(token).unwrap();
            historical
        };

        pause_once(&manager);
        let paused = manager.paused_snapshot().expect("terminal pause");
        assert!(paused.paused);
        assert_eq!(
            paused
                .historical
                .as_ref()
                .and_then(|historical| historical.install_root.as_deref()),
            Some(r"C:\Codex")
        );

        manager.clear_paused_snapshot();
        assert!(manager.paused_snapshot().is_none());

        let retained = pause_once(&manager);
        let resume = manager.begin_detached(OperationKind::Install).unwrap();
        manager.validate(&resume).unwrap();
        manager
            .set_historical_context(&resume, retained.clone())
            .unwrap();
        manager
            .set_progress(
                &resume,
                OperationProgress {
                    downloaded: 96,
                    total: 128,
                    source: "github.com".into(),
                },
            )
            .unwrap();
        let completion = manager.record_completion(&resume, false).unwrap();
        assert_eq!(
            completion.state,
            OperationCompletionState::FailedBeforeCommit
        );
        manager.end(resume).unwrap();
        assert_eq!(
            manager
                .paused_snapshot()
                .and_then(|paused| paused.historical),
            Some(retained.clone())
        );

        let successful_resume = manager.begin_detached(OperationKind::Install).unwrap();
        manager.validate(&successful_resume).unwrap();
        manager
            .set_historical_context(&successful_resume, retained)
            .unwrap();
        manager
            .set_progress(
                &successful_resume,
                OperationProgress {
                    downloaded: 128,
                    total: 128,
                    source: "github.com".into(),
                },
            )
            .unwrap();
        manager.record_completion(&successful_resume, true).unwrap();
        manager.end(successful_resume).unwrap();
        assert!(manager.paused_snapshot().is_none());

        let retained = pause_once(&manager);
        let ambiguous_resume = manager.begin_detached(OperationKind::Install).unwrap();
        manager.validate(&ambiguous_resume).unwrap();
        manager
            .set_historical_context(&ambiguous_resume, retained)
            .unwrap();
        manager
            .set_progress(
                &ambiguous_resume,
                OperationProgress {
                    downloaded: 112,
                    total: 128,
                    source: "github.com".into(),
                },
            )
            .unwrap();
        manager.mark_outcome_ambiguous(&ambiguous_resume).unwrap();
        let completion = manager.record_completion(&ambiguous_resume, false).unwrap();
        assert_eq!(completion.state, OperationCompletionState::OutcomeUnknown);
        manager.end(ambiguous_resume).unwrap();
        assert!(manager.paused_snapshot().is_none());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn cancellation_signal_runs_only_inside_a_live_interruptible_lease() {
        use crate::app::op_phase::OperationPhase;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let path = lock_path("cancel-linearization");
        let manager = OperationManager::new(path.clone());
        let calls = AtomicUsize::new(0);
        let missing = OperationToken("missing".to_string());
        assert!(!manager.request_cancellation(&missing, || {
            calls.fetch_add(1, Ordering::SeqCst);
            true
        }));

        let guard = manager.begin(OperationKind::Update).unwrap();
        let owner = guard.token().clone();
        assert!(manager.request_cancellation(&owner, || {
            calls.fetch_add(1, Ordering::SeqCst);
            true
        }));
        manager
            .set_phase(guard.token(), OperationPhase::Committing)
            .unwrap();
        assert!(!manager.request_cancellation(&owner, || {
            calls.fetch_add(1, Ordering::SeqCst);
            true
        }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        drop(guard);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pause_signal_and_paused_marker_share_the_same_live_lease() {
        use crate::app::op_phase::OperationPhase;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let path = lock_path("pause-linearization");
        let manager = OperationManager::new(path.clone());
        let calls = AtomicUsize::new(0);
        let missing = OperationToken("missing".to_string());
        assert!(!manager.request_pause(&missing, || {
            calls.fetch_add(1, Ordering::SeqCst);
            true
        }));

        let guard = manager.begin(OperationKind::Update).unwrap();
        let owner = guard.token().clone();
        assert!(!manager.request_pause(&owner, || {
            calls.fetch_add(1, Ordering::SeqCst);
            false
        }));
        assert!(!manager.snapshot().unwrap().paused);
        assert!(manager.request_pause(&owner, || {
            calls.fetch_add(1, Ordering::SeqCst);
            true
        }));
        assert!(manager.snapshot().unwrap().paused);

        manager
            .set_phase(guard.token(), OperationPhase::Committing)
            .unwrap();
        assert!(!manager.request_pause(&owner, || {
            calls.fetch_add(1, Ordering::SeqCst);
            true
        }));
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        drop(guard);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn accepted_pause_requires_cancelled_worker_evidence_before_retention() {
        use crate::app::op_phase::OperationPhase;

        let path = lock_path("pause-confirmation");
        let manager = OperationManager::new(path.clone());

        let raced = manager.begin_detached(OperationKind::Install).unwrap();
        manager.validate(&raced).unwrap();
        manager
            .set_phase(&raced, OperationPhase::Downloading)
            .unwrap();
        assert!(manager.request_pause(&raced, || true));
        // The downloader completed and a later verification error ended the
        // operation. A successful pause signal alone must not retain it.
        manager.record_completion(&raced, false).unwrap();
        manager.end(raced).unwrap();
        assert!(manager.paused_snapshot().is_none());

        let cancelled = manager.begin_detached(OperationKind::Install).unwrap();
        manager.validate(&cancelled).unwrap();
        manager
            .set_phase(&cancelled, OperationPhase::Downloading)
            .unwrap();
        assert!(manager.request_pause(&cancelled, || true));
        manager.confirm_pause(&cancelled).unwrap();
        manager.record_completion(&cancelled, false).unwrap();
        manager.end(cancelled).unwrap();
        assert!(manager.paused_snapshot().is_some());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stale_stop_token_cannot_target_the_next_operation() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let path = lock_path("stale-stop-token");
        let manager = OperationManager::new(path.clone());
        let first = manager.begin(OperationKind::Update).unwrap();
        let stale = first.token().clone();
        drop(first);

        let second = manager.begin(OperationKind::Install).unwrap();
        let calls = AtomicUsize::new(0);
        assert!(!manager.request_cancellation(&stale, || {
            calls.fetch_add(1, Ordering::SeqCst);
            true
        }));
        assert!(!manager.request_pause(&stale, || {
            calls.fetch_add(1, Ordering::SeqCst);
            true
        }));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!manager.snapshot().unwrap().paused);

        assert!(manager.request_pause(second.token(), || {
            calls.fetch_add(1, Ordering::SeqCst);
            true
        }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(manager.snapshot().unwrap().paused);

        drop(second);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn confirmed_quit_preparation_linearizes_before_commit_transition() {
        use crate::app::op_phase::{OperationPhase, QuitPolicy};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Barrier};

        let path = lock_path("confirmed-quit-linearization");
        let manager = OperationManager::new(path.clone());
        let guard = manager.begin(OperationKind::Install).unwrap();
        manager
            .set_phase(guard.token(), OperationPhase::Verifying)
            .unwrap();

        let attempted = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicBool::new(false));
        let prepared = Arc::new(AtomicBool::new(false));
        let start = Arc::new(Barrier::new(2));
        let worker_manager = manager.clone();
        let worker_token = guard.token().clone();
        let worker_attempted = Arc::clone(&attempted);
        let worker_completed = Arc::clone(&completed);
        let worker_start = Arc::clone(&start);
        let worker = std::thread::spawn(move || {
            worker_start.wait();
            worker_attempted.store(true, Ordering::SeqCst);
            worker_manager
                .set_phase(&worker_token, OperationPhase::Committing)
                .unwrap();
            worker_completed.store(true, Ordering::SeqCst);
        });

        let prepared_for_quit = Arc::clone(&prepared);
        let policy = manager.prepare_quit(true, true, || {
            start.wait();
            while !attempted.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            assert!(
                !completed.load(Ordering::SeqCst),
                "phase transition must stay behind confirmed-quit preparation"
            );
            prepared_for_quit.store(true, Ordering::SeqCst);
        });
        assert_eq!(policy, QuitPolicy::Confirm);
        assert!(prepared.load(Ordering::SeqCst));
        worker.join().unwrap();
        assert!(completed.load(Ordering::SeqCst));

        let blocked_preparation = AtomicBool::new(false);
        let blocked = manager.prepare_quit(true, true, || {
            blocked_preparation.store(true, Ordering::SeqCst);
        });
        assert!(matches!(blocked, QuitPolicy::Block { .. }));
        assert!(!blocked_preparation.load(Ordering::SeqCst));

        let needs_confirmation = AtomicBool::new(false);
        manager
            .set_phase(guard.token(), OperationPhase::Verifying)
            .unwrap();
        let confirm = manager.prepare_quit(true, false, || {
            needs_confirmation.store(true, Ordering::SeqCst);
        });
        assert_eq!(confirm, QuitPolicy::Confirm);
        assert!(!needs_confirmation.load(Ordering::SeqCst));

        let automatic_preparation = AtomicBool::new(false);
        let allow = manager.prepare_quit(false, false, || {
            automatic_preparation.store(true, Ordering::SeqCst);
        });
        assert_eq!(allow, QuitPolicy::Allow);
        assert!(automatic_preparation.load(Ordering::SeqCst));

        drop(guard);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn quit_abandons_unclaimed_destructive_token_or_commit_claim_blocks_quit() {
        use crate::app::op_phase::{OperationPhase, QuitPolicy};
        use std::sync::atomic::{AtomicBool, Ordering};

        let path = lock_path("quit-vs-destructive-claim");
        let manager = OperationManager::new(path.clone());

        // Quit wins: the still-unclaimed token is removed, so its delayed command
        // cannot validate and begin destructive work after force-exit preparation.
        let abandoned = manager.begin_detached(OperationKind::Uninstall).unwrap();
        let prepared = AtomicBool::new(false);
        assert_eq!(
            manager.prepare_quit(false, false, || prepared.store(true, Ordering::SeqCst)),
            QuitPolicy::Allow
        );
        assert!(prepared.load(Ordering::SeqCst));
        assert!(matches!(
            manager.validate_with_phase(&abandoned, OperationPhase::Committing),
            Err(OperationError::InvalidToken)
        ));
        assert!(!manager.is_busy());

        // Destructive claim wins: validation and Committing are atomic, so quit
        // observes the point of no return and does not run its preparation closure.
        let committed = manager.begin_detached(OperationKind::Uninstall).unwrap();
        manager
            .validate_with_phase(&committed, OperationPhase::Committing)
            .unwrap();
        let should_not_prepare = AtomicBool::new(false);
        let blocked = manager.prepare_quit(false, false, || {
            should_not_prepare.store(true, Ordering::SeqCst);
        });
        assert!(matches!(blocked, QuitPolicy::Block { .. }));
        assert!(!should_not_prepare.load(Ordering::SeqCst));
        manager.end(committed).unwrap();
    }

    #[test]
    fn terminal_completion_uses_mutation_evidence_independently_of_quit_phase() {
        use crate::app::op_phase::OperationPhase;

        let path = lock_path("completion");
        let manager = OperationManager::new(path.clone());

        let before_commit = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&before_commit).unwrap();
        manager
            .set_phase(&before_commit, OperationPhase::Downloading)
            .unwrap();
        let completion = manager.record_completion(&before_commit, false).unwrap();
        assert_eq!(
            completion.state,
            OperationCompletionState::FailedBeforeCommit
        );
        manager.end(before_commit.clone()).unwrap();
        assert_eq!(manager.completion(&before_commit), Some(completion));

        let committing_without_write = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&committing_without_write).unwrap();
        manager
            .set_phase(&committing_without_write, OperationPhase::Committing)
            .unwrap();
        let completion = manager
            .record_completion(&committing_without_write, false)
            .unwrap();
        assert_eq!(
            completion.state,
            OperationCompletionState::FailedBeforeCommit
        );
        manager.end(committing_without_write).unwrap();

        let after_mutation = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&after_mutation).unwrap();
        manager.mark_mutation_started(&after_mutation).unwrap();
        let completion = manager.record_completion(&after_mutation, false).unwrap();
        assert_eq!(completion.state, OperationCompletionState::OutcomeUnknown);
        manager.end(after_mutation.clone()).unwrap();
        assert_eq!(manager.completion(&after_mutation), Some(completion));

        let rolled_back = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&rolled_back).unwrap();
        manager.mark_mutation_started(&rolled_back).unwrap();
        assert!(matches!(
            manager.mark_mutation_rolled_back(&OperationToken("wrong".to_string())),
            Err(OperationError::InvalidToken)
        ));
        manager.mark_mutation_rolled_back(&rolled_back).unwrap();
        let completion = manager.record_completion(&rolled_back, false).unwrap();
        assert_eq!(completion.state, OperationCompletionState::RolledBack);
        manager.end(rolled_back.clone()).unwrap();
        assert_eq!(manager.completion(&rolled_back), Some(completion));

        let rollback_before_mutation = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&rollback_before_mutation).unwrap();
        manager
            .mark_mutation_rolled_back(&rollback_before_mutation)
            .unwrap();
        let completion = manager
            .record_completion(&rollback_before_mutation, false)
            .unwrap();
        assert_eq!(
            completion.state,
            OperationCompletionState::FailedBeforeCommit
        );
        manager.end(rollback_before_mutation).unwrap();

        let ambiguous_rollback = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&ambiguous_rollback).unwrap();
        manager.mark_outcome_ambiguous(&ambiguous_rollback).unwrap();
        manager.mark_mutation_started(&ambiguous_rollback).unwrap();
        manager
            .mark_mutation_rolled_back(&ambiguous_rollback)
            .unwrap();
        let completion = manager
            .record_completion(&ambiguous_rollback, false)
            .unwrap();
        assert_eq!(completion.state, OperationCompletionState::OutcomeUnknown);
        manager.end(ambiguous_rollback).unwrap();

        let mutation_after_rollback = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&mutation_after_rollback).unwrap();
        manager
            .mark_mutation_started(&mutation_after_rollback)
            .unwrap();
        manager
            .mark_mutation_rolled_back(&mutation_after_rollback)
            .unwrap();
        manager
            .mark_mutation_started(&mutation_after_rollback)
            .unwrap();
        let completion = manager
            .record_completion(&mutation_after_rollback, false)
            .unwrap();
        assert_eq!(completion.state, OperationCompletionState::OutcomeUnknown);
        manager.end(mutation_after_rollback).unwrap();

        let ambiguous_call = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&ambiguous_call).unwrap();
        manager.mark_outcome_ambiguous(&ambiguous_call).unwrap();
        let completion = manager.record_completion(&ambiguous_call, false).unwrap();
        assert_eq!(completion.state, OperationCompletionState::OutcomeUnknown);
        manager.end(ambiguous_call.clone()).unwrap();
        assert_eq!(manager.completion(&ambiguous_call), Some(completion));

        let succeeded = manager.begin_detached(OperationKind::Update).unwrap();
        manager.validate(&succeeded).unwrap();
        let completion = manager.record_completion(&succeeded, true).unwrap();
        assert_eq!(completion.state, OperationCompletionState::Succeeded);
        manager.end(succeeded.clone()).unwrap();
        assert_eq!(manager.completion(&succeeded), Some(completion));

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
