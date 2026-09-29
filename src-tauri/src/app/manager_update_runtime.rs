//! Tracks real download/install progress for the Manager's own self-update,
//! independent of any single renderer view. `manager_install_update` drives
//! this runtime as it awaits the updater plugin's `download_and_install`
//! callbacks; `manager_get_update_runtime` lets the renderer reattach to the
//! current snapshot after a reload (or a crash-recovered relaunch) instead of
//! losing progress state, and `manager_ack_update_runtime` clears a terminal
//! (installed/error) snapshot once the renderer has shown it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagerUpdatePhase {
    Idle,
    Downloading,
    Installing,
    Installed,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerUpdateSnapshot {
    pub phase: ManagerUpdatePhase,
    pub version: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
    pub updated_at_ms: u64,
}

impl ManagerUpdateSnapshot {
    fn idle() -> Self {
        Self {
            phase: ManagerUpdatePhase::Idle,
            version: None,
            downloaded: 0,
            total: None,
            message: None,
            updated_at_ms: now_ms(),
        }
    }
}

impl Default for ManagerUpdateSnapshot {
    fn default() -> Self {
        Self::idle()
    }
}

/// Shared, process-wide progress state for the Manager's self-update. One
/// instance lives on `ManagerState`; `manager_install_update` is guarded by
/// `OperationKind::ManagerUpdate` so only one run can drive it at a time, but
/// the snapshot itself stays readable (for reattach) even across that guard.
#[derive(Default)]
pub struct ManagerUpdateRuntime {
    snapshot: Mutex<ManagerUpdateSnapshot>,
    /// Version whose bytes this process has already written to disk. Unlike
    /// `snapshot` it survives `ack()`: dismissing the "installed, awaiting
    /// relaunch" reminder must not make the next check re-offer that same
    /// version, because the running process still reports its old version
    /// until it is relaunched. Cleared only by the process restarting.
    installed_version: Mutex<Option<String>>,
    /// Single-claim guard for `manager_relaunch`: the first caller wins and
    /// actually triggers `AppHandle::request_restart`; later calls (e.g. a
    /// duplicate click while the restart is already in flight) are treated as
    /// idempotent no-ops rather than errors.
    relaunch_reserved: AtomicBool,
}

impl ManagerUpdateRuntime {
    pub fn snapshot(&self) -> ManagerUpdateSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    pub fn start_download(&self, version: &str) {
        let mut guard = self.snapshot.lock().unwrap();
        *guard = ManagerUpdateSnapshot {
            phase: ManagerUpdatePhase::Downloading,
            version: Some(version.to_string()),
            downloaded: 0,
            total: None,
            message: None,
            updated_at_ms: now_ms(),
        };
    }

    /// Accumulates a downloaded chunk. `chunk_len` is the size of the chunk
    /// just received (matching the updater plugin's `on_chunk` callback,
    /// which reports per-chunk length rather than a running total), so this
    /// method carries the running sum itself. Progress reported after the
    /// phase has already moved on (e.g. a stray callback racing the install
    /// step) is ignored so it cannot resurrect a stale byte count.
    pub fn add_progress(&self, chunk_len: u64, total: Option<u64>) {
        let mut guard = self.snapshot.lock().unwrap();
        if guard.phase != ManagerUpdatePhase::Downloading {
            return;
        }
        guard.downloaded = guard.downloaded.saturating_add(chunk_len);
        if let Some(total) = total {
            guard.total = Some(total);
        }
        guard.updated_at_ms = now_ms();
    }

    pub fn mark_installing(&self) {
        let mut guard = self.snapshot.lock().unwrap();
        guard.phase = ManagerUpdatePhase::Installing;
        guard.updated_at_ms = now_ms();
    }

    /// Records that `update.install()` returned `Ok(())`.
    ///
    /// On macOS and Linux, `tauri-plugin-updater`'s `install_inner` swaps the
    /// bundle in place and returns normally, so this fires and the renderer
    /// can show the reattach/"Relaunch Now" UI.
    ///
    /// On Windows, `install_inner` launches the NSIS/MSI installer via
    /// `ShellExecuteW` and then unconditionally calls `std::process::exit(0)`
    /// on success (tauri-plugin-updater 2.12.0, `src/updater.rs`); it never
    /// returns `Ok(())`. So on a normal successful Windows update this method
    /// is never reached, the runtime never observes `Installed`, and the
    /// reattach/relaunch UI this phase drives is unreachable there today —
    /// not merely "non-durable" against a crash. See the Windows NSIS
    /// handoff follow-up tracked in the PR that introduced this runtime.
    pub fn mark_installed(&self) {
        let mut guard = self.snapshot.lock().unwrap();
        *self.installed_version.lock().unwrap() = guard.version.clone();
        guard.phase = ManagerUpdatePhase::Installed;
        guard.message = None;
        guard.updated_at_ms = now_ms();
    }

    /// Whether `version` is the one this process already installed and is
    /// only waiting to relaunch into.
    pub fn is_installed_version(&self, version: &str) -> bool {
        self.installed_version.lock().unwrap().as_deref() == Some(version)
    }

    pub fn mark_error(&self, message: impl Into<String>) {
        let mut guard = self.snapshot.lock().unwrap();
        guard.phase = ManagerUpdatePhase::Error;
        guard.message = Some(message.into());
        guard.updated_at_ms = now_ms();
    }

    /// Clears a terminal snapshot (Installed/Error) back to Idle. Returns
    /// whether anything changed; a no-op while a download/install is
    /// actually in flight so a stray/late ack can never erase live progress.
    pub fn ack(&self) -> bool {
        let mut guard = self.snapshot.lock().unwrap();
        if matches!(
            guard.phase,
            ManagerUpdatePhase::Installed | ManagerUpdatePhase::Error
        ) {
            *guard = ManagerUpdateSnapshot::idle();
            true
        } else {
            false
        }
    }

    /// Single-claim reservation for `manager_relaunch`. Returns `true` for
    /// the caller that wins the race and should actually restart the app.
    pub fn reserve_relaunch(&self) -> bool {
        self.relaunch_reserved
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Releases a claimed reservation without restarting — used when the
    /// restart the caller was about to perform did not actually happen (an
    /// uninterruptible operation elsewhere blocked it). Without this, the
    /// single-claim guard above would permanently silence every later
    /// `manager_relaunch` call for the rest of the process's lifetime, even
    /// after the blocking operation finishes.
    pub fn release_relaunch_reservation(&self) {
        self.relaunch_reserved.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_the_installed_version_across_ack() {
        let runtime = ManagerUpdateRuntime::default();
        assert!(!runtime.is_installed_version("1.2.3"));
        runtime.start_download("1.2.3");
        assert!(!runtime.is_installed_version("1.2.3"));
        runtime.mark_installing();
        runtime.mark_installed();
        assert!(runtime.is_installed_version("1.2.3"));
        assert!(runtime.ack());
        assert_eq!(runtime.snapshot().phase, ManagerUpdatePhase::Idle);
        assert!(runtime.is_installed_version("1.2.3"));
        assert!(!runtime.is_installed_version("1.2.4"));
    }

    #[test]
    fn a_failed_install_does_not_mark_the_version_installed() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.start_download("1.2.3");
        runtime.mark_installing();
        runtime.mark_error("boom");
        assert!(runtime.ack());
        assert!(!runtime.is_installed_version("1.2.3"));
    }

    #[test]
    fn starts_idle() {
        let runtime = ManagerUpdateRuntime::default();
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Idle);
        assert_eq!(snapshot.downloaded, 0);
        assert!(snapshot.total.is_none());
        assert!(snapshot.message.is_none());
    }

    #[test]
    fn tracks_the_full_download_then_install_then_installed_lifecycle() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.start_download("1.2.3");
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Downloading);
        assert_eq!(snapshot.version.as_deref(), Some("1.2.3"));
        assert_eq!(snapshot.downloaded, 0);

        runtime.add_progress(1_000, Some(10_000));
        runtime.add_progress(2_000, Some(10_000));
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.downloaded, 3_000);
        assert_eq!(snapshot.total, Some(10_000));

        runtime.mark_installing();
        assert_eq!(runtime.snapshot().phase, ManagerUpdatePhase::Installing);

        // Progress arriving after the phase has moved on must not resurrect
        // a stale byte count or flip the phase back.
        runtime.add_progress(500, Some(10_000));
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.downloaded, 3_000);
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Installing);

        runtime.mark_installed();
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Installed);
        assert!(snapshot.message.is_none());
    }

    #[test]
    fn error_carries_a_message_and_acks_back_to_idle() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.start_download("1.2.3");
        runtime.mark_error("network unreachable");
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Error);
        assert_eq!(snapshot.message.as_deref(), Some("network unreachable"));

        assert!(runtime.ack());
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Idle);
        assert!(snapshot.message.is_none());
    }

    #[test]
    fn ack_is_a_no_op_while_a_download_or_install_is_in_flight() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.start_download("1.2.3");
        assert!(!runtime.ack());
        assert_eq!(runtime.snapshot().phase, ManagerUpdatePhase::Downloading);

        runtime.mark_installing();
        assert!(!runtime.ack());
        assert_eq!(runtime.snapshot().phase, ManagerUpdatePhase::Installing);
    }

    #[test]
    fn ack_on_an_idle_runtime_reports_no_change() {
        let runtime = ManagerUpdateRuntime::default();
        assert!(!runtime.ack());
    }

    #[test]
    fn relaunch_reservation_is_single_claim() {
        let runtime = ManagerUpdateRuntime::default();
        assert!(runtime.reserve_relaunch());
        assert!(!runtime.reserve_relaunch());
        runtime.release_relaunch_reservation();
        assert!(runtime.reserve_relaunch());
    }

    #[test]
    fn a_released_reservation_can_be_reclaimed_after_a_blocked_restart() {
        // Mirrors `manager_relaunch` finding out the process cannot actually
        // exit (an uninterruptible operation is active elsewhere) and giving
        // the reservation back so a later retry is not silenced forever.
        let runtime = ManagerUpdateRuntime::default();
        assert!(runtime.reserve_relaunch());
        runtime.release_relaunch_reservation();
        assert!(runtime.reserve_relaunch());
        assert!(!runtime.reserve_relaunch());
    }

    #[test]
    fn a_second_download_run_overwrites_a_previous_terminal_snapshot() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.start_download("1.2.3");
        runtime.mark_error("boom");
        // A fresh check-and-install cycle must not require an explicit ack
        // first; starting a new download always wins.
        runtime.start_download("1.2.4");
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Downloading);
        assert_eq!(snapshot.version.as_deref(), Some("1.2.4"));
        assert_eq!(snapshot.downloaded, 0);
        assert!(snapshot.message.is_none());
    }
}
