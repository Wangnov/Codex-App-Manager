//! Recoverable single-transaction machinery for the *stopped-Codex file path*
//! (SPEC §6.2, §7, §10). One transaction wraps one config.toml mutation:
//!
//! ```text
//! begin()            → lock + preimage + journal(prepared)
//! set_phase(...)     → each advance is an atomic, fsynced journal rewrite
//! commit()/rolled_back() → terminal phase, evidence dir removed, lock freed
//! recovery_required()    → terminal-with-evidence: dir kept, new txs refused
//! ```
//!
//! The long-term user baseline is *not* here — that's `native::backup_*`,
//! created once and only deleted by a successful full restore. The preimage
//! saved here is strictly "this transaction's before-bytes", used for
//! immediate rollback and for the §10 startup-recovery decision.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::native::sha256_hex;
use crate::{Result, ThemeEngineError};

fn err(message: impl Into<String>) -> ThemeEngineError {
    ThemeEngineError::Native(message.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Validating,
    Prepared,
    CodexStopped,
    ConfigCommitted,
    CodexLaunched,
    InjectionVerified,
    SelectionPersisted,
    Committed,
    RollingBack,
    RolledBack,
    RecoveryRequired,
}

impl Phase {
    pub fn terminal(self) -> bool {
        matches!(self, Phase::Committed | Phase::RolledBack | Phase::RecoveryRequired)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    pub format_version: u32,
    pub tx_id: String,
    /// apply | switch | off_full | snapshot_restore | recovery
    pub operation: String,
    pub theme_id: Option<String>,
    pub phase: Phase,
    pub started_at: String,
    pub was_codex_running: bool,
    pub previous_active_theme: Option<String>,
    pub preimage_sha256: String,
    pub staged_sha256: Option<String>,
    pub last_error: Option<String>,
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

static TX_COUNTER: AtomicU64 = AtomicU64::new(0);

fn new_tx_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        nanos,
        TX_COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

fn write_atomic_fsync(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| err("transaction path has no parent"))?;
    std::fs::create_dir_all(parent).map_err(|e| err(format!("tx dir: {e}")))?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("tx"),
        std::process::id()
    ));
    let mut file = std::fs::File::create(&tmp).map_err(|e| err(format!("tx temp: {e}")))?;
    file.write_all(bytes).map_err(|e| err(format!("tx write: {e}")))?;
    file.sync_all().map_err(|e| err(format!("tx fsync: {e}")))?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(|e| err(format!("tx commit: {e}")))?;
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

// ── cross-process lock ──────────────────────────────────────────────────────

/// `create_new` lock file carrying the owner pid. Freed on drop; a lock whose
/// owner is dead is reclaimed (checked with `kill -0` on unix, lock-file age
/// elsewhere).
pub struct LockGuard {
    path: PathBuf,
    owned: bool,
}

impl LockGuard {
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| err(format!("lock dir: {e}")))?;
        }
        for attempt in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    let _ = file.write_all(std::process::id().to_string().as_bytes());
                    return Ok(Self { path: path.to_path_buf(), owned: true });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 0 && Self::holder_is_dead(path) {
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    return Err(err(
                        "另一个主题原生事务正在进行（跨进程锁被持有），请稍后再试",
                    ));
                }
                Err(e) => return Err(err(format!("acquire lock: {e}"))),
            }
        }
        Err(err("另一个主题原生事务正在进行，请稍后再试"))
    }

    fn holder_is_dead(path: &Path) -> bool {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(pid) = raw.trim().parse::<u32>() else {
            // Unreadable owner: only reclaim clearly ancient locks.
            return lock_age_secs(path).map(|a| a > 3600).unwrap_or(false);
        };
        if pid == std::process::id() {
            // Our own pid but we don't own the guard: a previous run of this
            // process id (pid reuse) or a leak after a crash-and-restart with
            // the same pid is vanishingly unlikely to be live-held.
            return true;
        }
        #[cfg(unix)]
        {
            std::process::Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .status()
                .map(|s| !s.success())
                .unwrap_or(false)
        }
        #[cfg(windows)]
        {
            // A live pid shows up as a CSV row quoting it; the locale-varying
            // "no tasks" info line never contains the quoted pid.
            std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
                .output()
                .map(|o| {
                    !String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\""))
                })
                .unwrap_or(false)
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            lock_age_secs(path).map(|a| a > 3600).unwrap_or(false)
        }
    }
}

fn lock_age_secs(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    std::time::SystemTime::now().duration_since(modified).ok().map(|d| d.as_secs())
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

// ── transaction ─────────────────────────────────────────────────────────────

pub struct NativeTransaction {
    pub dir: PathBuf,
    journal_path: PathBuf,
    preimage_path: PathBuf,
    journal: Journal,
    _lock: LockGuard,
}

pub struct BeginInput<'a> {
    pub root: &'a Path,
    pub config: &'a Path,
    pub operation: &'a str,
    pub theme_id: Option<String>,
    pub was_codex_running: bool,
    pub previous_active_theme: Option<String>,
}

impl NativeTransaction {
    /// Lock, snapshot the preimage, persist a `prepared` journal. Refuses to
    /// start while an unresolved transaction (evidence dir) exists — §9: after
    /// `recovery_required`, no new native theme transactions.
    pub fn begin(input: BeginInput<'_>) -> Result<Self> {
        std::fs::create_dir_all(input.root).map_err(|e| err(format!("tx root: {e}")))?;
        let lock = LockGuard::acquire(&input.root.join("lock"))?;
        if let Some(pending) = pending_transaction(input.root)? {
            return Err(err(format!(
                "存在未完结的主题事务（{}，phase={:?}）——需要先恢复，txId={}",
                pending.dir.display(),
                pending.journal.phase,
                pending.journal.tx_id
            )));
        }
        let tx_id = new_tx_id();
        let dir = input.root.join(format!("tx-{tx_id}"));
        std::fs::create_dir_all(&dir).map_err(|e| err(format!("tx dir: {e}")))?;
        let preimage_bytes =
            std::fs::read(input.config).map_err(|e| err(format!("read preimage: {e}")))?;
        let preimage_path = dir.join("config.preimage.toml");
        write_atomic_fsync(&preimage_path, &preimage_bytes)?;
        let journal = Journal {
            format_version: 1,
            tx_id,
            operation: input.operation.to_string(),
            theme_id: input.theme_id,
            phase: Phase::Prepared,
            started_at: timestamp(),
            was_codex_running: input.was_codex_running,
            previous_active_theme: input.previous_active_theme,
            preimage_sha256: sha256_hex(&preimage_bytes),
            staged_sha256: None,
            last_error: None,
        };
        let journal_path = dir.join("journal.json");
        let tx = Self {
            dir,
            journal_path,
            preimage_path,
            journal,
            _lock: lock,
        };
        tx.persist()?;
        Ok(tx)
    }

    fn persist(&self) -> Result<()> {
        let rendered = serde_json::to_string_pretty(&self.journal)
            .map_err(|e| err(format!("journal serialize: {e}")))?;
        write_atomic_fsync(&self.journal_path, rendered.as_bytes())
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn preimage_text(&self) -> Result<String> {
        std::fs::read_to_string(&self.preimage_path)
            .map_err(|e| err(format!("read preimage: {e}")))
    }

    pub fn preimage_sha256(&self) -> &str {
        &self.journal.preimage_sha256
    }

    /// Record the staged config (the planned post-write bytes).
    pub fn stage(&mut self, staged_text: &str) -> Result<()> {
        write_atomic_fsync(&self.dir.join("config.staged.toml"), staged_text.as_bytes())?;
        self.journal.staged_sha256 = Some(sha256_hex(staged_text.as_bytes()));
        self.persist()
    }

    pub fn set_phase(&mut self, phase: Phase) -> Result<()> {
        self.journal.phase = phase;
        self.persist()
    }

    pub fn note_error(&mut self, error: &str) -> Result<()> {
        self.journal.last_error = Some(error.to_string());
        self.persist()
    }

    /// Terminal success: journal says committed, evidence dir removed.
    pub fn commit(mut self) -> Result<()> {
        self.set_phase(Phase::Committed)?;
        std::fs::remove_dir_all(&self.dir).map_err(|e| err(format!("tx cleanup: {e}")))
    }

    /// Terminal rollback-success: preimage was restored (or nothing was ever
    /// written); evidence dir removed.
    pub fn rolled_back(mut self) -> Result<()> {
        self.set_phase(Phase::RolledBack)?;
        std::fs::remove_dir_all(&self.dir).map_err(|e| err(format!("tx cleanup: {e}")))
    }

    /// Terminal failure: rollback itself failed. Evidence stays on disk and
    /// blocks new transactions until resolved.
    pub fn recovery_required(mut self, error: &str) -> Result<()> {
        self.journal.last_error = Some(error.to_string());
        self.set_phase(Phase::RecoveryRequired)
    }
}

// ── startup recovery (§10) ──────────────────────────────────────────────────

#[derive(Debug)]
pub struct PendingTransaction {
    pub dir: PathBuf,
    pub journal: Journal,
    pub preimage: PathBuf,
    pub staged: Option<PathBuf>,
}

/// The unresolved transaction under `root`, if any. More than one pending dir
/// is itself a corrupt state → surfaced as an error.
pub fn pending_transaction(root: &Path) -> Result<Option<PendingTransaction>> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(None);
    };
    let mut pending = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir()
            || !dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("tx-"))
                .unwrap_or(false)
        {
            continue;
        }
        let journal_path = dir.join("journal.json");
        let Ok(raw) = std::fs::read_to_string(&journal_path) else {
            // A tx dir without a readable journal is pre-`prepared` debris.
            let _ = std::fs::remove_dir_all(&dir);
            continue;
        };
        let journal: Journal = serde_json::from_str(&raw)
            .map_err(|e| err(format!("journal parse {}: {e}", journal_path.display())))?;
        if journal.phase == Phase::Committed || journal.phase == Phase::RolledBack {
            let _ = std::fs::remove_dir_all(&dir);
            continue;
        }
        let staged = dir.join("config.staged.toml");
        pending.push(PendingTransaction {
            preimage: dir.join("config.preimage.toml"),
            staged: staged.is_file().then_some(staged),
            journal,
            dir,
        });
    }
    match pending.len() {
        0 => Ok(None),
        1 => Ok(pending.pop().map(Some).unwrap_or(None)),
        n => Err(err(format!("发现 {n} 个未完结主题事务，需要人工处理"))),
    }
}

/// What the current config bytes match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskMatch {
    Preimage,
    Staged,
    Neither,
}

pub fn classify_disk(disk_sha: &str, journal: &Journal) -> DiskMatch {
    if disk_sha == journal.preimage_sha256 {
        DiskMatch::Preimage
    } else if journal.staged_sha256.as_deref() == Some(disk_sha) {
        DiskMatch::Staged
    } else {
        DiskMatch::Neither
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Nothing was written — mark rolled back, clean up.
    MarkRolledBack,
    /// The write landed (fully or we can't be sure the op finished) — restore
    /// the preimage, then mark rolled back. Callers may upgrade this to
    /// roll-forward ONLY with external proofs (§10: disk==staged AND persisted
    /// selection matches AND daemon stamp matches).
    RestorePreimage,
    /// Unrecognized bytes — never guess over user changes.
    RequireManual,
}

/// §10 default decision table (before any caller-side roll-forward proof).
pub fn default_recovery_action(phase: Phase, disk: DiskMatch) -> RecoveryAction {
    match (phase, disk) {
        // Pre-commit phases: config untouched → clean; staged bytes on disk
        // mean the crash hit between write and phase-advance → undo.
        (Phase::Validating | Phase::Prepared | Phase::CodexStopped, DiskMatch::Preimage) => {
            RecoveryAction::MarkRolledBack
        }
        (Phase::Validating | Phase::Prepared | Phase::CodexStopped, DiskMatch::Staged) => {
            RecoveryAction::RestorePreimage
        }
        // Post-commit phases: default is rollback (roll-forward needs proofs).
        (
            Phase::ConfigCommitted
            | Phase::CodexLaunched
            | Phase::InjectionVerified
            | Phase::SelectionPersisted,
            DiskMatch::Staged,
        ) => RecoveryAction::RestorePreimage,
        (
            Phase::ConfigCommitted
            | Phase::CodexLaunched
            | Phase::InjectionVerified
            | Phase::SelectionPersisted,
            DiskMatch::Preimage,
        ) => RecoveryAction::MarkRolledBack,
        // Mid-rollback crash: finishing the rollback is idempotent.
        (Phase::RollingBack, DiskMatch::Preimage) => RecoveryAction::MarkRolledBack,
        (Phase::RollingBack, DiskMatch::Staged) => RecoveryAction::RestorePreimage,
        // Unrecognized disk bytes, or a journal already in recovery_required.
        (_, DiskMatch::Neither) => RecoveryAction::RequireManual,
        (Phase::RecoveryRequired, _) => RecoveryAction::RequireManual,
        // Terminal phases shouldn't reach here (pending_transaction cleans
        // them), but the safe answer exists.
        (Phase::Committed | Phase::RolledBack, _) => RecoveryAction::MarkRolledBack,
    }
}

/// Execute the default recovery for a pending transaction: restore preimage
/// when required, then clean up. Returns the action taken. `RequireManual`
/// stamps `recovery_required` and keeps the evidence.
pub fn recover_pending(root: &Path, config: &Path) -> Result<Option<RecoveryAction>> {
    let Some(pending) = pending_transaction(root)? else {
        return Ok(None);
    };
    // Re-acquire the same lock the writer held; a live writer means "not
    // crashed — leave it alone".
    let _lock = LockGuard::acquire(&root.join("lock"))?;
    let disk = std::fs::read(config).map_err(|e| err(format!("read config: {e}")))?;
    let action = default_recovery_action(
        pending.journal.phase,
        classify_disk(&sha256_hex(&disk), &pending.journal),
    );
    match action {
        RecoveryAction::MarkRolledBack => {
            let _ = std::fs::remove_dir_all(&pending.dir);
        }
        RecoveryAction::RestorePreimage => {
            let preimage = std::fs::read(&pending.preimage)
                .map_err(|e| err(format!("read preimage: {e}")))?;
            if sha256_hex(&preimage) != pending.journal.preimage_sha256 {
                return mark_recovery_required(pending, "preimage 校验失败");
            }
            let paths = crate::native::NativeThemePaths {
                config: config.to_path_buf(),
                backup: PathBuf::new(),
            };
            let text = String::from_utf8_lossy(&preimage).into_owned();
            crate::native::write_config_atomic(&paths, &text)?;
            let _ = std::fs::remove_dir_all(&pending.dir);
        }
        RecoveryAction::RequireManual => {
            return mark_recovery_required(pending, "磁盘配置与 preimage/staged 均不匹配");
        }
    }
    Ok(Some(action))
}

fn mark_recovery_required(
    mut pending: PendingTransaction,
    reason: &str,
) -> Result<Option<RecoveryAction>> {
    pending.journal.phase = Phase::RecoveryRequired;
    pending.journal.last_error = Some(reason.to_string());
    let rendered = serde_json::to_string_pretty(&pending.journal)
        .map_err(|e| err(format!("journal serialize: {e}")))?;
    write_atomic_fsync(&pending.dir.join("journal.json"), rendered.as_bytes())?;
    Ok(Some(RecoveryAction::RequireManual))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(root: &Path, config: &Path) -> NativeTransaction {
        NativeTransaction::begin(BeginInput {
            root,
            config,
            operation: "apply",
            theme_id: Some("test-theme".to_string()),
            was_codex_running: true,
            previous_active_theme: None,
        })
        .unwrap()
    }

    #[test]
    fn lifecycle_prepared_to_committed_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config.toml");
        std::fs::write(&config, "model = \"o4\"\n").unwrap();
        let root = tmp.path().join("tx");

        let mut tx = begin(&root, &config);
        assert_eq!(tx.journal().phase, Phase::Prepared);
        tx.stage("model = \"o4\"\nappearanceTheme = \"dark\"\n").unwrap();
        tx.set_phase(Phase::CodexStopped).unwrap();
        tx.set_phase(Phase::ConfigCommitted).unwrap();
        tx.commit().unwrap();

        assert!(pending_transaction(&root).unwrap().is_none());
        assert!(!root.join("lock").exists(), "lock released");
    }

    #[test]
    fn second_transaction_is_locked_out() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config.toml");
        std::fs::write(&config, "x = 1\n").unwrap();
        let root = tmp.path().join("tx");

        let _tx = begin(&root, &config);
        let contender = NativeTransaction::begin(BeginInput {
            root: &root,
            config: &config,
            operation: "apply",
            theme_id: None,
            was_codex_running: false,
            previous_active_theme: None,
        });
        assert!(contender.is_err(), "lock must exclude a second transaction");
    }

    #[test]
    fn stale_lock_from_dead_pid_is_reclaimed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("tx");
        std::fs::create_dir_all(&root).unwrap();
        // A pid that cannot be alive (pid_max on macOS/Linux < 99999999).
        std::fs::write(root.join("lock"), "99999999").unwrap();
        let config = tmp.path().join("config.toml");
        std::fs::write(&config, "x = 1\n").unwrap();
        let tx = begin(&root, &config);
        drop(tx);
    }

    #[test]
    fn crash_recovery_decision_table() {
        use DiskMatch::*;
        use RecoveryAction::*;
        let cases = [
            (Phase::Prepared, Preimage, MarkRolledBack),
            (Phase::Prepared, Staged, RestorePreimage),
            (Phase::Prepared, Neither, RequireManual),
            (Phase::CodexStopped, Preimage, MarkRolledBack),
            (Phase::ConfigCommitted, Staged, RestorePreimage),
            (Phase::ConfigCommitted, Preimage, MarkRolledBack),
            (Phase::ConfigCommitted, Neither, RequireManual),
            (Phase::SelectionPersisted, Staged, RestorePreimage),
            (Phase::RollingBack, Preimage, MarkRolledBack),
            (Phase::RollingBack, Staged, RestorePreimage),
            (Phase::RecoveryRequired, Preimage, RequireManual),
        ];
        for (phase, disk, want) in cases {
            assert_eq!(
                default_recovery_action(phase, disk),
                want,
                "phase={phase:?} disk={disk:?}"
            );
        }
    }

    #[test]
    fn recover_pending_restores_staged_write() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config.toml");
        std::fs::write(&config, "original = true\n").unwrap();
        let root = tmp.path().join("tx");

        // Simulate: staged write landed, then the process died before
        // advancing the phase (journal still `prepared`), lock leaked.
        let mut tx = begin(&root, &config);
        tx.stage("staged = true\n").unwrap();
        std::fs::write(&config, "staged = true\n").unwrap();
        let dir = tx.dir.clone();
        std::mem::forget(tx); // leak the lock + journal like a crash would
        std::fs::write(root.join("lock"), "99999999").unwrap(); // dead owner

        let action = recover_pending(&root, &config).unwrap();
        assert_eq!(action, Some(RecoveryAction::RestorePreimage));
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "original = true\n");
        assert!(!dir.exists(), "evidence cleaned after successful rollback");
        assert!(pending_transaction(&root).unwrap().is_none());
    }

    #[test]
    fn unrecognized_disk_bytes_block_new_transactions() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config.toml");
        std::fs::write(&config, "original = true\n").unwrap();
        let root = tmp.path().join("tx");

        let mut tx = begin(&root, &config);
        tx.stage("staged = true\n").unwrap();
        tx.set_phase(Phase::ConfigCommitted).unwrap();
        std::fs::write(&config, "user_hand_edit = true\n").unwrap();
        std::mem::forget(tx);
        std::fs::write(root.join("lock"), "99999999").unwrap();

        let action = recover_pending(&root, &config).unwrap();
        assert_eq!(action, Some(RecoveryAction::RequireManual));
        // Evidence kept, and new transactions refused.
        assert!(pending_transaction(&root).unwrap().is_some());
        std::fs::write(root.join("lock"), "99999999").unwrap();
        let blocked = NativeTransaction::begin(BeginInput {
            root: &root,
            config: &config,
            operation: "apply",
            theme_id: None,
            was_codex_running: false,
            previous_active_theme: None,
        });
        assert!(blocked.is_err());
        // The config the user hand-edited is untouched.
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "user_hand_edit = true\n"
        );
    }
}
