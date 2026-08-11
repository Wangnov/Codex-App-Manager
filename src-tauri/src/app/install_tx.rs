//! Crash-safe install transaction log + startup recovery.
//!
//! Before the first destructive rename (old install → backup), a durable log is
//! written. On the next launch we scan pending logs and decide:
//!   - **continue** — finish moving the staged payload into place
//!   - **rollback** — restore the backup
//!   - **keep** — leave materials for manual recovery when the matrix is ambiguous
//!
//! Recovery always runs before ordinary staging/backup cleanup.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app::atomic_file;
use crate::app::paths;
use crate::errors::AppError;

pub const SCHEMA_VERSION: u32 = 1;

/// Platform / path kind of a destructive install swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallTxKind {
    MacosSwap,
    WindowsPortable,
}

impl InstallTxKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MacosSwap => "macos-swap",
            Self::WindowsPortable => "windows-portable",
        }
    }
}

/// Durable step markers written across rename boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallTxStep {
    /// Log persisted; no destructive rename yet.
    Prepared,
    /// Old install moved to backup; install path is empty (or missing).
    OldMoved,
    /// New payload moved into install path.
    NewInstalled,
    /// Rollback was durably chosen before the backup is consumed.
    RollingBack,
    /// Success path finished; log may be deleted.
    Completed,
    /// Backup restored over install path.
    RolledBack,
    /// Ambiguous on-disk state; materials retained for manual recovery.
    NeedsManual,
}

/// Durable companion state for a historical install's explicit self-update
/// choice. It is written before the platform setting changes, then finalized
/// from the same disk-recovery verdict as the app swap: rollback/intact keeps
/// `previous_disabled`, while a landed install keeps `requested_disabled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfUpdatePolicyTransition {
    pub previous_disabled: bool,
    pub requested_disabled: bool,
}

impl InstallTxStep {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::OldMoved => "old-moved",
            Self::NewInstalled => "new-installed",
            Self::RollingBack => "rolling-back",
            Self::Completed => "completed",
            Self::RolledBack => "rolled-back",
            Self::NeedsManual => "needs-manual",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::RolledBack | Self::NeedsManual)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallTransaction {
    #[serde(default = "default_schema")]
    pub schema_version: u32,
    pub id: String,
    pub kind: InstallTxKind,
    pub step: InstallTxStep,
    pub install_path: String,
    pub new_path: String,
    pub backup_path: String,
    pub had_previous: bool,
    #[serde(default)]
    pub was_running: Option<bool>,
    #[serde(default)]
    pub self_update_policy: Option<SelfUpdatePolicyTransition>,
    pub started_unix: u64,
    pub updated_unix: u64,
    #[serde(default)]
    pub notes: Vec<String>,
}

fn default_schema() -> u32 {
    SCHEMA_VERSION
}

/// Pure recovery decision for the macOS swap / Windows portable rename matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// No damage yet — drop the log.
    ClearLog,
    /// Rename staged new → install, then clean up.
    ContinueInstall,
    /// Rename backup → install.
    Rollback,
    /// The rollback rename already landed; finalize the old-policy verdict.
    FinishRollback,
    /// Install already good; drop backup if present and clear log.
    Complete,
    /// Leave paths + log (marked needs-manual) for human inspection.
    KeepManual { reason: &'static str },
}

/// Decide recovery from the durable step + which paths still exist.
///
/// Matrix (macOS swap and Windows portable share the same two-rename shape).
///
/// **Prepared is reality-based**: a process kill can land between
/// `rename(old→backup)` and the durable `OldMoved` mark. Inspecting the disk
/// (not the step alone) is required so we never ClearLog away a half-swap.
///
/// | step          | install | backup | new | action            |
/// |---------------|---------|--------|-----|-------------------|
/// | prepared      | yes     | *      | *   | clear log (intact)|
/// | prepared      | no      | yes    | yes | **continue**      |
/// | prepared      | no      | yes    | no  | **rollback**      |
/// | prepared      | no      | no     | yes | keep              |
/// | prepared      | no      | no     | no  | keep              |
/// | old-moved     | no      | yes    | yes | continue          |
/// | old-moved     | no      | yes    | no  | rollback          |
/// | old-moved     | no      | no     | yes | keep (no backup)  |
/// | old-moved     | no      | no     | no  | keep (all missing)|
/// | old-moved     | yes     | *      | *   | complete          |
/// | new-installed | yes     | *      | *   | complete          |
/// | new-installed | no      | yes    | *   | rollback          |
/// | new-installed | no      | no     | *   | keep              |
/// | rolling-back  | *       | yes    | *   | rollback          |
/// | rolling-back  | yes     | no     | *   | finish rollback   |
/// | rolling-back  | no      | no     | *   | keep (or absent for fresh tx) |
/// | terminal      | *       | *      | *   | clear / keep note |
pub fn decide_recovery(
    step: InstallTxStep,
    install_exists: bool,
    backup_exists: bool,
    new_exists: bool,
) -> RecoveryAction {
    match step {
        InstallTxStep::Completed | InstallTxStep::RolledBack => RecoveryAction::ClearLog,
        InstallTxStep::NeedsManual => RecoveryAction::KeepManual {
            reason: "previous recovery already marked needs-manual",
        },
        // Reality-based: Prepared may still mean "old already moved" if we died
        // between rename and mark_old_moved.
        InstallTxStep::Prepared => match (install_exists, backup_exists, new_exists) {
            (true, _, _) => RecoveryAction::ClearLog,
            (false, true, true) => RecoveryAction::ContinueInstall,
            (false, true, false) => RecoveryAction::Rollback,
            (false, false, true) => RecoveryAction::KeepManual {
                reason: "prepared log but install missing, backup missing; staged new retained",
            },
            (false, false, false) => RecoveryAction::KeepManual {
                reason: "prepared log but install/backup/new all missing",
            },
        },
        InstallTxStep::OldMoved => match (install_exists, backup_exists, new_exists) {
            (false, true, true) => RecoveryAction::ContinueInstall,
            (false, true, false) => RecoveryAction::Rollback,
            (false, false, true) => RecoveryAction::KeepManual {
                reason: "old moved aside but backup missing; staged new retained",
            },
            (false, false, false) => RecoveryAction::KeepManual {
                reason: "install, backup, and staged new all missing after old-moved",
            },
            (true, _, _) => RecoveryAction::Complete,
        },
        InstallTxStep::NewInstalled => match (install_exists, backup_exists) {
            (true, _) => RecoveryAction::Complete,
            (false, true) => RecoveryAction::Rollback,
            (false, false) => RecoveryAction::KeepManual {
                reason: "new was installed but install path missing and no backup",
            },
        },
        InstallTxStep::RollingBack => match (install_exists, backup_exists) {
            // The intent is durable and the only backup still exists: finish the
            // rollback, replacing any failed/new tree that may remain.
            (_, true) => RecoveryAction::Rollback,
            // backup -> install already landed before the terminal mark.
            (true, false) => RecoveryAction::FinishRollback,
            (false, false) => RecoveryAction::KeepManual {
                reason: "rollback was started but install and backup are both missing",
            },
        },
    }
}

/// True when on-disk layout suggests a destructive rename already happened
/// even if the durable step is still `Prepared`.
pub fn prepared_looks_half_swapped(
    install_exists: bool,
    backup_exists: bool,
    new_exists: bool,
) -> bool {
    !install_exists && (backup_exists || new_exists)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn transactions_dir() -> Option<PathBuf> {
    paths::data_dir().map(|dir| dir.join("install-transactions"))
}

pub fn tx_path_for(id: &str) -> Option<PathBuf> {
    transactions_dir().map(|dir| dir.join(format!("{id}.json")))
}

impl InstallTransaction {
    pub fn begin(
        kind: InstallTxKind,
        install_path: &Path,
        new_path: &Path,
        backup_path: &Path,
        had_previous: bool,
        was_running: Option<bool>,
    ) -> Result<Self, AppError> {
        let now = now_unix();
        let tx = Self {
            schema_version: SCHEMA_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            step: InstallTxStep::Prepared,
            install_path: install_path.to_string_lossy().into_owned(),
            new_path: new_path.to_string_lossy().into_owned(),
            backup_path: backup_path.to_string_lossy().into_owned(),
            had_previous,
            was_running,
            self_update_policy: None,
            started_unix: now,
            updated_unix: now,
            notes: Vec::new(),
        };
        tx.persist()?;
        log::info!(
            "install transaction prepared id={} kind={} install={}",
            tx.id,
            kind.as_str(),
            tx.install_path
        );
        Ok(tx)
    }

    pub fn persist(&self) -> Result<(), AppError> {
        let path = tx_path_for(&self.id).ok_or_else(|| {
            AppError::Internal("无法定位 install-transactions 数据目录".to_string())
        })?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AppError::Internal(format!("创建事务日志目录失败: {e}")))?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| AppError::Internal(format!("序列化事务日志失败: {e}")))?;
        atomic_file::write_atomic(&path, &bytes)
            .map_err(|e| AppError::Internal(format!("写入事务日志失败: {e}")))?;
        Ok(())
    }

    pub fn advance(&mut self, step: InstallTxStep) -> Result<(), AppError> {
        self.step = step;
        self.updated_unix = now_unix();
        self.persist()?;
        log::info!(
            "install transaction step id={} step={}",
            self.id,
            step.as_str()
        );
        Ok(())
    }

    pub fn note(&mut self, message: impl Into<String>) -> Result<(), AppError> {
        self.notes.push(message.into());
        self.updated_unix = now_unix();
        self.persist()
    }

    pub fn set_self_update_policy_transition(
        &mut self,
        transition: SelfUpdatePolicyTransition,
    ) -> Result<(), AppError> {
        self.self_update_policy = Some(transition);
        self.updated_unix = now_unix();
        self.persist()
    }

    fn finalize_self_update_policy(&self, install_landed: bool) -> Result<(), AppError> {
        let Some(disabled) = self_update_policy_for_outcome(self, install_landed) else {
            return Ok(());
        };
        crate::app::codex_self_update::sync_and_persist_setting(disabled)
    }

    pub fn complete(mut self) -> Result<(), AppError> {
        self.step = InstallTxStep::Completed;
        self.updated_unix = now_unix();
        // Persist the terminal verdict before finalizing policy. A crash between
        // either write is replayed idempotently by startup recovery.
        self.persist()?;
        self.finalize_self_update_policy(true)?;
        self.remove_file_checked()?;
        Ok(())
    }

    /// Finalize a live install without turning an already-landed, healthy app
    /// into an installation error merely because transaction bookkeeping could
    /// not be closed. Any failed step leaves the last durable journal in place
    /// for idempotent startup recovery and is returned as a user-visible warning.
    fn complete_live(mut self) -> Vec<String> {
        self.step = InstallTxStep::Completed;
        self.updated_unix = now_unix();
        let mut warnings = Vec::new();
        let terminal_persisted = match self.persist() {
            Ok(()) => true,
            Err(err) => {
                let warning = format!(
                    "应用已安装，但安装事务的完成状态暂未写入（{err}）；管理器下次启动会按磁盘状态恢复"
                );
                log::warn!(
                    "live install transaction finalization warning id={} error={err}",
                    self.id
                );
                warnings.push(warning);
                false
            }
        };

        let policy_finalized = match self.finalize_self_update_policy(true) {
            Ok(()) => true,
            Err(err) => {
                let warning = format!(
                    "应用已安装，但所选自动更新策略暂未完成（{err}）；管理器下次启动会重试"
                );
                log::warn!(
                    "live install policy finalization warning id={} error={err}",
                    self.id
                );
                warnings.push(warning);
                false
            }
        };

        // Remove the journal only after both durable terminal state and policy
        // succeeded. Otherwise it is the recovery instruction for next launch.
        if terminal_persisted && policy_finalized {
            if let Err(err) = self.remove_file_checked() {
                let warning = format!(
                    "应用已安装，但安装事务日志暂未清理（{err}）；管理器下次启动会自动清理"
                );
                log::warn!(
                    "live install transaction cleanup warning id={} error={err}",
                    self.id
                );
                warnings.push(warning);
            }
        }
        warnings
    }

    pub fn remove_file(&self) {
        if let Some(path) = tx_path_for(&self.id) {
            let _ = fs::remove_file(path);
        }
    }

    fn remove_file_checked(&self) -> Result<(), AppError> {
        let path = tx_path_for(&self.id).ok_or_else(|| {
            AppError::Internal("无法定位 install-transactions 数据目录".to_string())
        })?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(AppError::Internal(format!(
                "删除事务日志失败（{}）: {err}",
                path.display()
            ))),
        }
    }

    pub fn load_from_path(path: &Path) -> Result<Self, AppError> {
        let bytes =
            fs::read(path).map_err(|e| AppError::Internal(format!("读取事务日志失败: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| AppError::Internal(format!("解析事务日志失败: {e}")))
    }
}

fn self_update_policy_for_outcome(tx: &InstallTransaction, install_landed: bool) -> Option<bool> {
    tx.self_update_policy.map(|transition| {
        if install_landed {
            transition.requested_disabled
        } else {
            transition.previous_disabled
        }
    })
}

#[derive(Debug, Clone, Default)]
pub struct RecoverySummary {
    pub scanned: usize,
    pub continued: usize,
    pub rolled_back: usize,
    pub completed: usize,
    pub cleared: usize,
    pub kept_manual: usize,
    pub failed: usize,
}

/// Paths referenced by any pending (non-cleared) transaction, including
/// `NeedsManual`. Staging cleanup must not delete these.
///
/// We protect install/new/backup **themselves** and staging-ish parents of
/// `new_path` / `backup_path` (update-* dirs, `.codex-app-manager-staging`),
/// but never the install's parent (e.g. `/Applications`) — that would block
/// unrelated cleanup.
pub fn protected_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(dir) = transactions_dir() else {
        return out;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(tx) = InstallTransaction::load_from_path(&path) else {
            continue;
        };
        if matches!(
            tx.step,
            InstallTxStep::Completed | InstallTxStep::RolledBack
        ) {
            continue;
        }
        for raw in [&tx.install_path, &tx.new_path, &tx.backup_path] {
            out.push(PathBuf::from(raw));
        }
        // Staging parents for new + backup only.
        for raw in [&tx.new_path, &tx.backup_path] {
            let p = PathBuf::from(raw);
            let mut cur = p.parent();
            while let Some(parent) = cur {
                let name = parent.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let staging_ish = name.starts_with("update-")
                    || name.starts_with("portable-")
                    || name == ".codex-app-manager-staging"
                    || name.starts_with("Codex.rollback")
                    || name.starts_with("backup-");
                if staging_ish {
                    out.push(parent.to_path_buf());
                    cur = parent.parent();
                } else {
                    break;
                }
            }
        }
    }
    out
}

/// Whether `path` is covered by a pending install transaction and must not be
/// reclaimed by staging cleanup.
pub fn path_is_protected(path: &Path, protected: &[PathBuf]) -> bool {
    protected
        .iter()
        .any(|p| path == p || path.starts_with(p) || p.starts_with(path))
}

/// Scan pending transaction logs and apply the recovery matrix. Must run
/// **before** ordinary staging cleanup so recovery materials are not deleted.
///
/// When `ops` is provided, recovery holds an operation lease so concurrent
/// install/update cannot race staging cleanup against recovery renames.
pub fn recover_pending_transactions(
    ops: Option<&crate::app::oplock::OperationManager>,
) -> RecoverySummary {
    let mut summary = RecoverySummary::default();
    let _lease = if let Some(ops) = ops {
        match ops.begin(crate::app::oplock::OperationKind::Install) {
            Ok(guard) => Some(guard),
            Err(err) => {
                log::warn!("install transaction recovery deferred (operation busy) error={err}");
                return summary;
            }
        }
    } else {
        None
    };

    let Some(dir) = transactions_dir() else {
        return summary;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return summary;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        summary.scanned += 1;
        match recover_one(&path) {
            Ok(outcome) => match outcome {
                Recovered::Continued => summary.continued += 1,
                Recovered::RolledBack => summary.rolled_back += 1,
                Recovered::Completed => summary.completed += 1,
                Recovered::Cleared => summary.cleared += 1,
                Recovered::KeptManual => summary.kept_manual += 1,
            },
            Err(err) => {
                summary.failed += 1;
                log::error!(
                    "install transaction recovery failed path={} error={err}",
                    path.display()
                );
            }
        }
    }
    if summary.scanned > 0 {
        log::info!(
            "install transaction recovery summary scanned={} continued={} rolled_back={} completed={} cleared={} kept_manual={} failed={}",
            summary.scanned,
            summary.continued,
            summary.rolled_back,
            summary.completed,
            summary.cleared,
            summary.kept_manual,
            summary.failed
        );
    }
    summary
}

#[derive(Debug)]
enum Recovered {
    Continued,
    RolledBack,
    Completed,
    Cleared,
    KeptManual,
}

fn path_exists(p: &str) -> bool {
    Path::new(p).exists()
}

fn recover_one(path: &Path) -> Result<Recovered, AppError> {
    let mut tx = InstallTransaction::load_from_path(path)?;
    if tx.step.is_terminal()
        && matches!(
            tx.step,
            InstallTxStep::Completed | InstallTxStep::RolledBack
        )
    {
        tx.finalize_self_update_policy(matches!(tx.step, InstallTxStep::Completed))?;
        tx.remove_file_checked()?;
        return Ok(Recovered::Cleared);
    }

    let install_exists = path_exists(&tx.install_path);
    let backup_exists = path_exists(&tx.backup_path);
    let new_exists = path_exists(&tx.new_path);
    let action = decide_transaction_recovery(&tx, install_exists, backup_exists, new_exists);
    log::info!(
        "install transaction recover id={} step={} action={:?} install={} backup={} new={}",
        tx.id,
        tx.step.as_str(),
        action,
        install_exists,
        backup_exists,
        new_exists
    );

    match action {
        RecoveryAction::ClearLog => {
            // Prepared + intact means the app swap never landed. Restore the
            // pre-confirmation policy before removing the only durable evidence.
            tx.finalize_self_update_policy(false)?;
            tx.remove_file_checked()?;
            Ok(Recovered::Cleared)
        }
        RecoveryAction::ContinueInstall => {
            fs::rename(&tx.new_path, &tx.install_path)
                .map_err(|e| AppError::Internal(format!("recovery continue rename failed: {e}")))?;
            tx.advance(InstallTxStep::NewInstalled)?;
            cleanup_backup_best_effort(&tx);
            tx.complete()?;
            Ok(Recovered::Continued)
        }
        RecoveryAction::Rollback => {
            // Persist rollback intent before consuming the only backup. If the
            // process dies after backup -> install but before the terminal mark,
            // `RollingBack + install present + backup absent` is unambiguously a
            // completed rollback, never a completed installation.
            if tx.step != InstallTxStep::RollingBack {
                tx.advance(InstallTxStep::RollingBack)?;
            }
            tx.finalize_self_update_policy(false)?;
            if path_exists(&tx.install_path) {
                let _ = fs::remove_dir_all(&tx.install_path);
                let _ = fs::remove_file(&tx.install_path);
            }
            if tx.had_previous {
                fs::rename(&tx.backup_path, &tx.install_path).map_err(|e| {
                    AppError::Internal(format!("recovery rollback rename failed: {e}"))
                })?;
            }
            tx.advance(InstallTxStep::RolledBack)?;
            tx.remove_file_checked()?;
            Ok(Recovered::RolledBack)
        }
        RecoveryAction::FinishRollback => {
            tx.finalize_self_update_policy(false)?;
            tx.advance(InstallTxStep::RolledBack)?;
            tx.remove_file_checked()?;
            Ok(Recovered::RolledBack)
        }
        RecoveryAction::Complete => {
            cleanup_backup_best_effort(&tx);
            // Staged new should already be gone; remove if leftover.
            if path_exists(&tx.new_path) {
                let _ = fs::remove_dir_all(&tx.new_path);
                let _ = fs::remove_file(&tx.new_path);
            }
            tx.complete()?;
            Ok(Recovered::Completed)
        }
        RecoveryAction::KeepManual { reason } => {
            tx.step = InstallTxStep::NeedsManual;
            tx.updated_unix = now_unix();
            tx.notes.push(reason.to_string());
            tx.persist()?;
            log::error!(
                "install transaction needs manual recovery id={} reason={reason} install={} backup={} new={}",
                tx.id,
                tx.install_path,
                tx.backup_path,
                tx.new_path
            );
            Ok(Recovered::KeptManual)
        }
    }
}

/// Fresh installs have no old tree to move aside. Their Prepared crash matrix
/// therefore differs from a replacement: payload present + target absent means
/// no mutation, while target present + payload absent proves the atomic rename
/// landed even if the process died before persisting NewInstalled.
fn decide_transaction_recovery(
    tx: &InstallTransaction,
    install_exists: bool,
    backup_exists: bool,
    new_exists: bool,
) -> RecoveryAction {
    if tx.step == InstallTxStep::RollingBack && !tx.had_previous {
        return if install_exists {
            RecoveryAction::Rollback
        } else {
            // A fresh install rolls back to absence, so no install and no backup
            // is the expected completed state rather than an ambiguity.
            RecoveryAction::FinishRollback
        };
    }
    if tx.had_previous || tx.step != InstallTxStep::Prepared {
        return decide_recovery(tx.step, install_exists, backup_exists, new_exists);
    }
    match (install_exists, backup_exists, new_exists) {
        (false, false, true) => RecoveryAction::ClearLog,
        (true, false, false) => RecoveryAction::Complete,
        _ => RecoveryAction::KeepManual {
            reason: "fresh-install prepared state is ambiguous",
        },
    }
}

fn cleanup_backup_best_effort(tx: &InstallTransaction) {
    if path_exists(&tx.backup_path) {
        let _ = fs::remove_dir_all(&tx.backup_path);
        let _ = fs::remove_file(&tx.backup_path);
    }
}

/// RAII helper used by perform paths: advances steps and completes/clears on drop
/// only if still non-terminal (failure path leaves the log for startup recovery).
pub struct ActiveInstallTx {
    inner: Option<InstallTransaction>,
}

impl ActiveInstallTx {
    pub fn begin(
        kind: InstallTxKind,
        install_path: &Path,
        new_path: &Path,
        backup_path: &Path,
        had_previous: bool,
        was_running: Option<bool>,
    ) -> Result<Self, AppError> {
        let tx = InstallTransaction::begin(
            kind,
            install_path,
            new_path,
            backup_path,
            had_previous,
            was_running,
        )?;
        Ok(Self { inner: Some(tx) })
    }

    pub fn mark_old_moved(&mut self) -> Result<(), AppError> {
        if let Some(tx) = self.inner.as_mut() {
            tx.advance(InstallTxStep::OldMoved)?;
        }
        Ok(())
    }

    pub fn mark_new_installed(&mut self) -> Result<(), AppError> {
        if let Some(tx) = self.inner.as_mut() {
            tx.advance(InstallTxStep::NewInstalled)?;
        }
        Ok(())
    }

    /// Persist the rollback verdict before deleting/replacing the new install or
    /// consuming the only backup.
    pub fn mark_rollback_started(&mut self) -> Result<(), AppError> {
        if let Some(tx) = self.inner.as_mut() {
            tx.advance(InstallTxStep::RollingBack)?;
        }
        Ok(())
    }

    pub fn set_self_update_policy_transition(
        &mut self,
        transition: SelfUpdatePolicyTransition,
    ) -> Result<(), AppError> {
        if let Some(tx) = self.inner.as_mut() {
            tx.set_self_update_policy_transition(transition)?;
        }
        Ok(())
    }

    pub fn step(&self) -> Option<InstallTxStep> {
        self.inner.as_ref().map(|tx| tx.step)
    }

    /// Mark success only after post-swap health / rollback has settled.
    pub fn succeed(mut self) -> Vec<String> {
        if let Some(tx) = self.inner.take() {
            return tx.complete_live();
        }
        Vec::new()
    }

    /// Record a successful rollback and clear the log.
    pub fn mark_rolled_back(mut self) -> Result<(), AppError> {
        if let Some(tx) = self.inner.take() {
            // Persist the terminal state before clearing the journal. If this
            // write or policy restore fails, startup recovery replays it instead
            // of claiming a fully recorded rollback.
            let mut tx = tx;
            tx.advance(InstallTxStep::RolledBack)?;
            tx.finalize_self_update_policy(false)?;
            tx.remove_file_checked()?;
        }
        Ok(())
    }

    /// Leave the log for startup recovery (e.g. keep backup after relaunch fail).
    pub fn leave_pending(mut self) {
        if let Some(tx) = self.inner.take() {
            log::warn!(
                "install transaction left pending id={} step={}",
                tx.id,
                tx.step.as_str()
            );
        }
    }

    /// Explicit abort before any destructive rename — safe to delete the log
    /// only when disk still looks untouched.
    pub fn abort_clean(mut self) {
        if let Some(tx) = self.inner.take() {
            if matches!(tx.step, InstallTxStep::Prepared)
                && tx.self_update_policy.is_none()
                && !prepared_looks_half_swapped(
                    path_exists(&tx.install_path),
                    path_exists(&tx.backup_path),
                    path_exists(&tx.new_path),
                )
            {
                tx.remove_file();
            }
            // Half-swapped Prepared / later steps: leave for startup recovery.
        }
    }
}

impl Drop for ActiveInstallTx {
    fn drop(&mut self) {
        // Non-terminal logs intentionally survive process death / panic so the
        // next launch can recover. Prepared-only logs are cleared ONLY when the
        // install path still looks intact — never clear a half-swap.
        if let Some(tx) = self.inner.take() {
            let half = prepared_looks_half_swapped(
                path_exists(&tx.install_path),
                path_exists(&tx.backup_path),
                path_exists(&tx.new_path),
            );
            if matches!(tx.step, InstallTxStep::Prepared)
                && tx.self_update_policy.is_none()
                && !half
            {
                tx.remove_file();
            } else {
                log::warn!(
                    "install transaction left pending for recovery id={} step={} half_swapped={half}",
                    tx.id,
                    tx.step.as_str()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn recovery_matrix_old_moved_boundaries() {
        assert_eq!(
            decide_recovery(InstallTxStep::OldMoved, false, true, true),
            RecoveryAction::ContinueInstall
        );
        assert_eq!(
            decide_recovery(InstallTxStep::OldMoved, false, true, false),
            RecoveryAction::Rollback
        );
        assert!(matches!(
            decide_recovery(InstallTxStep::OldMoved, false, false, true),
            RecoveryAction::KeepManual { .. }
        ));
        assert!(matches!(
            decide_recovery(InstallTxStep::OldMoved, false, false, false),
            RecoveryAction::KeepManual { .. }
        ));
        assert_eq!(
            decide_recovery(InstallTxStep::OldMoved, true, true, false),
            RecoveryAction::Complete
        );
    }

    #[test]
    fn recovery_matrix_new_installed_and_prepared() {
        assert_eq!(
            decide_recovery(InstallTxStep::NewInstalled, true, true, false),
            RecoveryAction::Complete
        );
        assert_eq!(
            decide_recovery(InstallTxStep::NewInstalled, false, true, false),
            RecoveryAction::Rollback
        );
        assert!(matches!(
            decide_recovery(InstallTxStep::NewInstalled, false, false, false),
            RecoveryAction::KeepManual { .. }
        ));
        // Intact prepared → clear.
        assert_eq!(
            decide_recovery(InstallTxStep::Prepared, true, false, true),
            RecoveryAction::ClearLog
        );
        assert_eq!(
            decide_recovery(InstallTxStep::Completed, true, false, false),
            RecoveryAction::ClearLog
        );
        assert_eq!(
            decide_recovery(InstallTxStep::RollingBack, true, false, false),
            RecoveryAction::FinishRollback
        );
        assert_eq!(
            decide_recovery(InstallTxStep::RollingBack, false, true, false),
            RecoveryAction::Rollback
        );
    }

    #[test]
    fn historical_policy_follows_the_durable_install_verdict() {
        let tx = InstallTransaction {
            schema_version: SCHEMA_VERSION,
            id: "policy-test".to_string(),
            kind: InstallTxKind::MacosSwap,
            step: InstallTxStep::Prepared,
            install_path: "/Applications/Codex.app".to_string(),
            new_path: "/tmp/update/Codex.app".to_string(),
            backup_path: "/tmp/update/backup-Codex.app".to_string(),
            had_previous: true,
            was_running: Some(true),
            self_update_policy: Some(SelfUpdatePolicyTransition {
                previous_disabled: false,
                requested_disabled: true,
            }),
            started_unix: 1,
            updated_unix: 1,
            notes: Vec::new(),
        };

        assert_eq!(self_update_policy_for_outcome(&tx, false), Some(false));
        assert_eq!(self_update_policy_for_outcome(&tx, true), Some(true));
        assert_eq!(
            decide_transaction_recovery(&tx, true, false, true),
            RecoveryAction::ClearLog,
            "a crash after policy persistence but before swap must restore the old policy"
        );
    }

    #[test]
    fn fresh_prepared_transaction_distinguishes_before_and_after_atomic_rename() {
        let tx = InstallTransaction {
            schema_version: SCHEMA_VERSION,
            id: "fresh-policy-test".to_string(),
            kind: InstallTxKind::MacosSwap,
            step: InstallTxStep::Prepared,
            install_path: "/Applications/Codex.app".to_string(),
            new_path: "/tmp/update/Codex.app".to_string(),
            backup_path: "/tmp/update/backup-Codex.app".to_string(),
            had_previous: false,
            was_running: Some(false),
            self_update_policy: Some(SelfUpdatePolicyTransition {
                previous_disabled: false,
                requested_disabled: true,
            }),
            started_unix: 1,
            updated_unix: 1,
            notes: Vec::new(),
        };

        assert_eq!(
            decide_transaction_recovery(&tx, false, false, true),
            RecoveryAction::ClearLog
        );
        assert_eq!(
            decide_transaction_recovery(&tx, true, false, false),
            RecoveryAction::Complete
        );

        let mut rolling_back = tx;
        rolling_back.step = InstallTxStep::RollingBack;
        assert_eq!(
            decide_transaction_recovery(&rolling_back, false, false, false),
            RecoveryAction::FinishRollback,
            "fresh rollback resolves to the original absent state"
        );
    }

    #[test]
    fn prepared_step_reality_based_after_kill_between_rename_and_mark() {
        // Crash window: rename(old→backup) done, mark_old_moved never wrote.
        assert_eq!(
            decide_recovery(InstallTxStep::Prepared, false, true, true),
            RecoveryAction::ContinueInstall
        );
        assert_eq!(
            decide_recovery(InstallTxStep::Prepared, false, true, false),
            RecoveryAction::Rollback
        );
        assert!(matches!(
            decide_recovery(InstallTxStep::Prepared, false, false, true),
            RecoveryAction::KeepManual { .. }
        ));
        assert!(prepared_looks_half_swapped(false, true, true));
        assert!(!prepared_looks_half_swapped(true, false, true));
    }

    fn test_root(name: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("install-tx-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn execute_continue_after_old_moved_crash() {
        let root = test_root("continue");
        let install = root.join("Codex.app");
        let backup = root.join("backup-Codex.app");
        let new_app = root.join("new-Codex.app");
        // Simulate crash after old→backup: install missing, backup+new present.
        fs::create_dir_all(backup.join("Contents")).unwrap();
        fs::write(backup.join("Contents/ver"), "old").unwrap();
        fs::create_dir_all(new_app.join("Contents")).unwrap();
        fs::write(new_app.join("Contents/ver"), "new").unwrap();

        let action = decide_recovery(
            InstallTxStep::OldMoved,
            install.exists(),
            backup.exists(),
            new_app.exists(),
        );
        assert_eq!(action, RecoveryAction::ContinueInstall);
        fs::rename(&new_app, &install).unwrap();
        assert_eq!(
            fs::read_to_string(install.join("Contents/ver")).unwrap(),
            "new"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn execute_rollback_when_new_missing_after_old_moved() {
        let root = test_root("rollback");
        let install = root.join("Codex");
        let backup = root.join("Codex.rollback");
        let new_app = root.join("payload");
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("marker"), "old").unwrap();
        // new missing → rollback
        assert_eq!(
            decide_recovery(
                InstallTxStep::OldMoved,
                install.exists(),
                backup.exists(),
                new_app.exists()
            ),
            RecoveryAction::Rollback
        );
        fs::rename(&backup, &install).unwrap();
        assert_eq!(fs::read_to_string(install.join("marker")).unwrap(), "old");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn kill_between_rename_and_mark_still_recovers_from_prepared() {
        // Full regression: log says Prepared, disk is post-rename(old→backup).
        let root = test_root("kill-between-rename-mark");
        let install = root.join("Codex.app");
        let backup = root.join("backup-Codex.app");
        let new_app = root.join("new-Codex.app");
        fs::create_dir_all(backup.join("Contents")).unwrap();
        fs::write(backup.join("Contents/ver"), "old").unwrap();
        fs::create_dir_all(new_app.join("Contents")).unwrap();
        fs::write(new_app.join("Contents/ver"), "new").unwrap();
        // install intentionally missing (rename already happened).

        let action = decide_recovery(
            InstallTxStep::Prepared, // mark_old_moved never ran
            install.exists(),
            backup.exists(),
            new_app.exists(),
        );
        assert_eq!(
            action,
            RecoveryAction::ContinueInstall,
            "must not ClearLog a half-swap still marked Prepared"
        );
        // Apply continue action.
        fs::rename(&new_app, &install).unwrap();
        assert_eq!(
            fs::read_to_string(install.join("Contents/ver")).unwrap(),
            "new"
        );
        // Drop of Prepared ActiveInstallTx must NOT clear when half-swapped.
        assert!(prepared_looks_half_swapped(false, true, true));
        assert!(!prepared_looks_half_swapped(true, true, false));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn path_is_protected_covers_tx_staging_tree() {
        let staging = PathBuf::from("/tmp/update-abc");
        let nested = staging.join("Codex.app");
        let protected = vec![staging.clone(), nested.clone()];
        assert!(path_is_protected(&nested, &protected));
        assert!(path_is_protected(&staging, &protected));
        assert!(path_is_protected(&staging.join("backup"), &protected));
        assert!(!path_is_protected(Path::new("/tmp/other"), &protected));
    }
}
